use actix_web::{get, patch, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use async_graphql::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::activity_log::ActivityLogRepository;
use crate::database::team::{team_repository_tx, TeamRepository};
use crate::logic::team::TeamLogic;
use crate::logic::user::UserLogic;
use crate::logic::ActorContext;
use crate::rest::error::RestError;
use crate::JwtUser;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

impl From<crate::graphql::schema::Team> for TeamResponse {
    fn from(team: crate::graphql::schema::Team) -> Self {
        let description = if team.description.trim().is_empty() {
            None
        } else {
            Some(team.description)
        };
        Self {
            id: team.id.to_string(),
            name: team.name,
            description,
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

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

async fn ensure_team_name_unique(
    repo: &dyn TeamRepository,
    name: &str,
    exclude_id: Option<ID>,
) -> Result<(), RestError> {
    let teams = repo
        .get_teams(Some(name.trim().to_string()))
        .await
        .map_err(RestError::from)?;

    let exclude_uuid = exclude_id
        .map(Uuid::try_from)
        .transpose()
        .map_err(|e| RestError::invalid_input(format!("invalid team_id: {e}")))?;

    let has_conflict = if let Some(exclude) = exclude_uuid {
        teams.iter().any(|team| team.id != exclude)
    } else {
        !teams.is_empty()
    };

    if has_conflict {
        return Err(RestError::conflict(format!(
            "Team with name '{}' already exists",
            name.trim()
        )));
    }

    Ok(())
}

fn validate_team_name(name: &str) -> Result<(), RestError> {
    let trimmed = name.trim();
    if trimmed.len() < 3 || trimmed.len() > 50 {
        return Err(RestError::invalid_input(
            "Team name must be between 3 and 50 characters",
        ));
    }
    Ok(())
}

fn validate_team_description(description: &Option<String>) -> Result<(), RestError> {
    if let Some(value) = description {
        if value.trim().len() > 200 {
            return Err(RestError::invalid_input(
                "Team description must be 200 characters or fewer",
            ));
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/teams",
    responses(
        (status = 200, description = "Teams list", body = [TeamResponse]),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Teams"
)]
#[get("/teams")]
pub(crate) async fn list_teams(
    team_logic: web::Data<Box<dyn TeamLogic>>,
    user_logic: web::Data<Box<dyn UserLogic>>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;

    let teams = if jwt.is_admin {
        team_logic.get_teams(None).await.map_err(RestError::from)?
    } else {
        user_logic
            .get_user_teams(ID::from(jwt.id))
            .await
            .map_err(RestError::from)?
    };

    Ok(HttpResponse::Ok().json(
        teams.into_iter().map(TeamResponse::from).collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams",
    request_body = CreateTeamRequest,
    responses(
        (status = 201, description = "Team created", body = TeamResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Teams"
)]
#[post("/teams")]
pub(crate) async fn create_team(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    payload: web::Json<CreateTeamRequest>,
) -> Result<impl Responder, RestError> {
    validate_team_name(&payload.name)?;
    validate_team_description(&payload.description)?;
    let repo = team_repository_tx(db_pool.get_ref().clone());
    ensure_team_name_unique(&repo, &payload.name, None).await?;

    let actor = actor_from_request(&req);
    let description = payload
        .description
        .as_ref()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let input = crate::graphql::schema::CreateTeamInput {
        name: payload.name.trim().to_string(),
        description,
    };

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::team_tx::create_team_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        input,
        actor,
    )
    .await;

    match result {
        Ok(team) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Created().json(TeamResponse::from(team)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/teams/{id}",
    request_body = UpdateTeamRequest,
    params(
        ("id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Team updated", body = TeamResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Teams"
)]
#[patch("/teams/{id}")]
pub(crate) async fn update_team(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<UpdateTeamRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let id = ID::from(team_uuid);

    let repo = team_repository_tx(db_pool.get_ref().clone());
    if let Some(name) = payload.name.as_deref() {
        validate_team_name(name)?;
        ensure_team_name_unique(&repo, name, Some(id.clone())).await?;
    }
    validate_team_description(&payload.description)?;

    let actor = actor_from_request(&req);
    let input = crate::graphql::schema::UpdateTeamInput {
        name: payload.name.as_ref().map(|value| value.trim().to_string()),
        description: payload
            .description
            .as_ref()
            .map(|value| value.trim().to_string()),
    };

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::team_tx::update_team_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        id,
        input,
        actor,
    )
    .await;

    match result {
        Ok(team) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Ok().json(TeamResponse::from(team)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_teams)
        .service(create_team)
        .service(update_team);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test, web};
    use crate::database::activity_log::PgActivityLogRepository;
    use crate::logic::team::MockTeamLogic;
    use crate::logic::user::MockUserLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_team(team_id: Uuid) -> crate::graphql::schema::Team {
        crate::graphql::schema::Team {
            id: ID::from(team_id),
            name: "Team A".to_string(),
            description: "Core team".to_string(),
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

    async fn insert_team(pool: &sqlx::PgPool, name: &str) -> Uuid {
        let team_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "team test"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    #[actix_web::test]
    async fn list_teams_returns_items_for_admin() {
        let team_id = Uuid::new_v4();
        let team = sample_team(team_id);
        let team_clone = team.clone();

        let mut mock_team_logic = MockTeamLogic::new();
        mock_team_logic
            .expect_get_teams()
            .times(1)
            .returning(move |_| Ok(vec![team_clone.clone()]));

        let mock_user_logic = MockUserLogic::new();

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_team_logic) as Box<dyn TeamLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_user_logic) as Box<dyn UserLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let mut req = test::TestRequest::get().uri("/api/v1/teams").to_request();
        req.extensions_mut().insert(JwtUser {
            id: Uuid::new_v4(),
            username: "admin".to_string(),
            is_admin: true,
            roles: vec![],
            token_hash: "hash".to_string(),
        });

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json[0]["id"], team_id.to_string());
    }

    #[actix_web::test]
    async fn create_team_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let team_name = format!("Team {}", Uuid::new_v4());
        let _team_id = insert_team(&pool, &team_name).await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/teams")
            .set_json(CreateTeamRequest {
                name: team_name,
                description: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[actix_web::test]
    async fn create_team_invalid_name_returns_bad_request() {
        let pool = test_pool().await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/teams")
            .set_json(CreateTeamRequest {
                name: "ab".to_string(),
                description: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
