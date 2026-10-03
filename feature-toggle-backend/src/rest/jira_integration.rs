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
use crate::database::activity_log::ActivityLogRepository;
use crate::database::entity::{JiraIntegrationRow, JiraStatusRuleRow};
use crate::database::jira_integration::{
    JiraIntegrationRepository, jira_integration_repository_tx,
};
use crate::logic::ActorContext;
use crate::logic::jira_integration::JiraStatusRuleInput;
use crate::logic::jira_integration_tx::{
    JiraIntegrationInput, JiraIntegrationPatch, JiraIntegrationWithSecret,
    create_jira_integration_in_tx, delete_jira_integration_in_tx, replace_jira_status_rules_in_tx,
    rotate_jira_integration_secret_in_tx, update_jira_integration_in_tx,
};
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    let updated = update_jira_integration_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        id,
        patch,
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

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_jira_integrations)
        .service(create_jira_integration)
        .service(get_jira_integration)
        .service(update_jira_integration)
        .service(delete_jira_integration)
        .service(rotate_jira_integration_secret)
        .service(list_jira_status_rules)
        .service(replace_jira_status_rules);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::activity_log_repository;
    use crate::database::jira_integration::jira_integration_repository;
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
            let app = test::init_service(
                App::new()
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
}
