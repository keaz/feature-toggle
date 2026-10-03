//! Jira integrations of a team: configuration, status rules and the inbound secret.
//! Only a system admin or a `Team Admin` of the team reaches these handlers
//! (`PolicyAction::ManageJiraIntegrations`, enforced by `JwtGuard`; system clients
//! are denied).

use std::collections::BTreeMap;

use actix_web::{
    HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, patch, post, put, web,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::config::{JiraConfig, JiraUiBaseUrl};
use crate::database::activity_log::ActivityLogRepository;
use crate::database::entity::{JiraIntegrationRow, JiraStatusRuleRow};
use crate::database::jira_integration::{
    JiraIntegrationRepository, jira_integration_repository_tx,
};
use crate::database::jira_outbound_job::jira_outbound_job_repository_tx;
use crate::logic::ActorContext;
use crate::logic::jira_client::client_for;
use crate::logic::jira_integration::JiraStatusRuleInput;
use crate::logic::jira_integration_tx::{
    JiraIntegrationInput, JiraIntegrationPatch, JiraIntegrationWithSecret, WritebackPatch,
    create_jira_integration_in_tx, delete_jira_integration_in_tx,
    generate_native_webhook_secret_in_tx, remove_native_webhook_secret_in_tx,
    replace_jira_status_rules_in_tx, resume_jira_writeback_in_tx,
    rotate_jira_integration_secret_in_tx, update_jira_integration_in_tx,
    update_jira_writeback_in_tx,
};
use crate::logic::secret_box;
use crate::rest::error::RestError;

/// A Jira integration. Never contains the secret or its hash.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraIntegrationResponse {
    pub id: String,
    pub team_id: String,
    pub name: String,
    /// Base URL of the Jira site, for links in the UI.
    pub jira_base_url: Option<String>,
    /// Jira field naming the environment: `labels`, `customfield_<id>` or a system field.
    pub environment_field: String,
    /// Jira value -> FluxGate environment id. Keys are compared ignoring case.
    pub environment_aliases: BTreeMap<String, String>,
    /// Environments where Jira is a trusted approver.
    pub jira_approved_environment_ids: Vec<String>,
    /// Optional Jira field holding FluxGate feature keys.
    pub feature_key_field: Option<String>,
    /// Shadow user that requests and executes changes for the integration.
    pub actor_user_id: String,
    pub enabled: bool,
    /// Write-back to Jira. Never contains the credential.
    pub writeback: JiraWritebackResponse,
    /// Whether a native Jira webhook secret is stored. The secret is never returned.
    pub has_native_webhook_secret: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Write-back settings of an integration.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraWritebackResponse {
    pub enabled: bool,
    /// Post comments to linked issues.
    pub comments: bool,
    /// Keep a FluxGate remote link on linked issues.
    pub remote_link: bool,
    /// `cloud_basic` or `dc_pat`.
    pub auth_kind: Option<String>,
    /// Account email for `cloud_basic`.
    pub account_email: Option<String>,
    /// Whether a credential is stored. The credential is never returned.
    pub has_credential: bool,
    /// Why the sender paused, for example `Jira returned 401 at ...`.
    pub paused_reason: Option<String>,
}

/// The whole write-back configuration.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJiraWritebackRequest {
    pub enabled: bool,
    pub comments: bool,
    pub remote_link: bool,
    /// `cloud_basic` or `dc_pat`. Omitted keeps the stored value; blank clears it.
    pub auth_kind: Option<String>,
    /// Required for `cloud_basic`. Omitted keeps the stored value; blank clears it.
    pub account_email: Option<String>,
    /// API token (Cloud) or personal access token (Data Center), 1 to 1000
    /// characters. Omitted keeps the stored one; blank clears it (only with
    /// `enabled = false`). Saving one clears the paused reason.
    pub credential: Option<String>,
}

/// Result of a test connection. HTTP status is 200 whatever Jira answered.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JiraWritebackTestResponse {
    pub ok: bool,
    /// Status Jira returned; null when Jira was not reached.
    pub status: Option<u16>,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JiraIntegrationsResponse {
    pub items: Vec<JiraIntegrationResponse>,
}

/// Returned on create and on rotate only: the secret is shown once.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JiraIntegrationWithSecretResponse {
    pub integration: JiraIntegrationResponse,
    /// Inbound secret (Jira sends it as `Authorization: Bearer <secret>`). Store it now:
    /// FluxGate keeps only its SHA-256.
    pub secret: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateJiraIntegrationRequest {
    /// 1 to 100 characters, unique in the team.
    pub name: String,
    /// Optional `http`/`https` URL.
    pub jira_base_url: Option<String>,
    /// `labels`, `customfield_<digits>` or a system field name made of `a-z` and `_`.
    pub environment_field: String,
    /// Jira value -> environment id of the team.
    #[serde(default)]
    pub environment_aliases: BTreeMap<String, String>,
    /// Environment ids of the team where Jira is a trusted approver.
    #[serde(default)]
    pub jira_approved_environment_ids: Vec<String>,
    /// Same format as `environmentField`; optional.
    pub feature_key_field: Option<String>,
    /// Defaults to `true`.
    pub enabled: Option<bool>,
}

/// Omitted fields keep their value. An empty `jiraBaseUrl` or `featureKeyField` clears it.
#[derive(Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJiraIntegrationRequest {
    pub name: Option<String>,
    pub jira_base_url: Option<String>,
    pub environment_field: Option<String>,
    pub environment_aliases: Option<BTreeMap<String, String>>,
    pub jira_approved_environment_ids: Option<Vec<String>>,
    pub feature_key_field: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraStatusRuleRequest {
    /// Jira status name, 1 to 100 characters; compared ignoring case and surrounding spaces.
    pub jira_status: String,
    /// One of `request`, `approve`, `deploy`, `rollback`.
    pub action: String,
    /// Optional, non-empty list of environment ids of the team. Omitted: every
    /// environment the issue names.
    pub environment_ids: Option<Vec<String>>,
    /// Defaults to `true`.
    pub enabled: Option<bool>,
}

/// The whole rule list; it replaces the stored rules. Rules run in list order.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplaceJiraStatusRulesRequest {
    pub rules: Vec<JiraStatusRuleRequest>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraStatusRuleResponse {
    pub id: String,
    pub jira_status: String,
    pub action: String,
    pub environment_ids: Option<Vec<String>>,
    pub enabled: bool,
    pub position: i32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JiraStatusRulesResponse {
    pub items: Vec<JiraStatusRuleResponse>,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor(req: &HttpRequest) -> Result<ActorContext, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .map(|user| ActorContext::new(user.id, user.username.clone()))
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn integration_not_found(id: Uuid) -> RestError {
    RestError::not_found(format!("Jira integration {id} not found"))
}

/// `Error::NotFound(id)` for the integration becomes a specific 404.
fn map_error(id: Uuid) -> impl Fn(crate::Error) -> RestError {
    move |err| match err {
        crate::Error::NotFound(missing) if missing == id => integration_not_found(id),
        other => RestError::from(other),
    }
}

fn map_integration(row: JiraIntegrationRow) -> JiraIntegrationResponse {
    JiraIntegrationResponse {
        id: row.id.to_string(),
        team_id: row.team_id.to_string(),
        name: row.name,
        jira_base_url: row.jira_base_url,
        environment_field: row.environment_field,
        environment_aliases: row
            .environment_aliases
            .0
            .into_iter()
            .map(|(alias, environment_id)| (alias, environment_id.to_string()))
            .collect(),
        jira_approved_environment_ids: row
            .jira_approved_environment_ids
            .iter()
            .map(Uuid::to_string)
            .collect(),
        feature_key_field: row.feature_key_field,
        actor_user_id: row.actor_user_id.to_string(),
        enabled: row.enabled,
        writeback: JiraWritebackResponse {
            enabled: row.writeback_enabled,
            comments: row.writeback_comments,
            remote_link: row.writeback_remote_link,
            auth_kind: row.jira_auth_kind,
            account_email: row.jira_account_email,
            has_credential: row.jira_credential_enc.is_some(),
            paused_reason: row.writeback_paused_reason,
        },
        has_native_webhook_secret: row.native_webhook_secret_enc.is_some(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn map_with_secret(created: JiraIntegrationWithSecret) -> JiraIntegrationWithSecretResponse {
    JiraIntegrationWithSecretResponse {
        integration: map_integration(created.integration),
        secret: created.secret,
    }
}

fn map_rules(rules: Vec<JiraStatusRuleRow>) -> JiraStatusRulesResponse {
    JiraStatusRulesResponse {
        items: rules
            .into_iter()
            .map(|rule| JiraStatusRuleResponse {
                id: rule.id.to_string(),
                jira_status: rule.jira_status,
                action: rule.action,
                environment_ids: rule
                    .environment_ids
                    .map(|ids| ids.iter().map(Uuid::to_string).collect()),
                enabled: rule.enabled,
                position: rule.position,
            })
            .collect(),
    }
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
    path = "/api/v1/teams/{team_id}/jira-integrations",
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Jira integrations of the team", body = JiraIntegrationsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[get("/teams/{team_id}/jira-integrations")]
pub(crate) async fn list_jira_integrations(
    repo: web::Data<Box<dyn JiraIntegrationRepository>>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_id = parse_uuid(&team_id, "team id")?;
    let integrations = repo.list_for_team(team_id).await?;
    Ok(HttpResponse::Ok().json(JiraIntegrationsResponse {
        items: integrations.into_iter().map(map_integration).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/jira-integrations",
    request_body = CreateJiraIntegrationRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 201, description = "Integration created; the secret is shown once", body = JiraIntegrationWithSecretResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Team not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "The team already has an integration with this name", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[post("/teams/{team_id}/jira-integrations")]
pub(crate) async fn create_jira_integration(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<CreateJiraIntegrationRequest>,
) -> Result<impl Responder, RestError> {
    let team_id = parse_uuid(&team_id, "team id")?;
    let actor = actor(&req)?;
    let payload = payload.into_inner();
    let input = JiraIntegrationInput {
        name: payload.name,
        jira_base_url: payload.jira_base_url,
        environment_field: payload.environment_field,
        environment_aliases: payload.environment_aliases,
        jira_approved_environment_ids: payload.jira_approved_environment_ids,
        feature_key_field: payload.feature_key_field,
        enabled: payload.enabled.unwrap_or(true),
    };

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let created = create_jira_integration_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        team_id,
        input,
        actor,
    )
    .await
    .map_err(|err| match err {
        crate::Error::NotFound(missing) if missing == team_id => {
            RestError::not_found(format!("Team {team_id} not found"))
        }
        other => RestError::from(other),
    })?;
    commit(tx).await?;

    Ok(HttpResponse::Created().json(map_with_secret(created)))
}

#[utoipa::path(
    get,
    path = "/api/v1/jira-integrations/{id}",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "The integration", body = JiraIntegrationResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[get("/jira-integrations/{id}")]
pub(crate) async fn get_jira_integration(
    repo: web::Data<Box<dyn JiraIntegrationRepository>>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let integration = repo
        .get(id)
        .await?
        .ok_or_else(|| integration_not_found(id))?;
    Ok(HttpResponse::Ok().json(map_integration(integration)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/jira-integrations/{id}",
    request_body = UpdateJiraIntegrationRequest,
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "Integration updated", body = JiraIntegrationResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "The team already has an integration with this name", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[patch("/jira-integrations/{id}")]
pub(crate) async fn update_jira_integration(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    jira_config: web::Data<JiraConfig>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<UpdateJiraIntegrationRequest>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;
    let payload = payload.into_inner();
    let patch = JiraIntegrationPatch {
        name: payload.name,
        jira_base_url: payload.jira_base_url,
        environment_field: payload.environment_field,
        environment_aliases: payload.environment_aliases,
        jira_approved_environment_ids: payload.jira_approved_environment_ids,
        feature_key_field: payload.feature_key_field,
        enabled: payload.enabled,
    };

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let outbound_repo = jira_outbound_job_repository_tx(db_pool.get_ref().clone());
    let updated = update_jira_integration_in_tx(
        &mut tx,
        &repo,
        &outbound_repo,
        activity_repo.as_ref().as_ref(),
        id,
        patch,
        jira_config.get_ref(),
        actor,
    )
    .await
    .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::Ok().json(map_integration(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/jira-integrations/{id}",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 204, description = "Integration and its rules deleted; its shadow user is disabled"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[delete("/jira-integrations/{id}")]
pub(crate) async fn delete_jira_integration(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    delete_jira_integration_in_tx(&mut tx, &repo, activity_repo.as_ref().as_ref(), id, actor)
        .await
        .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(
    post,
    path = "/api/v1/jira-integrations/{id}/rotate-secret",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "New secret; the old one stops working. Shown once", body = JiraIntegrationWithSecretResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[post("/jira-integrations/{id}/rotate-secret")]
pub(crate) async fn rotate_jira_integration_secret(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let rotated = rotate_jira_integration_secret_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        id,
        actor,
    )
    .await
    .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::Ok().json(map_with_secret(rotated)))
}

#[utoipa::path(
    get,
    path = "/api/v1/jira-integrations/{id}/rules",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "Status rules in run order", body = JiraStatusRulesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[get("/jira-integrations/{id}/rules")]
pub(crate) async fn list_jira_status_rules(
    repo: web::Data<Box<dyn JiraIntegrationRepository>>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    if repo.get(id).await?.is_none() {
        return Err(integration_not_found(id));
    }
    let rules = repo.list_rules(id).await?;
    Ok(HttpResponse::Ok().json(map_rules(rules)))
}

#[utoipa::path(
    put,
    path = "/api/v1/jira-integrations/{id}/rules",
    request_body = ReplaceJiraStatusRulesRequest,
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "Rules replaced", body = JiraStatusRulesResponse),
        (status = 400, description = "Invalid input or duplicate rule", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[put("/jira-integrations/{id}/rules")]
pub(crate) async fn replace_jira_status_rules(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<ReplaceJiraStatusRulesRequest>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;
    let rules = payload
        .into_inner()
        .rules
        .into_iter()
        .map(|rule| JiraStatusRuleInput {
            jira_status: rule.jira_status,
            action: rule.action,
            environment_ids: rule.environment_ids,
            enabled: rule.enabled.unwrap_or(true),
        })
        .collect();

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let stored = replace_jira_status_rules_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        id,
        rules,
        actor,
    )
    .await
    .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::Ok().json(map_rules(stored)))
}

#[utoipa::path(
    put,
    path = "/api/v1/jira-integrations/{id}/writeback",
    request_body = UpdateJiraWritebackRequest,
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "Write-back configuration saved", body = JiraIntegrationResponse),
        (status = 400, description = "Invalid input, or FLUXGATE_ENCRYPTION_KEY is not set (`encryption_key_missing`)", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[put("/jira-integrations/{id}/writeback")]
pub(crate) async fn update_jira_writeback(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    jira_config: web::Data<JiraConfig>,
    ui_base_url: web::Data<JiraUiBaseUrl>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<UpdateJiraWritebackRequest>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;
    let payload = payload.into_inner();
    let sends_credential = payload
        .credential
        .as_deref()
        .is_some_and(|credential| !credential.trim().is_empty());
    if sends_credential && !secret_box::is_configured() {
        return Err(RestError::encryption_key_missing());
    }
    let patch = WritebackPatch {
        enabled: payload.enabled,
        comments: payload.comments,
        remote_link: payload.remote_link,
        auth_kind: payload.auth_kind,
        account_email: payload.account_email,
        credential: payload.credential,
    };

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let outbound_repo = jira_outbound_job_repository_tx(db_pool.get_ref().clone());
    let updated = update_jira_writeback_in_tx(
        &mut tx,
        &repo,
        &outbound_repo,
        activity_repo.as_ref().as_ref(),
        id,
        patch,
        jira_config.get_ref(),
        ui_base_url.0.as_deref(),
        actor,
    )
    .await
    .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::Ok().json(map_integration(updated)))
}

#[utoipa::path(
    post,
    path = "/api/v1/jira-integrations/{id}/writeback/test",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "Outcome of `GET /rest/api/{v}/myself` with the stored credential, also when Jira answers with an error", body = JiraWritebackTestResponse),
        (status = 400, description = "Write-back is not configured, or FLUXGATE_ENCRYPTION_KEY is not set", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[post("/jira-integrations/{id}/writeback/test")]
pub(crate) async fn test_jira_writeback(
    repo: web::Data<Box<dyn JiraIntegrationRepository>>,
    jira_config: web::Data<JiraConfig>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let row = repo
        .get(id)
        .await?
        .ok_or_else(|| integration_not_found(id))?;
    if row.jira_credential_enc.is_some() && !secret_box::is_configured() {
        return Err(RestError::encryption_key_missing());
    }
    let client = client_for(&row, jira_config.get_ref())?
        .ok_or_else(|| RestError::invalid_input("write-back is not configured"))?;
    let result = match client.myself_display_name().await {
        Ok((response, name)) if (200..300).contains(&response.status) => {
            JiraWritebackTestResponse {
                ok: true,
                status: Some(response.status),
                message: match name {
                    Some(name) => format!("connected as {name}"),
                    None => "connected".to_string(),
                },
            }
        }
        Ok((response, _)) => JiraWritebackTestResponse {
            ok: false,
            status: Some(response.status),
            message: format!("Jira returned {}", response.status),
        },
        Err(err) => JiraWritebackTestResponse {
            ok: false,
            status: None,
            message: format!("could not reach Jira: {err}"),
        },
    };
    Ok(HttpResponse::Ok().json(result))
}

#[utoipa::path(
    post,
    path = "/api/v1/jira-integrations/{id}/writeback/resume",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "Paused reason cleared", body = JiraIntegrationResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[post("/jira-integrations/{id}/writeback/resume")]
pub(crate) async fn resume_jira_writeback(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let updated =
        resume_jira_writeback_in_tx(&mut tx, &repo, activity_repo.as_ref().as_ref(), id, actor)
            .await
            .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::Ok().json(map_integration(updated)))
}

/// Returned once when a native webhook secret is generated or rotated.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JiraNativeWebhookSecretResponse {
    /// Paste it into the Jira webhook's "Secret" field. Jira then signs each delivery
    /// (`X-Hub-Signature: sha256=<hex>`). Store it now: FluxGate cannot show it again.
    pub secret: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/jira-integrations/{id}/native-webhook-secret",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 200, description = "New native webhook secret (a second call rotates it). Shown once", body = JiraNativeWebhookSecretResponse),
        (status = 400, description = "FLUXGATE_ENCRYPTION_KEY is not set (`encryption_key_missing`)", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[post("/jira-integrations/{id}/native-webhook-secret")]
pub(crate) async fn generate_jira_native_webhook_secret(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;
    if !secret_box::is_configured() {
        return Err(RestError::encryption_key_missing());
    }

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    let secret = generate_native_webhook_secret_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        id,
        actor,
    )
    .await
    .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::Ok().json(JiraNativeWebhookSecretResponse { secret }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/jira-integrations/{id}/native-webhook-secret",
    params(("id" = String, Path, description = "Jira integration ID")),
    responses(
        (status = 204, description = "Native webhook secret removed; signed deliveries are rejected"),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[delete("/jira-integrations/{id}/native-webhook-secret")]
pub(crate) async fn remove_jira_native_webhook_secret(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let id = parse_uuid(&id, "integration id")?;
    let actor = actor(&req)?;

    let repo = jira_integration_repository_tx(db_pool.get_ref().clone());
    let mut tx = begin(db_pool.get_ref()).await?;
    remove_native_webhook_secret_in_tx(&mut tx, &repo, activity_repo.as_ref().as_ref(), id, actor)
        .await
        .map_err(map_error(id))?;
    commit(tx).await?;

    Ok(HttpResponse::NoContent().finish())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_jira_integrations)
        .service(create_jira_integration)
        .service(get_jira_integration)
        .service(update_jira_integration)
        .service(delete_jira_integration)
        .service(rotate_jira_integration_secret)
        .service(generate_jira_native_webhook_secret)
        .service(remove_jira_native_webhook_secret)
        .service(list_jira_status_rules)
        .service(replace_jira_status_rules)
        .service(update_jira_writeback)
        .service(test_jira_writeback)
        .service(resume_jira_writeback);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{JiraConfig, JiraUiBaseUrl};
    use crate::database::activity_log::activity_log_repository;
    use crate::database::jira_integration::jira_integration_repository;
    use crate::logic::secret_box;
    use actix_web::{App, http::StatusCode, test};
    use serial_test::serial;
    use sqlx::postgres::PgPoolOptions;

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    /// A team with two environments and an admin user. Authorization is the
    /// guard's job (`logic::policy` tests); handlers trust the `JwtUser`.
    struct Fixture {
        pool: sqlx::PgPool,
        team_id: Uuid,
        qa: Uuid,
        prod: Uuid,
        admin_id: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let pool = test_pool().await;
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira')")
                .bind(team_id)
                .bind(format!("jira-rest-{team_id}"))
                .execute(&pool)
                .await
                .expect("insert team");
            let mut envs = Vec::new();
            for name in ["QA", "Production"] {
                let id = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO environments (id, name, active, team_id, environment_type) \
                     VALUES ($1, $2, TRUE, $3, 'Development')",
                )
                .bind(id)
                .bind(name)
                .bind(team_id)
                .execute(&pool)
                .await
                .expect("insert environment");
                envs.push(id);
            }
            let admin_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin) \
                 VALUES ($1, $2, 'x', 'Jira', 'Admin', $3, TRUE)",
            )
            .bind(admin_id)
            .bind(format!("jira_rest_{admin_id}"))
            .bind(format!("jira_rest_{admin_id}@example.com"))
            .execute(&pool)
            .await
            .expect("insert admin");
            Self {
                pool,
                team_id,
                qa: envs[0],
                prod: envs[1],
                admin_id,
            }
        }

        fn admin(&self) -> JwtUser {
            JwtUser {
                id: self.admin_id,
                username: "jira-admin".to_string(),
                is_admin: true,
                roles: Vec::new(),
                team_id: None,
                token_hash: "hash".to_string(),
            }
        }

        async fn call(
            &self,
            method: &str,
            uri: &str,
            body: Option<serde_json::Value>,
        ) -> (StatusCode, serde_json::Value) {
            self.call_with(
                JiraConfig::default(),
                Some("https://flux.example.com"),
                method,
                uri,
                body,
            )
            .await
        }

        async fn call_with(
            &self,
            jira_config: JiraConfig,
            ui_base_url: Option<&str>,
            method: &str,
            uri: &str,
            body: Option<serde_json::Value>,
        ) -> (StatusCode, serde_json::Value) {
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(jira_config))
                    .app_data(web::Data::new(JiraUiBaseUrl(
                        ui_base_url.map(str::to_string),
                    )))
                    .app_data(web::Data::new(self.pool.clone()))
                    .app_data(web::Data::new(jira_integration_repository(
                        self.pool.clone(),
                    )))
                    .app_data(web::Data::new(activity_log_repository(self.pool.clone())))
                    .service(web::scope("/api/v1").configure(super::configure)),
            )
            .await;
            let builder = match method {
                "GET" => test::TestRequest::get(),
                "POST" => test::TestRequest::post(),
                "PUT" => test::TestRequest::put(),
                "PATCH" => test::TestRequest::patch(),
                _ => test::TestRequest::delete(),
            };
            let mut builder = builder.uri(&format!("/api/v1{uri}"));
            if let Some(body) = body {
                builder = builder.set_json(body);
            }
            let req = builder.to_request();
            req.extensions_mut().insert(self.admin());
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (status, serde_json::from_slice(&bytes).unwrap_or_default())
        }

        fn create_body(&self, name: &str) -> serde_json::Value {
            serde_json::json!({
                "name": name,
                "jiraBaseUrl": "https://acme.atlassian.net",
                "environmentField": "customfield_10042",
                "environmentAliases": {"Prod": self.prod.to_string()},
                "jiraApprovedEnvironmentIds": [self.qa.to_string()],
            })
        }

        async fn create(&self, name: &str) -> serde_json::Value {
            let (status, body) = self
                .call(
                    "POST",
                    &format!("/teams/{}/jira-integrations", self.team_id),
                    Some(self.create_body(name)),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            body
        }

        /// `extra_users`: shadow users of integrations the test deleted (kept for audit).
        async fn cleanup(self, extra_users: &[Uuid]) {
            let mut users: Vec<Uuid> = sqlx::query_scalar(
                "SELECT actor_user_id FROM jira_integrations WHERE team_id = $1",
            )
            .bind(self.team_id)
            .fetch_all(&self.pool)
            .await
            .expect("shadow users");
            users.extend_from_slice(extra_users);
            users.push(self.admin_id);
            sqlx::query("DELETE FROM teams WHERE id = $1")
                .bind(self.team_id)
                .execute(&self.pool)
                .await
                .expect("delete team");
            sqlx::query("DELETE FROM users WHERE id = ANY($1)")
                .bind(users)
                .execute(&self.pool)
                .await
                .expect("delete users");
        }
    }

    fn assert_no_secret_hash(body: &serde_json::Value) {
        let text = body.to_string();
        assert!(!text.contains("secretHash"), "{text}");
        assert!(!text.contains("secret_hash"), "{text}");
    }

    #[actix_web::test]
    async fn create_returns_201_with_the_secret_once() {
        let fixture = Fixture::new().await;
        let body = fixture.create(" Jira PROJ ").await;
        let integration = &body["integration"];
        let secret = body["secret"].as_str().expect("secret").to_string();
        assert_eq!(secret.len(), 43, "32 bytes base64url");
        assert_eq!(integration["teamId"], fixture.team_id.to_string());
        assert_eq!(integration["name"], "Jira PROJ");
        assert_eq!(integration["jiraBaseUrl"], "https://acme.atlassian.net");
        assert_eq!(integration["environmentField"], "customfield_10042");
        assert_eq!(
            integration["environmentAliases"],
            serde_json::json!({"Prod": fixture.prod.to_string()})
        );
        assert_eq!(
            integration["jiraApprovedEnvironmentIds"],
            serde_json::json!([fixture.qa.to_string()])
        );
        assert_eq!(integration["featureKeyField"], serde_json::Value::Null);
        assert_eq!(integration["enabled"], true);
        assert!(integration["actorUserId"].as_str().is_some());
        assert_no_secret_hash(&body);
        let id = integration["id"].as_str().expect("id").to_string();

        let (status, listed) = fixture
            .call(
                "GET",
                &format!("/teams/{}/jira-integrations", fixture.team_id),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(listed["items"][0]["id"], id);
        assert!(!listed.to_string().contains(&secret));
        assert_no_secret_hash(&listed);

        let (status, fetched) = fixture
            .call("GET", &format!("/jira-integrations/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{fetched}");
        assert_eq!(fetched["name"], "Jira PROJ");
        assert!(fetched.get("secret").is_none());
        assert_no_secret_hash(&fetched);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn create_rejects_invalid_input_duplicates_and_unknown_teams() {
        let fixture = Fixture::new().await;
        let uri = format!("/teams/{}/jira-integrations", fixture.team_id);
        let mut cases = Vec::new();
        for (field, value) in [
            ("name", serde_json::json!("  ")),
            ("environmentField", serde_json::json!("custom field")),
            ("jiraBaseUrl", serde_json::json!("ftp://jira")),
            ("featureKeyField", serde_json::json!("Feature-Key")),
            (
                "jiraApprovedEnvironmentIds",
                serde_json::json!([Uuid::new_v4().to_string()]),
            ),
            (
                "environmentAliases",
                serde_json::json!({"Prod": Uuid::new_v4().to_string()}),
            ),
        ] {
            let mut body = fixture.create_body("Jira");
            body[field] = value;
            cases.push(body);
        }
        for body in cases {
            let (status, response) = fixture.call("POST", &uri, Some(body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {response}");
        }

        fixture.create("Jira").await;
        let (status, body) = fixture
            .call("POST", &uri, Some(fixture.create_body("Jira")))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");

        let (status, body) = fixture
            .call(
                "POST",
                &format!("/teams/{}/jira-integrations", Uuid::new_v4()),
                Some(fixture.create_body("Elsewhere")),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    async fn patch_rotate_and_delete() {
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let first_secret = created["secret"].as_str().unwrap().to_string();
        let shadow_user =
            Uuid::parse_str(created["integration"]["actorUserId"].as_str().unwrap()).unwrap();

        let (status, patched) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({
                    "jiraBaseUrl": "",
                    "environmentField": "labels",
                    "featureKeyField": "customfield_10050",
                    "enabled": false,
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{patched}");
        assert_eq!(patched["name"], "Jira");
        assert_eq!(patched["jiraBaseUrl"], serde_json::Value::Null);
        assert_eq!(patched["environmentField"], "labels");
        assert_eq!(patched["featureKeyField"], "customfield_10050");
        assert_eq!(patched["enabled"], false);
        assert!(patched.get("secret").is_none());
        assert_no_secret_hash(&patched);

        let (status, body) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"environmentField": "bad field"})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let (status, rotated) = fixture
            .call(
                "POST",
                &format!("/jira-integrations/{id}/rotate-secret"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{rotated}");
        let second_secret = rotated["secret"].as_str().expect("secret");
        assert_ne!(second_secret, first_secret);
        assert_eq!(rotated["integration"]["id"], id);
        assert_no_secret_hash(&rotated);

        let (status, _) = fixture
            .call("DELETE", &format!("/jira-integrations/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        for (method, uri) in [
            ("GET", format!("/jira-integrations/{id}")),
            ("PATCH", format!("/jira-integrations/{id}")),
            ("DELETE", format!("/jira-integrations/{id}")),
            ("POST", format!("/jira-integrations/{id}/rotate-secret")),
            ("GET", format!("/jira-integrations/{id}/rules")),
            ("PUT", format!("/jira-integrations/{id}/rules")),
        ] {
            let body = match method {
                "PATCH" => Some(serde_json::json!({})),
                "PUT" => Some(serde_json::json!({"rules": []})),
                _ => None,
            };
            let (status, response) = fixture.call(method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {response}");
        }
        let (status, _) = fixture
            .call("GET", "/jira-integrations/not-a-uuid", None)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        fixture.cleanup(&[shadow_user]).await;
    }

    #[actix_web::test]
    async fn put_rules_replaces_the_list_in_order() {
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/rules");

        let (status, rules) = fixture
            .call(
                "PUT",
                &uri,
                Some(serde_json::json!({"rules": [
                    {"jiraStatus": " Ready for Release ", "action": "approve",
                     "environmentIds": [fixture.qa.to_string()]},
                    {"jiraStatus": "Done", "action": "deploy"},
                    {"jiraStatus": "Reopened", "action": "rollback", "enabled": false},
                ]})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{rules}");
        let items = rules["items"].as_array().expect("items");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["jiraStatus"], "Ready for Release");
        assert_eq!(items[0]["action"], "approve");
        assert_eq!(
            items[0]["environmentIds"],
            serde_json::json!([fixture.qa.to_string()])
        );
        assert_eq!(items[0]["position"], 0);
        assert_eq!(items[0]["enabled"], true);
        assert_eq!(items[1]["environmentIds"], serde_json::Value::Null);
        assert_eq!(items[2]["position"], 2);
        assert_eq!(items[2]["enabled"], false);

        let (status, listed) = fixture.call("GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        assert_eq!(listed, rules);

        for body in [
            serde_json::json!({"rules": [
                {"jiraStatus": "Done", "action": "deploy"},
                {"jiraStatus": "DONE", "action": "deploy"},
            ]}),
            serde_json::json!({"rules": [{"jiraStatus": "Done", "action": "ship"}]}),
            serde_json::json!({"rules": [{"jiraStatus": "Done", "action": "deploy",
                "environmentIds": [Uuid::new_v4().to_string()]}]}),
        ] {
            let (status, response) = fixture.call("PUT", &uri, Some(body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {response}");
        }
        let (_, unchanged) = fixture.call("GET", &uri, None).await;
        assert_eq!(unchanged["items"].as_array().map(Vec::len), Some(3));

        fixture.cleanup(&[]).await;
    }

    /// The encryption key is read once per process. Tests set a random one.
    pub(crate) fn ensure_encryption_key() {
        use base64::Engine;
        static SET: std::sync::Once = std::sync::Once::new();
        SET.call_once(|| {
            if std::env::var(secret_box::ENCRYPTION_KEY_ENV).is_err() {
                let mut key = Uuid::new_v4().as_bytes().to_vec();
                key.extend_from_slice(Uuid::new_v4().as_bytes());
                // SAFETY: runs once, before any test in this module reads the variable.
                unsafe {
                    std::env::set_var(
                        secret_box::ENCRYPTION_KEY_ENV,
                        base64::engine::general_purpose::STANDARD.encode(key),
                    )
                };
            }
        });
    }

    fn test_token() -> String {
        format!("tok-{}", Uuid::new_v4())
    }

    fn writeback_body(token: Option<&str>) -> serde_json::Value {
        let mut body = serde_json::json!({
            "enabled": true,
            "comments": true,
            "remoteLink": false,
            "authKind": "dc_pat",
        });
        if let Some(token) = token {
            body["credential"] = serde_json::json!(token);
        }
        body
    }

    async fn stored_credential(fixture: &Fixture, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT jira_credential_enc FROM jira_integrations WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&fixture.pool)
            .await
            .expect("credential column")
    }

    #[actix_web::test]
    #[serial]
    async fn writeback_credential_is_stored_encrypted_and_never_returned() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let token = test_token();

        let (status, body) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(writeback_body(Some(&token))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!body.to_string().contains(&token));
        assert_eq!(body["writeback"]["hasCredential"], true);
        assert_eq!(body["writeback"]["enabled"], true);
        assert_eq!(body["writeback"]["comments"], true);
        assert_eq!(body["writeback"]["remoteLink"], false);
        assert_eq!(body["writeback"]["authKind"], "dc_pat");
        assert_eq!(body["writeback"]["pausedReason"], serde_json::Value::Null);
        assert_eq!(body["hasNativeWebhookSecret"], false);
        assert!(!body.to_string().contains("Enc"));

        let sealed = stored_credential(&fixture, &id).await.expect("stored");
        assert_ne!(sealed, token);
        assert!(!sealed.contains(&token));
        assert_eq!(
            secret_box::decrypt_with_aad(&sealed, Uuid::parse_str(&id).unwrap().as_bytes())
                .unwrap(),
            token
        );

        let (status, fetched) = fixture
            .call("GET", &format!("/jira-integrations/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!fetched.to_string().contains(&token));
        assert_eq!(fetched["writeback"]["hasCredential"], true);

        // Omitting the credential keeps it; an empty one clears it only when disabled.
        let (status, kept) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(writeback_body(None)),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{kept}");
        assert_eq!(
            stored_credential(&fixture, &id).await.as_deref(),
            Some(sealed.as_str())
        );
        let mut clear = writeback_body(Some(""));
        let (status, body) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(clear.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        clear["enabled"] = serde_json::json!(false);
        let (status, body) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(clear),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["writeback"]["hasCredential"], false);
        assert_eq!(stored_credential(&fixture, &id).await, None);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn enabling_without_base_url_or_credential_is_400() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/writeback");

        let (status, _) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": ""})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = fixture
            .call("PUT", &uri, Some(writeback_body(Some(&test_token()))))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let (status, _) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": "https://acme.atlassian.net"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = fixture.call("PUT", &uri, Some(writeback_body(None))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let mut no_kind = writeback_body(Some(&test_token()));
        no_kind["authKind"] = serde_json::Value::Null;
        let (status, body) = fixture.call("PUT", &uri, Some(no_kind)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let mut bad_kind = writeback_body(Some(&test_token()));
        bad_kind["authKind"] = serde_json::json!("oauth");
        let (status, body) = fixture.call("PUT", &uri, Some(bad_kind)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = fixture
            .call("PUT", &uri, Some(writeback_body(Some(&"x".repeat(1001)))))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        // Disabled needs none of them.
        let (status, body) = fixture
            .call(
                "PUT",
                &uri,
                Some(serde_json::json!({"enabled": false, "comments": true, "remoteLink": true})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _) = fixture
            .call(
                "PUT",
                "/jira-integrations/not-a-uuid/writeback",
                Some(writeback_body(None)),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{}/writeback", Uuid::new_v4()),
                Some(writeback_body(None)),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn http_base_url_needs_allow_insecure_http() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/writeback");
        let (status, _) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": "http://jira.internal:8080"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);

        let body = writeback_body(Some(&test_token()));
        let (status, response) = fixture.call("PUT", &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        let allow = JiraConfig {
            allow_insecure_http: true,
            ui_base_url: None,
            ..JiraConfig::default()
        };
        let (status, response) = fixture
            .call_with(allow, None, "PUT", &uri, Some(body))
            .await;
        assert_eq!(status, StatusCode::OK, "{response}");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn cloud_needs_account_email() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/writeback");
        let mut body = writeback_body(Some(&test_token()));
        body["authKind"] = serde_json::json!("cloud_basic");

        let (status, response) = fixture.call("PUT", &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        body["accountEmail"] = serde_json::json!("not-an-email");
        let (status, response) = fixture.call("PUT", &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        body["accountEmail"] = serde_json::json!(format!("{}@example.com", "a".repeat(250)));
        let (status, response) = fixture.call("PUT", &uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        body["accountEmail"] = serde_json::json!(" jira@example.com ");
        let (status, response) = fixture.call("PUT", &uri, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["writeback"]["accountEmail"], "jira@example.com");
        assert_eq!(response["writeback"]["authKind"], "cloud_basic");

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn remote_link_needs_a_ui_base_url() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/writeback");
        let mut body = writeback_body(Some(&test_token()));
        body["remoteLink"] = serde_json::json!(true);

        let (status, response) = fixture
            .call_with(JiraConfig::default(), None, "PUT", &uri, Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert!(
            response
                .to_string()
                .contains("ui base URL is not configured")
        );
        let (status, response) = fixture.call("PUT", &uri, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["writeback"]["remoteLink"], true);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn saving_a_credential_clears_the_paused_reason() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/writeback");
        let pause = || async {
            sqlx::query(
                "UPDATE jira_integrations SET writeback_paused_reason = 'Jira returned 401' WHERE id = $1",
            )
            .bind(Uuid::parse_str(&id).unwrap())
            .execute(&fixture.pool)
            .await
            .expect("pause");
        };
        let (status, _) = fixture
            .call("PUT", &uri, Some(writeback_body(Some(&test_token()))))
            .await;
        assert_eq!(status, StatusCode::OK);

        pause().await;
        let (status, kept) = fixture.call("PUT", &uri, Some(writeback_body(None))).await;
        assert_eq!(status, StatusCode::OK, "{kept}");
        assert_eq!(kept["writeback"]["pausedReason"], "Jira returned 401");

        let (status, cleared) = fixture
            .call("PUT", &uri, Some(writeback_body(Some(&test_token()))))
            .await;
        assert_eq!(status, StatusCode::OK, "{cleared}");
        assert_eq!(
            cleared["writeback"]["pausedReason"],
            serde_json::Value::Null
        );

        pause().await;
        let (status, resumed) = fixture.call("POST", &format!("{uri}/resume"), None).await;
        assert_eq!(status, StatusCode::OK, "{resumed}");
        assert_eq!(
            resumed["writeback"]["pausedReason"],
            serde_json::Value::Null
        );
        assert_eq!(resumed["writeback"]["hasCredential"], true);
        let (status, _) = fixture
            .call(
                "POST",
                &format!("/jira-integrations/{}/writeback/resume", Uuid::new_v4()),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn test_connection_reports_jira_status() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let writeback = format!("/jira-integrations/{id}/writeback");
        let test_uri = format!("{writeback}/test");
        let insecure = || JiraConfig {
            allow_insecure_http: true,
            ui_base_url: None,
            ..JiraConfig::default()
        };

        // Not configured yet.
        let (status, body) = fixture.call("POST", &test_uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.to_string().contains("write-back is not configured"));

        let server = MockServer::start().await;
        let token = test_token();
        let (status, _) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": server.uri()})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = fixture
            .call_with(
                insecure(),
                None,
                "PUT",
                &writeback,
                Some(writeback_body(Some(&token))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        Mock::given(method("GET"))
            .and(path("/rest/api/2/myself"))
            .and(header("Authorization", format!("Bearer {token}").as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"displayName": "Ada"})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let (status, ok) = fixture
            .call_with(insecure(), None, "POST", &test_uri, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{ok}");
        assert_eq!(
            ok,
            serde_json::json!({"ok": true, "status": 200, "message": "connected as Ada"})
        );

        Mock::given(method("GET"))
            .and(path("/rest/api/2/myself"))
            .respond_with(ResponseTemplate::new(401).set_body_string(format!("bad {token}")))
            .mount(&server)
            .await;
        let (status, denied) = fixture
            .call_with(insecure(), None, "POST", &test_uri, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{denied}");
        assert_eq!(
            denied,
            serde_json::json!({"ok": false, "status": 401, "message": "Jira returned 401"})
        );

        // Nothing listens on the port: a transport error, still HTTP 200.
        let (status, patched) = fixture
            .call_with(
                insecure(),
                None,
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": "http://127.0.0.1:1"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        // A new host clears the stored credential: save it again.
        assert_eq!(patched["writeback"]["hasCredential"], false);
        let (status, body) = fixture
            .call_with(
                insecure(),
                None,
                "PUT",
                &writeback,
                Some(writeback_body(Some(&token))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, refused) = fixture.call("POST", &test_uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert!(refused.to_string().contains("Jira base URL must use https"));
        let (status, unreachable) = fixture
            .call_with(insecure(), None, "POST", &test_uri, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{unreachable}");
        assert_eq!(unreachable["ok"], false);
        assert_eq!(unreachable["status"], serde_json::Value::Null);
        assert_eq!(unreachable["message"], "could not reach Jira: connect");
        assert!(!unreachable.to_string().contains(&token));

        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn activity_row_lists_changed_fields_without_values() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let token = test_token();
        let account = format!("acct-{}@example.com", Uuid::new_v4());
        let mut body = writeback_body(Some(&token));
        body["authKind"] = serde_json::json!("cloud_basic");
        body["accountEmail"] = serde_json::json!(account);
        let (status, response) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        let sealed = stored_credential(&fixture, &id).await.expect("sealed");

        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT metadata FROM activity_log WHERE entity_id = $1 \
             AND activity_type = 'jira_integration_updated' ORDER BY created_at DESC",
        )
        .bind(&id)
        .fetch_all(&fixture.pool)
        .await
        .expect("activity rows");
        assert_eq!(rows.len(), 1);
        let metadata = &rows[0];
        let changed: Vec<&str> = metadata["changed_fields"]
            .as_array()
            .expect("changed_fields")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(
            changed,
            [
                "writeback_enabled",
                "writeback_remote_link",
                "jira_auth_kind",
                "jira_account_email",
                "jira_credential"
            ]
        );
        assert_eq!(metadata["has_credential"], true);
        let text = metadata.to_string();
        assert!(!text.contains(&token));
        assert!(!text.contains(&sealed));
        assert!(!text.contains(&account));

        fixture.cleanup(&[]).await;
    }

    async fn stored_native_secret(fixture: &Fixture, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT native_webhook_secret_enc FROM jira_integrations WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&fixture.pool)
            .await
            .expect("native secret column")
    }

    #[actix_web::test]
    #[serial]
    async fn native_webhook_secret_create_rotate_delete() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/native-webhook-secret");
        assert_eq!(created["integration"]["hasNativeWebhookSecret"], false);
        assert!(stored_native_secret(&fixture, &id).await.is_none());

        // Create: plaintext returned once, the column holds the sealed value.
        let (status, body) = fixture.call("POST", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let first = body["secret"].as_str().expect("secret").to_string();
        assert!(first.len() >= 32);
        let sealed = stored_native_secret(&fixture, &id).await.expect("sealed");
        assert_ne!(sealed, first);
        assert_eq!(
            secret_box::decrypt_with_aad(&sealed, Uuid::parse_str(&id).unwrap().as_bytes())
                .unwrap(),
            first
        );

        // GET shows the flag, never the value.
        let (status, got) = fixture
            .call("GET", &format!("/jira-integrations/{id}"), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got["hasNativeWebhookSecret"], true);
        assert!(!got.to_string().contains(&first));
        assert!(!got.to_string().contains(&sealed));

        // Rotate: a second call changes it.
        let (status, body) = fixture.call("POST", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let second = body["secret"].as_str().expect("secret").to_string();
        assert_ne!(second, first);
        let sealed2 = stored_native_secret(&fixture, &id).await.expect("sealed");
        assert_eq!(
            secret_box::decrypt_with_aad(&sealed2, Uuid::parse_str(&id).unwrap().as_bytes())
                .unwrap(),
            second
        );

        // The activity rows name the field and carry no value.
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT metadata FROM activity_log WHERE entity_id = $1 \
             AND activity_type = 'jira_integration_updated'",
        )
        .bind(&id)
        .fetch_all(&fixture.pool)
        .await
        .expect("activity rows");
        assert_eq!(rows.len(), 2);
        for metadata in &rows {
            assert_eq!(
                metadata["changed_fields"],
                serde_json::json!(["native_webhook_secret"])
            );
            let text = metadata.to_string();
            for value in [&first, &second, &sealed, &sealed2] {
                assert!(!text.contains(value.as_str()));
            }
        }

        // Delete clears it.
        let (status, _) = fixture.call("DELETE", &uri, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(stored_native_secret(&fixture, &id).await.is_none());
        let (_, got) = fixture
            .call("GET", &format!("/jira-integrations/{id}"), None)
            .await;
        assert_eq!(got["hasNativeWebhookSecret"], false);

        // Unknown integration.
        let missing = format!(
            "/jira-integrations/{}/native-webhook-secret",
            Uuid::new_v4()
        );
        let (status, _) = fixture.call("POST", &missing, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = fixture.call("DELETE", &missing, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        fixture.cleanup(&[]).await;
    }

    /// Write-back enabled on a Data Center integration at `base_url`.
    async fn enabled_integration(fixture: &Fixture, base_url: &str, token: &str) -> String {
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let (status, _) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": base_url})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(writeback_body(Some(token))),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        id
    }

    #[actix_web::test]
    #[serial]
    async fn patch_to_http_while_writeback_enabled_is_400() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let id = enabled_integration(&fixture, "https://jira.example.com", &test_token()).await;
        let uri = format!("/jira-integrations/{id}");
        let (status, body) = fixture
            .call(
                "PATCH",
                &uri,
                Some(serde_json::json!({"jiraBaseUrl": "http://jira.example.com"})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.to_string()
                .contains("jira base URL must be an https URL")
        );
        let allow = JiraConfig {
            allow_insecure_http: true,
            ui_base_url: None,
            ..JiraConfig::default()
        };
        let (status, body) = fixture
            .call_with(
                allow,
                None,
                "PATCH",
                &uri,
                Some(serde_json::json!({"jiraBaseUrl": "http://jira.example.com"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn patch_clearing_base_url_while_enabled_is_400() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let id = enabled_integration(&fixture, "https://jira.example.com", &test_token()).await;
        let (status, body) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": ""})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.to_string()
                .contains("jira base URL is required while write-back is enabled")
        );
        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn changing_jira_host_clears_the_credential_and_disables_writeback() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let token = test_token();
        let id = enabled_integration(&fixture, "https://jira.example.com", &token).await;
        let uri = format!("/jira-integrations/{id}");

        // Same origin (path and case differ): nothing is cleared.
        let (status, body) = fixture
            .call(
                "PATCH",
                &uri,
                Some(serde_json::json!({"jiraBaseUrl": "https://JIRA.example.com/jira"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["writeback"]["hasCredential"], true);
        assert_eq!(body["writeback"]["enabled"], true);

        let (status, body) = fixture
            .call(
                "PATCH",
                &uri,
                Some(serde_json::json!({"jiraBaseUrl": "https://other.example.com"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["writeback"]["hasCredential"], false);
        assert_eq!(body["writeback"]["enabled"], false);
        assert_eq!(stored_credential(&fixture, &id).await, None);

        let metadata: serde_json::Value = sqlx::query_scalar(
            "SELECT metadata FROM activity_log WHERE entity_id = $1 \
             AND activity_type = 'jira_integration_updated' ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&id)
        .fetch_one(&fixture.pool)
        .await
        .expect("activity row");
        let changed = metadata["changed_fields"].to_string();
        assert!(changed.contains("jira_base_url"), "{changed}");
        assert!(changed.contains("jira_credential"), "{changed}");
        assert!(changed.contains("writeback_enabled"), "{changed}");
        assert!(!metadata.to_string().contains(&token));
        fixture.cleanup(&[]).await;
    }

    async fn insert_job(fixture: &Fixture, integration: &str, status: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO jira_outbound_jobs (id, integration_id, issue_key, kind, dedupe_key, status) \
             VALUES ($1, $2, 'PROJ-1', 'comment', $3, $4)",
        )
        .bind(id)
        .bind(Uuid::parse_str(integration).unwrap())
        .bind(format!("rest-test:{id}"))
        .bind(status)
        .execute(&fixture.pool)
        .await
        .expect("insert job");
        id
    }

    async fn job_state(fixture: &Fixture, id: Uuid) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, last_error FROM jira_outbound_jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&fixture.pool)
            .await
            .expect("job state")
    }

    #[actix_web::test]
    #[serial]
    async fn writeback_off_cancels_pending_jobs() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let id = enabled_integration(&fixture, "https://jira.example.com", &test_token()).await;
        let pending = insert_job(&fixture, &id, "pending").await;
        let sent = insert_job(&fixture, &id, "sent").await;

        // Saving with write-back still on keeps the queue.
        let (status, body) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(writeback_body(None)),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(job_state(&fixture, pending).await.0, "pending");

        let mut off = writeback_body(None);
        off["enabled"] = serde_json::json!(false);
        let (status, body) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(off),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            job_state(&fixture, pending).await,
            ("dead".to_string(), Some("write-back disabled".to_string()))
        );
        assert_eq!(job_state(&fixture, sent).await.0, "sent");
        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn changing_jira_host_cancels_pending_jobs() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let id = enabled_integration(&fixture, "https://jira.example.com", &test_token()).await;
        let pending = insert_job(&fixture, &id, "pending").await;

        let (status, body) = fixture
            .call(
                "PATCH",
                &format!("/jira-integrations/{id}"),
                Some(serde_json::json!({"jiraBaseUrl": "https://other.example.com"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["writeback"]["enabled"], false);
        assert_eq!(
            job_state(&fixture, pending).await,
            ("dead".to_string(), Some("write-back disabled".to_string()))
        );
        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn put_does_not_clobber_a_concurrent_pause() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let id = enabled_integration(&fixture, "https://jira.example.com", &test_token()).await;
        sqlx::query(
            "UPDATE jira_integrations SET writeback_paused_reason = 'Jira returned 401' WHERE id = $1",
        )
        .bind(Uuid::parse_str(&id).unwrap())
        .execute(&fixture.pool)
        .await
        .expect("pause");
        let mut body = writeback_body(None);
        body["comments"] = serde_json::json!(false);
        let (status, response) = fixture
            .call(
                "PUT",
                &format!("/jira-integrations/{id}/writeback"),
                Some(body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["writeback"]["pausedReason"], "Jira returned 401");
        fixture.cleanup(&[]).await;
    }

    #[actix_web::test]
    #[serial]
    async fn credential_with_control_characters_is_400() {
        ensure_encryption_key();
        let fixture = Fixture::new().await;
        let created = fixture.create("Jira").await;
        let id = created["integration"]["id"].as_str().unwrap().to_string();
        let uri = format!("/jira-integrations/{id}/writeback");
        for bad in ["tok\r\nX-Evil: 1", "tok en", "tok\u{7f}", "tök"] {
            let (status, body) = fixture
                .call("PUT", &uri, Some(writeback_body(Some(bad))))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?} {body}");
            assert!(
                body.to_string()
                    .contains("credential contains invalid characters")
            );
        }
        fixture.cleanup(&[]).await;
    }
}
