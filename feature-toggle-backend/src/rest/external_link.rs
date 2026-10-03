//! Links between a feature and issues in an external tracker (Jira).

use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, post, web};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::entity::ExternalLinkRow;
use crate::database::external_link::{ExternalLinkRepository, external_link_repository_tx};
use crate::logic::ActorContext;
use crate::logic::external_link::validate_new_link;
use crate::logic::external_link_tx::{create_external_link_in_tx, delete_external_link_in_tx};
use crate::logic::policy::ActorKind;
use crate::rest::error::RestError;
use crate::rest::operational_safety::{policy_actor_for_request, rest_error_from_policy};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalLinkResponse {
    pub id: String,
    pub feature_id: String,
    /// External system. Only `jira` today.
    pub system: String,
    /// Issue key, upper case, for example `PROJ-123`.
    pub external_key: String,
    pub url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExternalLinksResponse {
    pub items: Vec<ExternalLinkResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateExternalLinkRequest {
    /// External system. Only `jira` is accepted.
    pub system: String,
    /// Jira issue key such as `PROJ-123`; trimmed and upper-cased.
    pub external_key: String,
    /// Optional `http`/`https` link to the issue, at most 2048 characters.
    pub url: Option<String>,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn feature_not_found(feature_id: Uuid) -> RestError {
    RestError::not_found(format!("Feature {feature_id} not found"))
}

/// Users need what `PATCH /features/{id}` needs (system admin, or `Team Admin` of the
/// feature's team). System clients reach the handler only with `flag:write` and a
/// token of the feature's team (`JwtGuard`); the team is checked again here.
async fn authorize_link_write(
    pool: &sqlx::PgPool,
    link_repo: &dyn ExternalLinkRepository,
    feature_id: Uuid,
    jwt: &JwtUser,
) -> Result<(), RestError> {
    let feature = link_repo
        .feature_scope(feature_id)
        .await?
        .ok_or_else(|| feature_not_found(feature_id))?;
    let actor = policy_actor_for_request(pool, jwt).await?;
    if actor.kind == ActorKind::SystemClient {
        return if jwt.team_id == Some(feature.team_id) {
            Ok(())
        } else {
            Err(RestError::forbidden(
                "System client token is not allowed for this resource",
            ))
        };
    }
    crate::logic::policy::authorize_feature_update(pool, feature_id, feature.team_id, actor)
        .await
        .map_err(rest_error_from_policy)
}

fn map_link(link: ExternalLinkRow) -> ExternalLinkResponse {
    ExternalLinkResponse {
        id: link.id.to_string(),
        feature_id: link.feature_id.to_string(),
        system: link.system,
        external_key: link.external_key,
        url: link.url,
        created_by: link.created_by.map(|id| id.to_string()),
        created_at: link.created_at,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/features/{id}/external-links",
    params(("id" = String, Path, description = "Feature ID")),
    responses(
        (status = 200, description = "External links of the feature", body = ExternalLinksResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Feature not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/{id}/external-links")]
pub(crate) async fn list_external_links(
    link_repo: web::Data<Box<dyn ExternalLinkRepository>>,
    feature_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    if link_repo.feature_scope(feature_uuid).await?.is_none() {
        return Err(feature_not_found(feature_uuid));
    }
    let links = link_repo.list_for_feature(feature_uuid).await?;
    Ok(HttpResponse::Ok().json(ExternalLinksResponse {
        items: links.into_iter().map(map_link).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/external-links",
    request_body = CreateExternalLinkRequest,
    params(("id" = String, Path, description = "Feature ID")),
    responses(
        (status = 201, description = "Link created", body = ExternalLinkResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Feature not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "The feature already links this issue", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/features/{id}/external-links")]
pub(crate) async fn create_external_link(
    db_pool: web::Data<sqlx::PgPool>,
    link_repo: web::Data<Box<dyn ExternalLinkRepository>>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_id: web::Path<String>,
    payload: web::Json<CreateExternalLinkRequest>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let jwt = jwt_user(&req)?;
    let payload = payload.into_inner();
    let link = validate_new_link(
        &payload.system,
        &payload.external_key,
        payload.url.as_deref(),
    )?;
    authorize_link_write(
        db_pool.get_ref(),
        link_repo.as_ref().as_ref(),
        feature_uuid,
        &jwt,
    )
    .await?;

    let repo_tx = external_link_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;
    let external_key = link.external_key.clone();
    let created = create_external_link_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        feature_uuid,
        link,
        Some(ActorContext::new(jwt.id, jwt.username.clone())),
    )
    .await
    .map_err(|err| match err {
        crate::Error::RecordAlreadyExists(_) => {
            RestError::conflict(format!("{external_key} is already linked to this feature"))
        }
        other => RestError::from(other),
    })?;
    tx.commit()
        .await
        .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;

    Ok(HttpResponse::Created().json(map_link(created)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/features/{id}/external-links/{link_id}",
    params(
        ("id" = String, Path, description = "Feature ID"),
        ("link_id" = String, Path, description = "External link ID")
    ),
    responses(
        (status = 204, description = "Link removed"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Feature or link not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[delete("/features/{id}/external-links/{link_id}")]
pub(crate) async fn delete_external_link(
    db_pool: web::Data<sqlx::PgPool>,
    link_repo: web::Data<Box<dyn ExternalLinkRepository>>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    path: web::Path<(String, String)>,
) -> Result<impl Responder, RestError> {
    let (feature_id, link_id) = path.into_inner();
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let link_uuid = parse_uuid(&link_id, "link id")?;
    let jwt = jwt_user(&req)?;
    authorize_link_write(
        db_pool.get_ref(),
        link_repo.as_ref().as_ref(),
        feature_uuid,
        &jwt,
    )
    .await?;

    let repo_tx = external_link_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;
    delete_external_link_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        feature_uuid,
        link_uuid,
        Some(ActorContext::new(jwt.id, jwt.username.clone())),
    )
    .await
    .map_err(|err| match err {
        crate::Error::NotFound(id) if id == link_uuid => RestError::not_found(format!(
            "External link {link_uuid} not found on this feature"
        )),
        other => RestError::from(other),
    })?;
    tx.commit()
        .await
        .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;

    Ok(HttpResponse::NoContent().finish())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_external_links)
        .service(create_external_link)
        .service(delete_external_link);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::activity_log_repository;
    use crate::database::external_link::FeatureScope;
    use crate::database::external_link::{
        CreateExternalLink, MockExternalLinkRepository, external_link_repository,
    };
    use actix_web::{App, http::StatusCode, test};
    use sqlx::postgres::PgPoolOptions;

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    /// A team with one feature, a plain member and a team admin.
    struct Fixture {
        pool: sqlx::PgPool,
        team_id: Uuid,
        feature_id: Uuid,
        member_id: Uuid,
        team_admin_id: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let pool = test_pool().await;
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'links')")
                .bind(team_id)
                .bind(format!("external-links-rest-{team_id}"))
                .execute(&pool)
                .await
                .expect("insert team");
            let feature_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, 'ext-rest', 'Simple', $2)",
            )
            .bind(feature_id)
            .bind(team_id)
            .execute(&pool)
            .await
            .expect("insert feature");
            let member_id = insert_user(&pool, "member").await;
            let team_admin_id = insert_user(&pool, "teamadmin").await;
            for user_id in [member_id, team_admin_id] {
                sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
                    .bind(user_id)
                    .bind(team_id)
                    .execute(&pool)
                    .await
                    .expect("insert user team");
            }
            Self {
                pool,
                team_id,
                feature_id,
                member_id,
                team_admin_id,
            }
        }

        fn admin(&self) -> JwtUser {
            jwt(self.member_id, true, vec![], None)
        }

        fn team_admin(&self) -> JwtUser {
            jwt(
                self.team_admin_id,
                false,
                vec!["Team Admin".to_string()],
                None,
            )
        }

        fn member(&self) -> JwtUser {
            jwt(self.member_id, false, vec!["Requester".to_string()], None)
        }

        /// A `flag:write` system client of `team_id` (the guard checked scope and team).
        async fn system_client(&self, team_id: Uuid) -> JwtUser {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO system_clients (id, team_id, name, enabled, expires_at) \
                 VALUES ($1, $2, $3, TRUE, NOW() + INTERVAL '1 day')",
            )
            .bind(id)
            .bind(team_id)
            .bind(format!("jira-{id}"))
            .execute(&self.pool)
            .await
            .expect("insert system client");
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, first_name, last_name, email, enabled) \
                 VALUES ($1, $2, 'x', 'System', 'Client', $3, TRUE)",
            )
            .bind(id)
            .bind(format!("sc_{id}"))
            .bind(format!("sc_{id}@example.com"))
            .execute(&self.pool)
            .await
            .expect("insert system client user");
            jwt(
                id,
                false,
                vec!["Requester".to_string(), "Approver".to_string()],
                Some(team_id),
            )
        }

        async fn other_team(&self) -> Uuid {
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'other')")
                .bind(team_id)
                .bind(format!("external-links-other-{team_id}"))
                .execute(&self.pool)
                .await
                .expect("insert other team");
            team_id
        }

        async fn send(
            &self,
            req: actix_http::Request,
            user: Option<JwtUser>,
        ) -> (StatusCode, serde_json::Value) {
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(self.pool.clone()))
                    .app_data(web::Data::new(external_link_repository(self.pool.clone())))
                    .app_data(web::Data::new(activity_log_repository(self.pool.clone())))
                    .service(web::scope("/api/v1").configure(super::configure)),
            )
            .await;
            if let Some(user) = user {
                req.extensions_mut().insert(user);
            }
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (status, serde_json::from_slice(&bytes).unwrap_or_default())
        }

        async fn post_link(
            &self,
            feature_id: Uuid,
            body: serde_json::Value,
            user: JwtUser,
        ) -> (StatusCode, serde_json::Value) {
            let req = test::TestRequest::post()
                .uri(&format!("/api/v1/features/{feature_id}/external-links"))
                .set_json(body)
                .to_request();
            self.send(req, Some(user)).await
        }

        async fn delete_link(
            &self,
            feature_id: Uuid,
            link_id: &str,
            user: JwtUser,
        ) -> (StatusCode, serde_json::Value) {
            let req = test::TestRequest::delete()
                .uri(&format!(
                    "/api/v1/features/{feature_id}/external-links/{link_id}"
                ))
                .to_request();
            self.send(req, Some(user)).await
        }

        async fn link_count(&self) -> i64 {
            sqlx::query_scalar("SELECT COUNT(*) FROM feature_external_links WHERE feature_id = $1")
                .bind(self.feature_id)
                .fetch_one(&self.pool)
                .await
                .expect("count links")
        }

        async fn cleanup(self, extra_teams: &[Uuid]) {
            for team_id in std::iter::once(&self.team_id).chain(extra_teams) {
                sqlx::query(
                    "DELETE FROM users WHERE id IN (SELECT id FROM system_clients WHERE team_id = $1)",
                )
                .bind(team_id)
                .execute(&self.pool)
                .await
                .expect("delete system client users");
                sqlx::query("DELETE FROM features WHERE team_id = $1")
                    .bind(team_id)
                    .execute(&self.pool)
                    .await
                    .expect("delete features");
                sqlx::query("DELETE FROM teams WHERE id = $1")
                    .bind(team_id)
                    .execute(&self.pool)
                    .await
                    .expect("delete team");
            }
            sqlx::query("DELETE FROM users WHERE id = ANY($1)")
                .bind(vec![self.member_id, self.team_admin_id])
                .execute(&self.pool)
                .await
                .expect("delete users");
        }
    }

    async fn insert_user(pool: &sqlx::PgPool, prefix: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, first_name, last_name, email, enabled) \
             VALUES ($1, $2, 'x', 'Link', 'Tester', $3, TRUE)",
        )
        .bind(user_id)
        .bind(format!("{prefix}_{user_id}"))
        .bind(format!("{prefix}_{user_id}@example.com"))
        .execute(pool)
        .await
        .expect("insert user");
        user_id
    }

    fn jwt(id: Uuid, is_admin: bool, roles: Vec<String>, team_id: Option<Uuid>) -> JwtUser {
        JwtUser {
            id,
            username: format!("user-{id}"),
            is_admin,
            roles,
            team_id,
            token_hash: "hash".to_string(),
        }
    }

    fn jira(key: &str) -> serde_json::Value {
        serde_json::json!({
            "system": "jira",
            "externalKey": key,
            "url": format!("https://acme.atlassian.net/browse/{}", key.trim().to_uppercase()),
        })
    }

    #[actix_web::test]
    async fn create_returns_201_with_the_normalized_link() {
        let fixture = Fixture::new().await;
        let (status, body) = fixture
            .post_link(fixture.feature_id, jira(" proj-42 "), fixture.admin())
            .await;

        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["featureId"], fixture.feature_id.to_string());
        assert_eq!(body["system"], "jira");
        assert_eq!(body["externalKey"], "PROJ-42");
        assert_eq!(body["url"], "https://acme.atlassian.net/browse/PROJ-42");
        assert_eq!(body["createdBy"], fixture.member_id.to_string());
        assert!(body["id"].as_str().is_some());
        assert!(body["createdAt"].as_str().is_some());
        assert_eq!(fixture.link_count().await, 1);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn create_duplicate_returns_409() {
        let fixture = Fixture::new().await;
        let (first, _) = fixture
            .post_link(fixture.feature_id, jira("PROJ-1"), fixture.team_admin())
            .await;
        assert_eq!(first, StatusCode::CREATED);

        let (status, body) = fixture
            .post_link(fixture.feature_id, jira("proj-1"), fixture.team_admin())
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["message"], "PROJ-1 is already linked to this feature");
        assert_eq!(fixture.link_count().await, 1);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn create_rejects_bad_key_system_and_url_with_400() {
        let fixture = Fixture::new().await;
        for body in [
            serde_json::json!({"system": "jira", "externalKey": "PROJ"}),
            serde_json::json!({"system": "jira", "externalKey": "PROJ-0"}),
            serde_json::json!({"system": "github", "externalKey": "PROJ-1"}),
            serde_json::json!({"system": "jira", "externalKey": "PROJ-1", "url": "javascript:alert(1)"}),
        ] {
            let (status, response) = fixture
                .post_link(fixture.feature_id, body.clone(), fixture.admin())
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {response}");
        }
        assert_eq!(fixture.link_count().await, 0);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn create_on_missing_feature_returns_404() {
        let fixture = Fixture::new().await;
        let (status, body) = fixture
            .post_link(Uuid::new_v4(), jira("PROJ-1"), fixture.admin())
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn create_requires_feature_update_rights_for_users() {
        let fixture = Fixture::new().await;
        let (status, body) = fixture
            .post_link(fixture.feature_id, jira("PROJ-1"), fixture.member())
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

        let (status, body) = fixture
            .post_link(fixture.feature_id, jira("PROJ-1"), fixture.team_admin())
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn create_and_delete_without_a_user_return_401() {
        let fixture = Fixture::new().await;
        let req = test::TestRequest::post()
            .uri(&format!(
                "/api/v1/features/{}/external-links",
                fixture.feature_id
            ))
            .set_json(jira("PROJ-1"))
            .to_request();
        let (status, _) = fixture.send(req, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let req = test::TestRequest::delete()
            .uri(&format!(
                "/api/v1/features/{}/external-links/{}",
                fixture.feature_id,
                Uuid::new_v4()
            ))
            .to_request();
        let (status, _) = fixture.send(req, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn system_client_of_the_team_can_link_and_unlink() {
        let fixture = Fixture::new().await;
        let client = fixture.system_client(fixture.team_id).await;

        let (status, body) = fixture
            .post_link(fixture.feature_id, jira("PROJ-9"), client.clone())
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["createdBy"], client.id.to_string());

        let link_id = body["id"].as_str().unwrap().to_string();
        let (status, body) = fixture
            .delete_link(fixture.feature_id, &link_id, client)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn system_client_of_another_team_is_forbidden() {
        let fixture = Fixture::new().await;
        let other_team = fixture.other_team().await;
        let client = fixture.system_client(other_team).await;

        let (status, body) = fixture
            .post_link(fixture.feature_id, jira("PROJ-9"), client)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(fixture.link_count().await, 0);

        fixture.cleanup(&[other_team]).await;
    }

    #[actix_web::test]
    async fn delete_returns_204_then_404() {
        let fixture = Fixture::new().await;
        let (_, created) = fixture
            .post_link(fixture.feature_id, jira("PROJ-5"), fixture.admin())
            .await;
        let link_id = created["id"].as_str().unwrap().to_string();

        let (status, body) = fixture
            .delete_link(fixture.feature_id, &link_id, fixture.admin())
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        assert_eq!(fixture.link_count().await, 0);

        let (status, body) = fixture
            .delete_link(fixture.feature_id, &link_id, fixture.admin())
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn delete_of_a_link_on_another_feature_returns_404() {
        let fixture = Fixture::new().await;
        let other_feature = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, 'ext-rest-2', 'Simple', $2)",
        )
        .bind(other_feature)
        .bind(fixture.team_id)
        .execute(&fixture.pool)
        .await
        .expect("insert feature");
        let link = external_link_repository(fixture.pool.clone())
            .create(CreateExternalLink {
                feature_id: other_feature,
                system: "jira".to_string(),
                external_key: "PROJ-3".to_string(),
                url: None,
                created_by: None,
            })
            .await
            .expect("create link");

        let (status, body) = fixture
            .delete_link(fixture.feature_id, &link.id.to_string(), fixture.admin())
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn list_returns_the_links_of_the_feature() {
        let feature_id = Uuid::new_v4();
        let link = ExternalLinkRow {
            id: Uuid::new_v4(),
            feature_id,
            system: "jira".to_string(),
            external_key: "PROJ-123".to_string(),
            url: Some("https://acme.atlassian.net/browse/PROJ-123".to_string()),
            created_by: None,
            created_at: Utc::now(),
        };
        let mut repo = MockExternalLinkRepository::new();
        repo.expect_feature_scope()
            .withf(move |id| *id == feature_id)
            .returning(|_| {
                Ok(Some(FeatureScope {
                    team_id: Uuid::new_v4(),
                    key: "checkout".to_string(),
                }))
            });
        let returned = link.clone();
        repo.expect_list_for_feature()
            .withf(move |id| *id == feature_id)
            .returning(move |_| Ok(vec![returned.clone()]));
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(repo) as Box<dyn ExternalLinkRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/api/v1/features/{feature_id}/external-links"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(
            body,
            serde_json::json!({
                "items": [{
                    "id": link.id.to_string(),
                    "featureId": feature_id.to_string(),
                    "system": "jira",
                    "externalKey": "PROJ-123",
                    "url": "https://acme.atlassian.net/browse/PROJ-123",
                    "createdBy": null,
                    "createdAt": serde_json::to_value(link.created_at).unwrap(),
                }]
            })
        );
    }

    #[actix_web::test]
    async fn list_of_a_missing_feature_returns_404() {
        let mut repo = MockExternalLinkRepository::new();
        repo.expect_feature_scope().returning(|_| Ok(None));
        repo.expect_list_for_feature().never();
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(repo) as Box<dyn ExternalLinkRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!(
                "/api/v1/features/{}/external-links",
                Uuid::new_v4()
            ))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
