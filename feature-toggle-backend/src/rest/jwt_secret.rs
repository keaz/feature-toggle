use actix_web::{get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::logic::jwt_secret::JwtSecretLogic;
use crate::rest::error::RestError;
use crate::JwtUser;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JwtSecretResponse {
    pub id: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub secret_preview: String,
}

impl From<crate::database::entity::JwtSecret> for JwtSecretResponse {
    fn from(secret: crate::database::entity::JwtSecret) -> Self {
        let secret_preview = if secret.secret.len() <= 12 {
            secret.secret.clone()
        } else {
            format!(
                "{}...{}",
                &secret.secret[..8],
                &secret.secret[secret.secret.len() - 4..]
            )
        };

        Self {
            id: secret.id.to_string(),
            is_active: secret.is_active,
            created_at: secret.created_at,
            created_by: secret.created_by.map(|id| id.to_string()),
            expires_at: secret.expires_at,
            secret_preview,
        }
    }
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn require_admin(req: &HttpRequest) -> Result<JwtUser, RestError> {
    let jwt = jwt_user(req)?;
    if !jwt.is_admin {
        return Err(RestError::forbidden("Admin access required"));
    }
    Ok(jwt)
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/jwt-secrets",
    responses(
        (status = 200, description = "JWT secrets", body = [JwtSecretResponse]),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[get("/auth/jwt-secrets")]
pub(crate) async fn list_jwt_secrets(
    logic: web::Data<Box<dyn JwtSecretLogic>>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    let secrets = logic.get_all_secrets().await.map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(
        secrets
            .into_iter()
            .map(JwtSecretResponse::from)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/jwt-secrets",
    responses(
        (status = 201, description = "JWT secret created", body = JwtSecretResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/jwt-secrets")]
pub(crate) async fn generate_jwt_secret(
    logic: web::Data<Box<dyn JwtSecretLogic>>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    let jwt = require_admin(&req)?;
    let secret = logic
        .generate_new_secret(Some(jwt.id))
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::Created().json(JwtSecretResponse::from(secret)))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/jwt-secrets/deactivate-all",
    responses(
        (status = 204, description = "JWT secrets deactivated"),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/jwt-secrets/deactivate-all")]
pub(crate) async fn deactivate_all_jwt_secrets(
    logic: web::Data<Box<dyn JwtSecretLogic>>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    logic
        .deactivate_all_secrets()
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::NoContent().finish())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_jwt_secrets)
        .service(generate_jwt_secret)
        .service(deactivate_all_jwt_secrets);
}
