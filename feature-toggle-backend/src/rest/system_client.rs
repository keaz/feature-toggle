use actix_web::{HttpResponse, Responder, get, patch, post, web};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::logic::system_client::{
    SystemClientLogic, SystemClientTokenResult, default_system_client_scopes,
};
use crate::model::{
    CreateSystemClientInput, CreateSystemClientTokenInput, ID, SystemClient as ModelSystemClient,
    SystemClientToken as ModelSystemClientToken, UpdateSystemClientInput,
};
use crate::rest::error::RestError;
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemClientListQuery {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemClientResponse {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemClientsResponse {
    pub items: Vec<SystemClientResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemClientWithTokenResponse {
    pub system_client: SystemClientResponse,
    pub token_meta: SystemClientTokenResponse,
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSystemClientRequest {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub expires_at: DateTime<Utc>,
    pub token_name: Option<String>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSystemClientRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSystemClientTokenRequest {
    pub name: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemClientTokenResponse {
    pub id: String,
    pub system_client_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub is_revoked: bool,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemClientTokensResponse {
    pub items: Vec<SystemClientTokenResponse>,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn validate_name(name: &str) -> Result<(), RestError> {
    let trimmed = name.trim();
    if trimmed.len() < 3 || trimmed.len() > 100 {
        return Err(RestError::invalid_input(
            "System client name must be between 3 and 100 characters",
        ));
    }
    Ok(())
}

fn validate_description(description: &Option<String>) -> Result<(), RestError> {
    if let Some(value) = description
        && value.len() > 500
    {
        return Err(RestError::invalid_input(
            "System client description must be 500 characters or fewer",
        ));
    }
    Ok(())
}

impl From<ModelSystemClient> for SystemClientResponse {
    fn from(value: ModelSystemClient) -> Self {
        Self {
            id: value.id.to_string(),
            team_id: value.team_id.to_string(),
            name: value.name,
            description: value.description,
            enabled: value.enabled,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            last_used_at: value.last_used_at,
        }
    }
}

impl From<ModelSystemClientToken> for SystemClientTokenResponse {
    fn from(value: ModelSystemClientToken) -> Self {
        Self {
            id: value.id.to_string(),
            system_client_id: value.system_client_id.to_string(),
            name: value.name,
            scopes: value.scopes,
            expires_at: value.expires_at,
            created_at: value.created_at,
            revoked_at: value.revoked_at,
            is_revoked: value.is_revoked,
            last_used_at: value.last_used_at,
        }
    }
}

impl From<SystemClientTokenResult> for SystemClientWithTokenResponse {
    fn from(value: SystemClientTokenResult) -> Self {
        Self {
            system_client: SystemClientResponse::from(value.system_client),
            token_meta: SystemClientTokenResponse::from(value.token_meta),
            token: value.token,
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/system-clients",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("name" = Option<String>, Query, description = "Filter by name"),
        ("enabled" = Option<bool>, Query, description = "Filter by enabled status"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "System clients", body = SystemClientsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[get("/teams/{team_id}/system-clients")]
pub(crate) async fn list_system_clients(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    team_id: web::Path<String>,
    query: web::Query<SystemClientListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
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
        .list_system_clients(ID::from(team_uuid), name, query.enabled, offset, limit)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(SystemClientsResponse {
        items: items.into_iter().map(SystemClientResponse::from).collect(),
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/system-clients/{id}",
    params(
        ("id" = String, Path, description = "System client ID")
    ),
    responses(
        (status = 200, description = "System client", body = SystemClientResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[get("/system-clients/{id}")]
pub(crate) async fn get_system_client(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let system_client_id = parse_uuid(&id, "system client id")?;
    let item = logic
        .get_system_client_by_id(ID::from(system_client_id))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(SystemClientResponse::from(item)))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/system-clients",
    request_body = CreateSystemClientRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "System client created", body = SystemClientWithTokenResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[post("/teams/{team_id}/system-clients")]
pub(crate) async fn create_system_client(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    team_id: web::Path<String>,
    payload: web::Json<CreateSystemClientRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team id")?;
    validate_name(&payload.name)?;
    validate_description(&payload.description)?;

    let created = logic
        .create_system_client(
            ID::from(team_uuid),
            CreateSystemClientInput {
                name: payload.name.trim().to_string(),
                description: payload.description.clone(),
                enabled: payload.enabled,
                expires_at: payload.expires_at,
                token_name: payload.token_name.clone(),
                scopes: payload
                    .scopes
                    .clone()
                    .filter(|scopes| !scopes.is_empty())
                    .unwrap_or_else(default_system_client_scopes),
            },
        )
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Created().json(SystemClientWithTokenResponse::from(created)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/system-clients/{id}",
    request_body = UpdateSystemClientRequest,
    params(
        ("id" = String, Path, description = "System client ID")
    ),
    responses(
        (status = 200, description = "System client updated", body = SystemClientResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[patch("/system-clients/{id}")]
pub(crate) async fn update_system_client(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    id: web::Path<String>,
    payload: web::Json<UpdateSystemClientRequest>,
) -> Result<impl Responder, RestError> {
    let system_client_id = parse_uuid(&id, "system client id")?;

    if let Some(name) = payload.name.as_deref() {
        validate_name(name)?;
    }
    validate_description(&payload.description)?;

    let updated = logic
        .update_system_client(
            ID::from(system_client_id),
            UpdateSystemClientInput {
                name: payload.name.as_ref().map(|value| value.trim().to_string()),
                description: payload.description.clone(),
                enabled: payload.enabled,
                expires_at: payload.expires_at,
            },
        )
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(SystemClientResponse::from(updated)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system-clients/{id}/regenerate-token",
    params(
        ("id" = String, Path, description = "System client ID")
    ),
    responses(
        (status = 200, description = "Token regenerated", body = SystemClientWithTokenResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[post("/system-clients/{id}/regenerate-token")]
pub(crate) async fn regenerate_system_client_token(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    id: web::Path<String>,
    payload: Option<web::Json<CreateSystemClientTokenRequest>>,
) -> Result<impl Responder, RestError> {
    let system_client_id = parse_uuid(&id, "system client id")?;
    let payload = payload
        .map(|body| body.into_inner())
        .unwrap_or(CreateSystemClientTokenRequest {
            name: None,
            scopes: None,
            expires_at: None,
        });

    let result = logic
        .regenerate_token(
            ID::from(system_client_id),
            CreateSystemClientTokenInput {
                name: payload.name,
                scopes: payload
                    .scopes
                    .filter(|scopes| !scopes.is_empty())
                    .unwrap_or_else(default_system_client_scopes),
                expires_at: payload.expires_at,
            },
        )
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(SystemClientWithTokenResponse::from(result)))
}

#[utoipa::path(
    get,
    path = "/api/v1/system-clients/{id}/tokens",
    params(("id" = String, Path, description = "System client ID")),
    responses(
        (status = 200, description = "System client tokens", body = SystemClientTokensResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[get("/system-clients/{id}/tokens")]
pub(crate) async fn list_system_client_tokens(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let system_client_id = parse_uuid(&id, "system client id")?;
    let tokens = logic
        .list_tokens(ID::from(system_client_id))
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(SystemClientTokensResponse {
        items: tokens
            .into_iter()
            .map(SystemClientTokenResponse::from)
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/system-clients/{id}/tokens",
    request_body = CreateSystemClientTokenRequest,
    params(("id" = String, Path, description = "System client ID")),
    responses(
        (status = 201, description = "Token created", body = SystemClientWithTokenResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[post("/system-clients/{id}/tokens")]
pub(crate) async fn create_system_client_token(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    id: web::Path<String>,
    payload: web::Json<CreateSystemClientTokenRequest>,
) -> Result<impl Responder, RestError> {
    let system_client_id = parse_uuid(&id, "system client id")?;
    let payload = payload.into_inner();
    let result = logic
        .create_token(
            ID::from(system_client_id),
            CreateSystemClientTokenInput {
                name: payload.name,
                scopes: payload
                    .scopes
                    .filter(|scopes| !scopes.is_empty())
                    .unwrap_or_else(default_system_client_scopes),
                expires_at: payload.expires_at,
            },
        )
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::Created().json(SystemClientWithTokenResponse::from(result)))
}

#[utoipa::path(
    post,
    path = "/api/v1/system-client-tokens/{id}/revoke",
    params(("id" = String, Path, description = "System client token ID")),
    responses(
        (status = 204, description = "Token revoked"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "System Clients"
)]
#[post("/system-client-tokens/{id}/revoke")]
pub(crate) async fn revoke_system_client_token(
    logic: web::Data<Box<dyn SystemClientLogic>>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let token_id = parse_uuid(&id, "system client token id")?;
    let _ = logic
        .revoke_token(ID::from(token_id))
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::NoContent().finish())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_system_clients)
        .service(get_system_client)
        .service(create_system_client)
        .service(update_system_client)
        .service(regenerate_system_client_token)
        .service(list_system_client_tokens)
        .service(create_system_client_token)
        .service(revoke_system_client_token);
}
