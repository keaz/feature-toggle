//! SSO administration (providers, group mappings, settings) and the public list of
//! enabled providers. Every admin route requires a system admin; the client secret is
//! write-only and never appears in a response.

use actix_web::{
    HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, patch, post, put, web,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::entity::{SsoGroupMapping, SsoProvider};
use crate::database::role::role_repository;
use crate::database::sso_group_mapping::sso_group_mapping_repository_tx;
use crate::database::sso_provider::{sso_provider_repository, sso_provider_repository_tx};
use crate::database::sso_settings::{sso_settings_repository, sso_settings_repository_tx};
use crate::database::team::team_repository_tx;
use crate::logic::ActorContext;
use crate::logic::sso_provider::{
    ProviderFields, SsoSecrets, client_secret_from_env, test_discovery,
};
use crate::logic::sso_provider_tx::{
    MappingInput, create_provider_in_tx, delete_provider_in_tx, replace_mappings_in_tx,
    set_settings_in_tx, update_provider_in_tx,
};
use crate::rest::error::{ErrorResponse, RestError};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoProviderResponse {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    /// Whether an encrypted client secret is stored. The secret itself is never returned.
    pub has_client_secret: bool,
    /// Whether `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET` overrides the stored secret.
    pub client_secret_from_env: bool,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub allowed_email_domains: Vec<String>,
    pub jit_provisioning: bool,
    pub allow_email_linking: bool,
    /// `authoritative`, `additive` or `off`.
    pub role_sync_mode: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<SsoProvider> for SsoProviderResponse {
    fn from(p: SsoProvider) -> Self {
        Self {
            id: p.id.to_string(),
            has_client_secret: p.client_secret_enc.is_some(),
            client_secret_from_env: client_secret_from_env(&p.slug).is_some(),
            slug: p.slug,
            display_name: p.display_name,
            issuer_url: p.issuer_url,
            client_id: p.client_id,
            scopes: p.scopes,
            groups_claim: p.groups_claim,
            allowed_email_domains: p.allowed_email_domains,
            jit_provisioning: p.jit_provisioning,
            allow_email_linking: p.allow_email_linking,
            role_sync_mode: p.role_sync_mode,
            enabled: p.enabled,
            created_at: p.created_at.to_rfc3339(),
            updated_at: p.updated_at.to_rfc3339(),
        }
    }
}

/// Provider fields for create (`slug`, `displayName`, `issuerUrl`, `clientId` required)
/// and, with every field optional, for PATCH.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoProviderInput {
    pub slug: Option<String>,
    pub display_name: Option<String>,
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    /// Write-only. Omitted on PATCH = unchanged; `""` = clear.
    pub client_secret: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub groups_claim: Option<String>,
    pub allowed_email_domains: Option<Vec<String>>,
    pub jit_provisioning: Option<bool>,
    pub allow_email_linking: Option<bool>,
    pub role_sync_mode: Option<String>,
    pub enabled: Option<bool>,
}

impl SsoProviderInput {
    fn into_parts(self) -> (ProviderFields, Option<String>) {
        (
            ProviderFields {
                slug: self.slug,
                display_name: self.display_name,
                issuer_url: self.issuer_url,
                client_id: self.client_id,
                scopes: self.scopes,
                groups_claim: self.groups_claim,
                allowed_email_domains: self.allowed_email_domains,
                jit_provisioning: self.jit_provisioning,
                allow_email_linking: self.allow_email_linking,
                role_sync_mode: self.role_sync_mode,
                enabled: self.enabled,
            },
            self.client_secret,
        )
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoProviderTestResponse {
    pub ok: bool,
    pub issuer: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoGroupMappingResponse {
    pub id: String,
    pub group_value: String,
    /// `role`, `team` or `admin`.
    pub target_type: String,
    /// Null only for `admin`.
    pub target_id: Option<String>,
}

impl From<SsoGroupMapping> for SsoGroupMappingResponse {
    fn from(m: SsoGroupMapping) -> Self {
        Self {
            id: m.id.to_string(),
            group_value: m.group_value,
            target_type: m.target_type,
            target_id: m.target_id.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoGroupMappingInput {
    pub group_value: String,
    pub target_type: String,
    pub target_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SsoSettingsBody {
    pub enforce_sso: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicSsoProviderResponse {
    pub slug: String,
    pub display_name: String,
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

/// Admin gate for every route in this module (also enforced by `RoutePolicy`).
fn require_admin(req: &HttpRequest) -> Result<ActorContext, RestError> {
    let jwt = jwt_user(req)?;
    if !jwt.is_admin {
        return Err(RestError::forbidden("Admin access required"));
    }
    Ok(ActorContext::new(jwt.id, jwt.username))
}

fn parse_provider_id(value: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input("invalid provider id"))
}

async fn begin(
    pool: &sqlx::PgPool,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, RestError> {
    pool.begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))
}

async fn commit(tx: sqlx::Transaction<'static, sqlx::Postgres>) -> Result<(), RestError> {
    tx.commit()
        .await
        .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))
}

#[utoipa::path(
    get,
    path = "/api/v1/sso/providers",
    responses(
        (status = 200, description = "SSO providers", body = [SsoProviderResponse]),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[get("/sso/providers")]
pub(crate) async fn list_sso_providers(
    db_pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    let providers = sso_provider_repository(db_pool.get_ref().clone())
        .list_providers()
        .await?;
    Ok(HttpResponse::Ok().json(
        providers
            .into_iter()
            .map(SsoProviderResponse::from)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/sso/providers",
    request_body = SsoProviderInput,
    responses(
        (status = 201, description = "Provider created", body = SsoProviderResponse),
        (status = 400, description = "Invalid input or encryption_key_missing", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 409, description = "Slug already exists", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[post("/sso/providers")]
pub(crate) async fn create_sso_provider(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    secrets: web::Data<SsoSecrets>,
    req: HttpRequest,
    payload: web::Json<SsoProviderInput>,
) -> Result<impl Responder, RestError> {
    let actor = require_admin(&req)?;
    let (fields, client_secret) = payload.into_inner().into_parts();
    let repo = sso_provider_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(&db_pool).await?;
    let result = create_provider_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        &secrets,
        fields,
        client_secret,
        Some(actor),
    )
    .await;
    match result {
        Ok(provider) => {
            commit(tx).await?;
            Ok(HttpResponse::Created().json(SsoProviderResponse::from(provider)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/sso/providers/{id}",
    params(("id" = String, Path, description = "Provider ID")),
    responses(
        (status = 200, description = "Provider", body = SsoProviderResponse),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[get("/sso/providers/{id}")]
pub(crate) async fn get_sso_provider(
    db_pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    let id = parse_provider_id(&id)?;
    let provider = sso_provider_repository(db_pool.get_ref().clone())
        .get_provider_by_id(id)
        .await?;
    Ok(HttpResponse::Ok().json(SsoProviderResponse::from(provider)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/sso/providers/{id}",
    params(("id" = String, Path, description = "Provider ID")),
    request_body = SsoProviderInput,
    responses(
        (status = 200, description = "Provider updated", body = SsoProviderResponse),
        (status = 400, description = "Invalid input or encryption_key_missing", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Slug already exists", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[patch("/sso/providers/{id}")]
pub(crate) async fn update_sso_provider(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    secrets: web::Data<SsoSecrets>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<SsoProviderInput>,
) -> Result<impl Responder, RestError> {
    let actor = require_admin(&req)?;
    let id = parse_provider_id(&id)?;
    let (fields, client_secret) = payload.into_inner().into_parts();
    let repo = sso_provider_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(&db_pool).await?;
    let result = update_provider_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        &secrets,
        id,
        fields,
        client_secret,
        Some(actor),
    )
    .await;
    match result {
        Ok(provider) => {
            commit(tx).await?;
            Ok(HttpResponse::Ok().json(SsoProviderResponse::from(provider)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/sso/providers/{id}",
    params(("id" = String, Path, description = "Provider ID")),
    responses(
        (status = 204, description = "Provider, its identities and mappings deleted; users remain"),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[delete("/sso/providers/{id}")]
pub(crate) async fn delete_sso_provider(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let actor = require_admin(&req)?;
    let id = parse_provider_id(&id)?;
    let repo = sso_provider_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(&db_pool).await?;
    let result = delete_provider_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        id,
        Some(actor),
    )
    .await;
    match result {
        Ok(()) => {
            commit(tx).await?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/sso/providers/{id}/test",
    params(("id" = String, Path, description = "Provider ID")),
    responses(
        (status = 200, description = "Discovery and JWKS check; IdP problems are reported in `error`", body = SsoProviderTestResponse),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[post("/sso/providers/{id}/test")]
pub(crate) async fn test_sso_provider(
    db_pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    let id = parse_provider_id(&id)?;
    let provider = sso_provider_repository(db_pool.get_ref().clone())
        .get_provider_by_id(id)
        .await?;
    let result = test_discovery(&provider.issuer_url).await;
    Ok(HttpResponse::Ok().json(SsoProviderTestResponse {
        ok: result.ok,
        issuer: result.issuer,
        authorization_endpoint: result.authorization_endpoint,
        error: result.error,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/sso/providers/{id}/mappings",
    params(("id" = String, Path, description = "Provider ID")),
    responses(
        (status = 200, description = "Group mappings", body = [SsoGroupMappingResponse]),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[get("/sso/providers/{id}/mappings")]
pub(crate) async fn list_sso_mappings(
    db_pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    let id = parse_provider_id(&id)?;
    sso_provider_repository(db_pool.get_ref().clone())
        .get_provider_by_id(id)
        .await?;
    let mappings =
        crate::database::sso_group_mapping::sso_group_mapping_repository(db_pool.get_ref().clone())
            .list_mappings(id)
            .await?;
    Ok(HttpResponse::Ok().json(
        mappings
            .into_iter()
            .map(SsoGroupMappingResponse::from)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/sso/providers/{id}/mappings",
    params(("id" = String, Path, description = "Provider ID")),
    request_body = [SsoGroupMappingInput],
    responses(
        (status = 200, description = "Mappings replaced", body = [SsoGroupMappingResponse]),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[put("/sso/providers/{id}/mappings")]
pub(crate) async fn replace_sso_mappings(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<Vec<SsoGroupMappingInput>>,
) -> Result<impl Responder, RestError> {
    let actor = require_admin(&req)?;
    let id = parse_provider_id(&id)?;
    let inputs = payload
        .into_inner()
        .into_iter()
        .map(|m| {
            let target_id = match m.target_id.as_deref() {
                Some(value) => Some(
                    Uuid::parse_str(value)
                        .map_err(|_| RestError::invalid_input("invalid targetId"))?,
                ),
                None => None,
            };
            Ok(MappingInput {
                group_value: m.group_value,
                target_type: m.target_type,
                target_id,
            })
        })
        .collect::<Result<Vec<_>, RestError>>()?;

    let pool = db_pool.get_ref().clone();
    let provider_repo = sso_provider_repository_tx(pool.clone());
    let mapping_repo = sso_group_mapping_repository_tx(pool.clone());
    let team_repo = team_repository_tx(pool.clone());
    let roles = role_repository(pool);
    let mut tx = begin(&db_pool).await?;
    let result = replace_mappings_in_tx(
        &mut tx,
        &provider_repo,
        &mapping_repo,
        roles.as_ref(),
        &team_repo,
        activity_repo.as_ref().as_ref(),
        id,
        inputs,
        Some(actor),
    )
    .await;
    match result {
        Ok(mappings) => {
            commit(tx).await?;
            Ok(HttpResponse::Ok().json(
                mappings
                    .into_iter()
                    .map(SsoGroupMappingResponse::from)
                    .collect::<Vec<_>>(),
            ))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/sso/settings",
    responses(
        (status = 200, description = "SSO settings", body = SsoSettingsBody),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[get("/sso/settings")]
pub(crate) async fn get_sso_settings(
    db_pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    require_admin(&req)?;
    let enforce_sso = sso_settings_repository(db_pool.get_ref().clone())
        .get_enforce_sso()
        .await?;
    Ok(HttpResponse::Ok().json(SsoSettingsBody { enforce_sso }))
}

#[utoipa::path(
    put,
    path = "/api/v1/sso/settings",
    request_body = SsoSettingsBody,
    responses(
        (status = 200, description = "SSO settings updated", body = SsoSettingsBody),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse)
    ),
    tag = "SSO"
)]
#[put("/sso/settings")]
pub(crate) async fn update_sso_settings(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    payload: web::Json<SsoSettingsBody>,
) -> Result<impl Responder, RestError> {
    let actor = require_admin(&req)?;
    let repo = sso_settings_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(&db_pool).await?;
    let result = set_settings_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        payload.enforce_sso,
        Some(actor),
    )
    .await;
    match result {
        Ok(enforce_sso) => {
            commit(tx).await?;
            Ok(HttpResponse::Ok().json(SsoSettingsBody { enforce_sso }))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/sso/providers",
    responses(
        (status = 200, description = "Enabled SSO providers", body = [PublicSsoProviderResponse])
    ),
    security(()),
    tag = "Auth"
)]
#[get("/auth/sso/providers")]
pub(crate) async fn list_public_sso_providers(
    db_pool: web::Data<sqlx::PgPool>,
) -> Result<impl Responder, RestError> {
    let providers = sso_provider_repository(db_pool.get_ref().clone())
        .list_enabled_providers()
        .await?;
    Ok(HttpResponse::Ok().json(
        providers
            .into_iter()
            .map(|p| PublicSsoProviderResponse {
                slug: p.slug,
                display_name: p.display_name,
            })
            .collect::<Vec<_>>(),
    ))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_public_sso_providers)
        .service(list_sso_providers)
        .service(create_sso_provider)
        .service(get_sso_provider)
        .service(update_sso_provider)
        .service(delete_sso_provider)
        .service(test_sso_provider)
        .service(list_sso_mappings)
        .service(replace_sso_mappings)
        .service(get_sso_settings)
        .service(update_sso_settings);
}
