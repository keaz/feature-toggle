use actix_web::{delete, get, patch, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::model::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::activity_log::ActivityLogRepository;
use crate::database::environment::{environment_repository_tx, EnvironmentRepository};
use crate::logic::environment::EnvironmentLogic;
use crate::logic::ActorContext;
use crate::rest::error::RestError;
use crate::rest::pagination::{normalize_pagination, PageMeta};
use crate::JwtUser;

#[derive(Debug, Deserialize, ToSchema)]
pub struct EnvironmentListQuery {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentResponse {
    pub id: String,
    pub name: String,
    pub team_id: String,
    pub active: bool,
    pub environment_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EnvironmentsResponse {
    pub items: Vec<EnvironmentResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateEnvironmentRequest {
    pub name: String,
    pub active: bool,
    pub environment_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEnvironmentRequest {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub environment_type: Option<String>,
}

impl From<crate::model::Environment> for EnvironmentResponse {
    fn from(env: crate::model::Environment) -> Self {
        Self {
            id: env.id.to_string(),
            name: env.name,
            team_id: env.team_id.to_string(),
            active: env.active,
            environment_type: env.environment_type,
        }
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

async fn ensure_environment_name_unique(
    repo: &dyn EnvironmentRepository,
    team_id: ID,
    name: &str,
    exclude_id: Option<ID>,
) -> Result<(), RestError> {
    if name.trim().is_empty() {
        return Ok(());
    }

    let team_uuid = Uuid::try_from(team_id)
        .map_err(|e| RestError::invalid_input(format!("invalid team_id: {e}")))?;
    let environments = repo
        .get_environments(team_uuid, Some(name.to_string()), None)
        .await
        .map_err(RestError::from)?;

    let exclude_uuid = exclude_id
        .map(Uuid::try_from)
        .transpose()
        .map_err(|e| RestError::invalid_input(format!("invalid environment_id: {e}")))?;

    let has_conflict = if let Some(exclude) = exclude_uuid {
        environments.iter().any(|env| env.id != exclude)
    } else {
        !environments.is_empty()
    };

    if has_conflict {
        return Err(RestError::conflict(format!(
            "Environment with name '{}' already exists",
            name
        )));
    }

    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/environments",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("name" = Option<String>, Query, description = "Filter by environment name"),
        ("active" = Option<bool>, Query, description = "Filter by active status"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Environments list", body = EnvironmentsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Environments"
)]
#[get("/teams/{team_id}/environments")]
pub(crate) async fn list_environments(
    logic: web::Data<Box<dyn EnvironmentLogic>>,
    team_id: web::Path<String>,
    query: web::Query<EnvironmentListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let (offset, limit) = normalize_pagination(&crate::rest::pagination::PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let name = query
        .name
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let (items, total) = logic
        .get_environments_with_offset(
            ID::from(team_uuid),
            name,
            query.active,
            offset,
            limit,
        )
        .await
        .map_err(RestError::from)?;

    let response = EnvironmentsResponse {
        items: items.into_iter().map(EnvironmentResponse::from).collect(),
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    };

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    get,
    path = "/api/v1/environments/{id}",
    params(
        ("id" = String, Path, description = "Environment ID")
    ),
    responses(
        (status = 200, description = "Environment", body = EnvironmentResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Environments"
)]
#[get("/environments/{id}")]
pub(crate) async fn get_environment(
    logic: web::Data<Box<dyn EnvironmentLogic>>,
    env_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let env_uuid = parse_uuid(&env_id, "environment id")?;
    let environment = logic
        .get_environment_by_id(ID::from(env_uuid))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(EnvironmentResponse::from(environment)))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/environments",
    request_body = CreateEnvironmentRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Environment created", body = EnvironmentResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Environments"
)]
#[post("/teams/{team_id}/environments")]
pub(crate) async fn create_environment(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<CreateEnvironmentRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let actor = actor_from_request(&req);
    let repo = environment_repository_tx(db_pool.get_ref().clone());
    ensure_environment_name_unique(&repo, ID::from(team_uuid), payload.name.as_str(), None).await?;

    let input = crate::model::CreateEnvironmentInput {
        name: payload.name.clone(),
        active: payload.active,
        environment_type: payload.environment_type.clone(),
    };

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::environment_tx::create_environment_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(team_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(environment) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Created().json(EnvironmentResponse::from(environment)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/environments/{id}",
    request_body = UpdateEnvironmentRequest,
    params(
        ("id" = String, Path, description = "Environment ID")
    ),
    responses(
        (status = 200, description = "Environment updated", body = EnvironmentResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Environments"
)]
#[patch("/environments/{id}")]
pub(crate) async fn update_environment(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    env_id: web::Path<String>,
    payload: web::Json<UpdateEnvironmentRequest>,
) -> Result<impl Responder, RestError> {
    let env_uuid = parse_uuid(&env_id, "environment id")?;
    let actor = actor_from_request(&req);
    let repo = environment_repository_tx(db_pool.get_ref().clone());
    if let Some(name) = payload
        .name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let existing = repo
            .get_environment_by_id(env_uuid)
            .await
            .map_err(RestError::from)?;
        ensure_environment_name_unique(
            &repo,
            ID::from(existing.team_id),
            name,
            Some(ID::from(existing.id)),
        )
        .await?;
    }

    let input = crate::model::UpdateEnvironmentInput {
        name: payload.name.clone(),
        active: payload.active,
        environment_type: payload.environment_type.clone(),
    };

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::environment_tx::update_environment_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(env_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(environment) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Ok().json(EnvironmentResponse::from(environment)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/environments/{id}",
    params(
        ("id" = String, Path, description = "Environment ID")
    ),
    responses(
        (status = 204, description = "Environment deleted"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Environments"
)]
#[delete("/environments/{id}")]
pub(crate) async fn delete_environment(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    env_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let env_uuid = parse_uuid(&env_id, "environment id")?;
    let actor = actor_from_request(&req);
    let repo = environment_repository_tx(db_pool.get_ref().clone());
    let existing = repo
        .get_environment_by_id(env_uuid)
        .await
        .map_err(RestError::from)?;

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::environment_tx::delete_environment_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(env_uuid),
        existing.name,
        actor,
    )
    .await;

    match result {
        Ok(_) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_environments)
        .service(get_environment)
        .service(create_environment)
        .service(update_environment)
        .service(delete_environment);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test, web};

    use crate::database::activity_log::PgActivityLogRepository;
    use crate::logic::environment::MockEnvironmentLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_env(env_id: Uuid, team_id: Uuid) -> crate::model::Environment {
        crate::model::Environment {
            id: ID::from(env_id),
            name: "Production".to_string(),
            team_id: ID::from(team_id),
            active: true,
            environment_type: "Production".to_string(),
        }
    }

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    async fn insert_team(pool: &sqlx::PgPool) -> Uuid {
        let team_id = Uuid::new_v4();
        let name = format!("env-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "env test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_environment(pool: &sqlx::PgPool, team_id: Uuid, name: &str) -> Uuid {
        let env_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO environments (id, name, active, team_id, environment_type)
               VALUES ($1, $2, $3, $4, $5)"#,
            env_id,
            name,
            true,
            team_id,
            "Development"
        )
        .execute(pool)
        .await
        .expect("Failed to insert environment");
        env_id
    }

    #[actix_web::test]
    async fn list_environments_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let env = sample_env(env_id, team_id);
        let env_clone = env.clone();

        let mut mock_logic = MockEnvironmentLogic::new();
        mock_logic
            .expect_get_environments_with_offset()
            .withf(move |id, name, active, offset, limit| {
                id.to_string() == team_id.to_string()
                    && name.as_deref() == Some("prod")
                    && *active == Some(true)
                    && *offset == 5
                    && *limit == 10
            })
            .times(1)
            .returning(move |_, _, _, _, _| Ok((vec![env_clone.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn EnvironmentLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!(
            "/api/v1/teams/{team_id}/environments?offset=5&limit=10&name=prod&active=true"
        );
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], env_id.to_string());
        assert_eq!(json["items"][0]["teamId"], team_id.to_string());
        assert_eq!(json["meta"]["offset"], 5);
        assert_eq!(json["meta"]["limit"], 10);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn get_environment_returns_environment() {
        let team_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let env = sample_env(env_id, team_id);
        let env_clone = env.clone();

        let mut mock_logic = MockEnvironmentLogic::new();
        mock_logic
            .expect_get_environment_by_id()
            .withf(move |id| id.to_string() == env_id.to_string())
            .times(1)
            .returning(move |_| Ok(env_clone.clone()));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn EnvironmentLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/environments/{env_id}");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], env_id.to_string());
        assert_eq!(json["teamId"], team_id.to_string());
    }

    #[actix_web::test]
    async fn get_environment_invalid_id_returns_bad_request() {
        let mut mock_logic = MockEnvironmentLogic::new();
        mock_logic.expect_get_environment_by_id().times(0);

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn EnvironmentLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/v1/environments/not-a-uuid")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "invalid_input");
        assert_eq!(json["message"], "invalid environment id");
    }

    #[actix_web::test]
    async fn create_environment_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/environments");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateEnvironmentRequest {
                name: "New Env".to_string(),
                active: true,
                environment_type: Some("Development".to_string()),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["teamId"], team_id.to_string());
    }

    #[actix_web::test]
    async fn create_environment_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let _existing_env = insert_environment(&pool, team_id, "New Env").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/environments");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateEnvironmentRequest {
                name: "New Env".to_string(),
                active: true,
                environment_type: Some("Development".to_string()),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
        assert_eq!(json["message"], "Environment with name 'New Env' already exists");
    }

    #[actix_web::test]
    async fn update_environment_returns_updated() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id, "Old Env").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/environments/{env_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateEnvironmentRequest {
                name: Some("Updated Env".to_string()),
                active: Some(false),
                environment_type: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn update_environment_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id, "Primary Env").await;
        let _other_env_id = insert_environment(&pool, team_id, "Updated Env").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/environments/{env_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateEnvironmentRequest {
                name: Some("Updated Env".to_string()),
                active: Some(false),
                environment_type: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
        assert_eq!(json["message"], "Environment with name 'Updated Env' already exists");
    }

    #[actix_web::test]
    async fn delete_environment_returns_no_content() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id, "Delete Env").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/environments/{env_id}");
        let req = test::TestRequest::delete().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
}
