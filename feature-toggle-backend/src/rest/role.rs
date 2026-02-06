use crate::model::ID;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, post, web};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::role::role_repository_tx;
use crate::logic::ActorContext;
use crate::logic::role::{ApiRole, RoleLogic};
use crate::rest::error::RestError;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ApiRole> for RoleResponse {
    fn from(role: ApiRole) -> Self {
        Self {
            id: role.id.to_string(),
            name: role.name,
            description: role.description,
            created_at: role.created_at,
            updated_at: role.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRoleRequest {
    pub name: String,
    pub description: String,
}

fn parse_uuid(value: &str, field: &str) -> Result<uuid::Uuid, RestError> {
    uuid::Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
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

#[utoipa::path(
    get,
    path = "/api/v1/roles",
    responses(
        (status = 200, description = "Roles list", body = [RoleResponse]),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Roles"
)]
#[get("/roles")]
pub(crate) async fn list_roles(
    logic: web::Data<Box<dyn RoleLogic>>,
) -> Result<impl Responder, RestError> {
    let roles = logic.get_all_roles().await.map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(
        roles
            .into_iter()
            .map(RoleResponse::from)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/roles",
    request_body = CreateRoleRequest,
    responses(
        (status = 201, description = "Role created", body = RoleResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Roles"
)]
#[post("/roles")]
pub(crate) async fn create_role(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    payload: web::Json<CreateRoleRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    if !jwt.is_admin {
        return Err(RestError::forbidden("Admin access required"));
    }

    let actor = actor_from_request(&req);
    let repo = role_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::role_tx::create_role_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        payload.name.clone(),
        payload.description.clone(),
        actor,
    )
    .await;

    match result {
        Ok(role) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::Created().json(RoleResponse::from(role)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/roles/{id}",
    params(
        ("id" = String, Path, description = "Role ID")
    ),
    responses(
        (status = 204, description = "Role deleted"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Roles"
)]
#[delete("/roles/{id}")]
pub(crate) async fn delete_role(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    role_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    if !jwt.is_admin {
        return Err(RestError::forbidden("Admin access required"));
    }

    let role_uuid = parse_uuid(&role_id, "role_id")?;
    let actor = actor_from_request(&req);
    let repo = role_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::role_tx::delete_role_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(role_uuid),
        actor,
    )
    .await;

    match result {
        Ok(_) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_roles)
        .service(create_role)
        .service(delete_role);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::PgActivityLogRepository;
    use actix_web::{App, http::StatusCode, test, web};
    use sqlx::postgres::PgPoolOptions;

    fn sample_role() -> ApiRole {
        ApiRole {
            id: ID::from(uuid::Uuid::new_v4()),
            name: "Role".to_string(),
            description: "Role description".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
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

    #[actix_web::test]
    async fn create_role_requires_admin() {
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
            .uri("/api/v1/roles")
            .set_json(CreateRoleRequest {
                name: "Role".to_string(),
                description: "Role".to_string(),
            })
            .to_request();
        req.extensions_mut().insert(JwtUser {
            id: uuid::Uuid::new_v4(),
            username: "user".to_string(),
            is_admin: false,
            roles: vec![],
            token_hash: "hash".to_string(),
        });

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn create_role_returns_created() {
        let pool = test_pool().await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());
        let role_name = format!("Role-{}", uuid::Uuid::new_v4());

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
            .uri("/api/v1/roles")
            .set_json(CreateRoleRequest {
                name: role_name,
                description: "Role".to_string(),
            })
            .to_request();
        let admin_id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
        req.extensions_mut().insert(JwtUser {
            id: admin_id,
            username: "admin".to_string(),
            is_admin: true,
            roles: vec![],
            token_hash: "hash".to_string(),
        });

        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body = test::read_body(resp).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "response body: {}",
            String::from_utf8_lossy(&body)
        );
    }
}
