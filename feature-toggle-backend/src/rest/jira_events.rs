//! Inbound Jira events (design §3.9, JI-15) and the event log.
//!
//! `POST /api/v1/integrations/jira/{integration_id}/events` is public in
//! `JwtGuard` (`middleware::is_public_jira_event_path`): Jira has no FluxGate
//! JWT. It is authenticated by the integration's secret instead, sent as
//! `Authorization: Bearer <secret>` or `X-FluxGate-Jira-Secret: <secret>`, or by a
//! native Jira webhook signature (`X-Hub-Signature: sha256=<hex>`, JI-45).
//! `GET /api/v1/jira-integrations/{id}/events` is the event log, behind the
//! same policy as the rest of `/jira-integrations/**`.

use actix_web::{HttpRequest, HttpResponse, Responder, get, web};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::entity::JiraIntegrationRow;
use crate::database::environment::EnvironmentRepository;
use crate::database::external_link::ExternalLinkRepository;
use crate::database::feature::FeatureRepository;
use crate::database::jira_event::{JiraEventRepository, JiraEventRow, NewJiraEvent};
use crate::database::jira_integration::JiraIntegrationRepository;
use crate::database::jira_outbound_job::{NewOutboundJob, OutboundKind};
use crate::logic::external_change::ExternalChangeLogic;
use crate::logic::jira_capture::event_comment_lines;
use crate::logic::jira_events::{JiraActor, delivery_hash, field_values, parse_event};
use crate::logic::jira_integration::hash_secret;
use crate::logic::jira_rules::{RuleEngine, RuleResult};
use crate::logic::jira_signature::{SIGNATURE_HEADER, signature_matches};
use crate::logic::secret_box;
use crate::rest::error::RestError;
use crate::rest::jira_inbound_limit::{Admission, JiraInboundLimiter};
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};

/// Largest accepted event body.
pub const MAX_EVENT_BODY_BYTES: usize = 1024 * 1024;
/// Header carrying the secret when `Authorization` cannot be set.
pub const SECRET_HEADER: &str = "X-FluxGate-Jira-Secret";
/// A repeated delivery within this window returns the stored result.
pub const DUPLICATE_WINDOW_MINUTES: i64 = 10;
/// The one 401 message: never says whether the integration exists.
const UNAUTHORIZED_MESSAGE: &str = "unknown integration or wrong secret";
pub const IGNORED_NO_STATUS_CHANGE: &str = "no status change";

/// What FluxGate did with an event. Also returned for a repeated delivery
/// (`duplicate: true`) with the first delivery's results.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraEventResponse {
    pub event_id: String,
    pub results: Vec<RuleResult>,
    /// Environment values on the issue that matched no environment.
    pub unknown_environments: Vec<String>,
    /// Keys from the integration's feature key field that matched no feature.
    pub unknown_features: Vec<String>,
    /// Set when the event was accepted but does nothing, e.g. "no status change".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored: Option<String>,
    pub duplicate: bool,
}

/// One stored event.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraEventLogItem {
    pub id: String,
    pub integration_id: String,
    pub received_at: DateTime<Utc>,
    pub issue_key: Option<String>,
    pub jira_status: Option<String>,
    pub jira_actor: Option<JiraActor>,
    pub results: Vec<RuleResult>,
    pub unknown_environments: Vec<String>,
    pub unknown_features: Vec<String>,
    pub ignored: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JiraEventsResponse {
    pub items: Vec<JiraEventLogItem>,
    pub meta: PageMeta,
}

fn stored_results(row: &JiraEventRow) -> Vec<RuleResult> {
    serde_json::from_value(row.results.clone()).unwrap_or_default()
}

fn response_from_row(row: &JiraEventRow, duplicate: bool) -> JiraEventResponse {
    JiraEventResponse {
        event_id: row.id.to_string(),
        results: stored_results(row),
        unknown_environments: row.unknown_environments.clone(),
        unknown_features: row.unknown_features.clone(),
        ignored: row.ignored.clone(),
        duplicate,
    }
}

fn map_event(row: JiraEventRow) -> JiraEventLogItem {
    JiraEventLogItem {
        id: row.id.to_string(),
        integration_id: row.integration_id.to_string(),
        received_at: row.received_at,
        issue_key: row.issue_key.clone(),
        jira_status: row.jira_status.clone(),
        jira_actor: row
            .jira_actor
            .clone()
            .and_then(|actor| serde_json::from_value(actor).ok()),
        results: stored_results(&row),
        unknown_environments: row.unknown_environments,
        unknown_features: row.unknown_features,
        ignored: row.ignored,
        error: row.error,
    }
}

/// The secret from `Authorization: Bearer` or [`SECRET_HEADER`].
fn presented_secret(req: &HttpRequest) -> Option<String> {
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    header("Authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .or_else(|| header(SECRET_HEADER))
        .map(str::to_string)
}

fn secret_matches(secret: &str, stored_hash: &str) -> bool {
    hash_secret(secret)
        .as_bytes()
        .ct_eq(stored_hash.as_bytes())
        .into()
}

/// Integrations whose native secret failed to decrypt, so the warning is logged once.
static DECRYPT_WARNED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<Uuid>>> =
    std::sync::LazyLock::new(Default::default);

/// True when `X-Hub-Signature` is a valid HMAC of the raw body under the
/// integration's native webhook secret. No stored secret, or one that cannot be
/// decrypted (the key changed), is a failed check.
fn native_signature_ok(req: &HttpRequest, integration: &JiraIntegrationRow, body: &[u8]) -> bool {
    let Some(signature) = req
        .headers()
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(sealed) = integration.native_webhook_secret_enc.as_deref() else {
        return false;
    };
    match secret_box::decrypt_with_aad(sealed, integration.id.as_bytes()) {
        Ok(secret) => signature_matches(&secret, body, signature),
        Err(_) => {
            let first = DECRYPT_WARNED
                .lock()
                .map(|mut warned| warned.insert(integration.id))
                .unwrap_or(false);
            if first {
                log::warn!(
                    "Jira integration {}: native webhook secret cannot be decrypted",
                    integration.id
                );
            }
            false
        }
    }
}

/// Receives one Jira event. The body is the Jira webhook payload or an
/// Automation "Issue data (Jira format)" body.
#[utoipa::path(
    post,
    path = "/api/v1/integrations/jira/{integration_id}/events",
    request_body(content = Object, description = "Jira webhook or Automation issue body (Jira format), at most 1 MiB"),
    params(
        ("integration_id" = String, Path, description = "Jira integration ID"),
        ("X-FluxGate-Jira-Secret" = Option<String>, Header, description = "Integration secret, when `Authorization: Bearer <secret>` is not used"),
        ("X-Hub-Signature" = Option<String>, Header, description = "Native Jira webhook signature `sha256=<hex>`: HMAC-SHA256 of the raw body with the integration's native webhook secret. Accepted instead of the integration secret")
    ),
    responses(
        (status = 200, description = "Event processed (also when no rule matched, the event is not a status change, or it repeats a recent delivery)", body = JiraEventResponse),
        (status = 400, description = "Body is not JSON or names no issue", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unknown or disabled integration, or wrong secret or signature", body = crate::rest::error::ErrorResponse),
        (status = 413, description = "Body larger than 1 MiB"),
        (status = 429, description = "Rate limited; nothing is stored. `Retry-After` gives the seconds to wait", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn receive_jira_event(
    req: HttpRequest,
    integration_id: web::Path<String>,
    body: web::Bytes,
    integrations: web::Data<Box<dyn JiraIntegrationRepository>>,
    events: web::Data<Box<dyn JiraEventRepository>>,
    external_change: web::Data<Box<dyn ExternalChangeLogic>>,
    links: web::Data<Box<dyn ExternalLinkRepository>>,
    features: web::Data<Box<dyn FeatureRepository>>,
    environments: web::Data<Box<dyn EnvironmentRepository>>,
    limiter: web::Data<JiraInboundLimiter>,
) -> Result<HttpResponse, RestError> {
    let unauthorized = || RestError::unauthorized(UNAUTHORIZED_MESSAGE);
    let limited = |admission: Admission| match admission {
        Admission::Allowed => Ok(()),
        Admission::Limited { retry_after_secs } => {
            Err(RestError::too_many_requests(retry_after_secs))
        }
    };
    // Rate limit first: before the secret check and the body parse. A bad
    // id and an id of no enabled integration share one bucket, so the
    // limiter never grows with random ids.
    let Ok(integration_id) = Uuid::parse_str(&integration_id) else {
        limited(limiter.check_unknown())?;
        return Err(unauthorized());
    };
    let Some(integration) = integrations
        .get(integration_id)
        .await?
        .filter(|integration| integration.enabled)
    else {
        limited(limiter.check_unknown())?;
        return Err(unauthorized());
    };
    limited(limiter.check_known(integration_id))?;
    // Either check passes: the integration secret, or a native Jira webhook
    // signature. One 401 for both, so a caller never learns which check failed.
    let bearer_ok = presented_secret(&req)
        .is_some_and(|secret| secret_matches(&secret, &integration.secret_hash));
    if !bearer_ok && !native_signature_ok(&req, &integration, &body) {
        return Err(unauthorized());
    }

    let empty_event = NewJiraEvent {
        id: Uuid::new_v4(),
        integration_id,
        issue_key: None,
        jira_status: None,
        jira_actor: None,
        delivery_hash: None,
        results: serde_json::json!([]),
        unknown_environments: Vec::new(),
        unknown_features: Vec::new(),
        ignored: None,
        error: None,
    };
    let rejected = |error: String| NewJiraEvent {
        error: Some(error),
        ..empty_event.clone()
    };

    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(json) => json,
        Err(err) => {
            let message = format!("body is not JSON: {err}");
            events.insert(rejected(message.clone())).await?;
            return Err(RestError::invalid_input(message));
        }
    };
    let event = match parse_event(&json) {
        Ok(event) => event,
        Err(err) => {
            events.insert(rejected(err.to_string())).await?;
            return Err(RestError::invalid_input(err.to_string()));
        }
    };

    let jira_actor = serde_json::to_value(&event.jira_actor).ok();
    let base = NewJiraEvent {
        issue_key: Some(event.issue_key.clone()),
        jira_status: event.target_status.clone(),
        jira_actor,
        ..empty_event.clone()
    };
    let status = match (&event.target_status, event.status_changed) {
        (Some(status), true) => status.clone(),
        _ => {
            let row = events
                .insert(NewJiraEvent {
                    ignored: Some(IGNORED_NO_STATUS_CHANGE.to_string()),
                    ..base
                })
                .await?;
            return Ok(HttpResponse::Ok().json(response_from_row(&row, false)));
        }
    };

    let environment_values = field_values(&event.fields, &integration.environment_field);
    let hash = delivery_hash(
        &event.issue_key,
        &status,
        &environment_values,
        event.delivery_marker.as_deref(),
    );
    let since = Utc::now() - Duration::minutes(DUPLICATE_WINDOW_MINUTES);
    if let Some(row) = events
        .find_recent_delivery(integration_id, &hash, since)
        .await?
    {
        return Ok(HttpResponse::Ok().json(response_from_row(&row, true)));
    }
    let base = NewJiraEvent {
        delivery_hash: Some(hash),
        ..base
    };

    let rules = integrations.list_rules(integration_id).await?;
    let engine = RuleEngine {
        external_change: external_change.as_ref().as_ref(),
        links: links.as_ref().as_ref(),
        features: features.as_ref().as_ref(),
        environments: environments.as_ref().as_ref(),
    };
    match engine.run(&integration, &rules, &event, &status).await {
        Ok(outcome) => {
            // Source 1 of the write-back outbox (JI-43): one comment for the event,
            // stored in the same transaction as the event row.
            let jobs = if integration.writeback_enabled && integration.writeback_comments {
                event_comment_lines(&status, &outcome)
                    .map(|lines| {
                        vec![NewOutboundJob {
                            integration_id,
                            issue_key: event.issue_key.clone(),
                            feature_id: None,
                            kind: OutboundKind::Comment,
                            payload: serde_json::json!({ "lines": lines }),
                            dedupe_key: format!("event:{}", base.id),
                        }]
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let stored = NewJiraEvent {
                results: serde_json::to_value(&outcome.results)
                    .unwrap_or_else(|_| serde_json::json!([])),
                unknown_environments: outcome.unknown_environments,
                unknown_features: outcome.unknown_features,
                ..base
            };
            let row = if jobs.is_empty() {
                events.insert(stored).await?
            } else {
                events.insert_with_jobs(stored, jobs).await?
            };
            Ok(HttpResponse::Ok().json(response_from_row(&row, false)))
        }
        Err(err) => {
            log::error!("Jira integration {integration_id}: event processing failed: {err}");
            events
                .insert(NewJiraEvent {
                    error: Some(err.to_string()),
                    ..base
                })
                .await?;
            Err(RestError::internal("event processing failed"))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/jira-integrations/{id}/events",
    params(
        ("id" = String, Path, description = "Jira integration ID"),
        ("offset" = Option<i64>, Query, description = "Events to skip"),
        ("limit" = Option<i64>, Query, description = "Events to return (default 50, at most 200)")
    ),
    responses(
        (status = 200, description = "Events of the integration, newest first", body = JiraEventsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Integration not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Jira"
)]
#[get("/jira-integrations/{id}/events")]
pub(crate) async fn list_jira_events(
    integrations: web::Data<Box<dyn JiraIntegrationRepository>>,
    events: web::Data<Box<dyn JiraEventRepository>>,
    id: web::Path<String>,
    query: web::Query<PaginationQuery>,
) -> Result<impl Responder, RestError> {
    let id =
        Uuid::parse_str(&id).map_err(|_| RestError::invalid_input("invalid integration id"))?;
    if integrations.get(id).await?.is_none() {
        return Err(RestError::not_found(format!(
            "Jira integration {id} not found"
        )));
    }
    let (offset, limit) = normalize_pagination(&query);
    let (rows, total) = events.list(id, offset, limit).await?;
    Ok(HttpResponse::Ok().json(JiraEventsResponse {
        items: rows.into_iter().map(map_event).collect(),
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::resource("/integrations/jira/{integration_id}/events")
            .app_data(web::PayloadConfig::new(MAX_EVENT_BODY_BYTES))
            .route(web::post().to(receive_jira_event)),
    )
    .service(list_jira_events);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::entity::{Environment, JiraIntegrationRow, JiraStatusRuleRow};
    use crate::database::environment::MockEnvironmentRepository;
    use crate::database::external_link::{FeatureScope, MockExternalLinkRepository};
    use crate::database::feature::MockFeatureRepository;
    use crate::database::jira_event::MockJiraEventRepository;
    use crate::database::jira_integration::MockJiraIntegrationRepository;
    use crate::database::jira_outbound_job::{NewOutboundJob, OutboundKind};
    use crate::logic::external_change::{ExternalOutcome, MockExternalChangeLogic};
    use actix_web::{App, http::StatusCode, test};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    const SECRET: &str = "s3cr3t-value-for-tests";

    struct Setup {
        integration: JiraIntegrationRow,
        rules: Vec<JiraStatusRuleRow>,
        qa: Environment,
        prod: Environment,
        features: Vec<(Uuid, String)>,
        /// What `find_recent_delivery` returns.
        stored_delivery: Option<JiraEventRow>,
        /// `None`: `get` finds nothing.
        known: bool,
        /// Burst of the limiter registered for the test app.
        burst: u32,
    }

    fn setup() -> Setup {
        let team_id = Uuid::new_v4();
        let env = |name: &str| Environment {
            id: Uuid::new_v4(),
            name: name.to_string(),
            active: true,
            team_id,
            environment_type: "Development".to_string(),
        };
        let (qa, prod) = (env("QA"), env("Production"));
        let integration = JiraIntegrationRow {
            id: Uuid::new_v4(),
            team_id,
            name: "Jira".to_string(),
            jira_base_url: None,
            secret_hash: hash_secret(SECRET),
            environment_field: "customfield_10042".to_string(),
            environment_aliases: sqlx::types::Json(BTreeMap::new()),
            jira_approved_environment_ids: vec![qa.id],
            feature_key_field: None,
            actor_user_id: Uuid::new_v4(),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            writeback_enabled: false,
            writeback_comments: true,
            writeback_remote_link: true,
            jira_auth_kind: None,
            jira_account_email: None,
            jira_credential_enc: None,
            writeback_paused_reason: None,
            native_webhook_secret_enc: None,
        };
        let rule = |status: &str, action: &str, position: i32| JiraStatusRuleRow {
            id: Uuid::new_v4(),
            integration_id: integration.id,
            jira_status: status.to_string(),
            action: action.to_string(),
            environment_ids: None,
            enabled: true,
            position,
        };
        let rules = vec![
            rule("Ready for Release", "approve", 0),
            rule("Ready for Release", "request", 1),
            rule("Done", "deploy", 2),
        ];
        Setup {
            integration,
            rules,
            qa,
            prod,
            features: vec![
                (Uuid::new_v4(), "checkout".to_string()),
                (Uuid::new_v4(), "search".to_string()),
            ],
            stored_delivery: None,
            known: true,
            burst: 50,
        }
    }

    type Applied = Arc<Mutex<Vec<(Uuid, Uuid, String)>>>;
    type Inserted = Arc<Mutex<Vec<NewJiraEvent>>>;
    type WithJobs = Arc<Mutex<Vec<(NewJiraEvent, Vec<NewOutboundJob>)>>>;

    struct Outcome {
        status: StatusCode,
        body: serde_json::Value,
        /// Status and `Retry-After` of every request sent, in order.
        responses: Vec<(StatusCode, Option<String>)>,
        applied: Applied,
        inserted: Inserted,
        with_jobs: WithJobs,
    }

    async fn send(setup: Setup, request: test::TestRequest) -> Outcome {
        send_all(setup, vec![request]).await
    }

    /// Sends the requests, in order, to one app (so one limiter).
    async fn send_all(setup: Setup, requests: Vec<test::TestRequest>) -> Outcome {
        let applied: Applied = Arc::default();
        let inserted: Inserted = Arc::default();
        let with_jobs: WithJobs = Arc::default();

        let mut integrations = MockJiraIntegrationRepository::new();
        let integration = setup.integration.clone();
        let known = setup.known;
        integrations
            .expect_get()
            .returning(move |id| Ok((known && id == integration.id).then(|| integration.clone())));
        let rules = setup.rules.clone();
        integrations
            .expect_list_rules()
            .returning(move |_| Ok(rules.clone()));

        let mut events = MockJiraEventRepository::new();
        let stored = setup.stored_delivery.clone();
        events
            .expect_find_recent_delivery()
            .returning(move |_, _, since| {
                assert!(since <= Utc::now() - Duration::minutes(DUPLICATE_WINDOW_MINUTES - 1));
                Ok(stored.clone())
            });
        fn stored_row(event: NewJiraEvent) -> JiraEventRow {
            JiraEventRow {
                id: event.id,
                integration_id: event.integration_id,
                received_at: Utc::now(),
                issue_key: event.issue_key,
                jira_status: event.jira_status,
                jira_actor: event.jira_actor,
                delivery_hash: event.delivery_hash,
                results: event.results,
                unknown_environments: event.unknown_environments,
                unknown_features: event.unknown_features,
                ignored: event.ignored,
                error: event.error,
            }
        }
        let record = inserted.clone();
        events.expect_insert().returning(move |event| {
            record.lock().unwrap().push(event.clone());
            Ok(stored_row(event))
        });
        let record = with_jobs.clone();
        events
            .expect_insert_with_jobs()
            .returning(move |event, jobs| {
                record.lock().unwrap().push((event.clone(), jobs));
                Ok(stored_row(event))
            });

        let mut external = MockExternalChangeLogic::new();
        let calls = applied.clone();
        external
            .expect_apply_external_action()
            .returning(move |feature, env, action, _| {
                calls
                    .lock()
                    .unwrap()
                    .push((feature, env, format!("{action:?}")));
                Ok(ExternalOutcome::NoOp {
                    status: "NOT_DEPLOYED".to_string(),
                })
            });

        let mut links = MockExternalLinkRepository::new();
        let ids: Vec<Uuid> = setup.features.iter().map(|(id, _)| *id).collect();
        links
            .expect_feature_ids_for_key()
            .returning(move |_, _, _| Ok(ids.clone()));
        let keys = setup.features.clone();
        let team_id = setup.integration.team_id;
        links.expect_feature_scope().returning(move |id| {
            Ok(keys
                .iter()
                .find(|(feature, _)| *feature == id)
                .map(|(_, key)| FeatureScope {
                    team_id,
                    key: key.clone(),
                }))
        });

        let mut environments = MockEnvironmentRepository::new();
        let team_envs = vec![setup.qa.clone(), setup.prod.clone()];
        environments
            .expect_get_environments()
            .returning(move |_, _, _| Ok(team_envs.clone()));

        let integrations: Box<dyn JiraIntegrationRepository> = Box::new(integrations);
        let events: Box<dyn JiraEventRepository> = Box::new(events);
        let external: Box<dyn ExternalChangeLogic> = Box::new(external);
        let links: Box<dyn ExternalLinkRepository> = Box::new(links);
        let features: Box<dyn FeatureRepository> = Box::new(MockFeatureRepository::new());
        let environments: Box<dyn EnvironmentRepository> = Box::new(environments);
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(integrations))
                .app_data(web::Data::new(events))
                .app_data(web::Data::new(external))
                .app_data(web::Data::new(links))
                .app_data(web::Data::new(features))
                .app_data(web::Data::new(environments))
                .app_data(web::Data::new(JiraInboundLimiter::new(60, setup.burst, 30)))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let mut responses = Vec::new();
        let mut last = (StatusCode::OK, serde_json::Value::Null);
        for request in requests {
            let resp = test::call_service(&app, request.to_request()).await;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let bytes = test::read_body(resp).await;
            responses.push((status, retry_after));
            last = (status, serde_json::from_slice(&bytes).unwrap_or_default());
        }
        Outcome {
            status: last.0,
            body: last.1,
            responses,
            applied,
            inserted,
            with_jobs,
        }
    }

    fn post(setup: &Setup) -> test::TestRequest {
        test::TestRequest::post().uri(&format!(
            "/api/v1/integrations/jira/{}/events",
            setup.integration.id
        ))
    }

    fn status_change(status: &str, environments: serde_json::Value) -> serde_json::Value {
        json!({
            "webhookEvent": "jira:issue_updated",
            "user": {"accountId": "acc-1", "displayName": "Jane Doe"},
            "issue": {
                "key": "PROJ-123",
                "fields": {"status": {"name": status}, "customfield_10042": environments}
            },
            "changelog": {"id": "100", "items": [{"field": "status", "toString": status}]}
        })
    }

    fn bearer(request: test::TestRequest, secret: &str) -> test::TestRequest {
        request.insert_header(("Authorization", format!("Bearer {secret}")))
    }

    async fn list_events(
        known: bool,
        uri_suffix: &str,
    ) -> (StatusCode, serde_json::Value, Arc<Mutex<Vec<(i64, i64)>>>) {
        let id = Uuid::new_v4();
        let pages: Arc<Mutex<Vec<(i64, i64)>>> = Arc::default();
        let mut integrations = MockJiraIntegrationRepository::new();
        let s = setup();
        let integration = s.integration.clone();
        integrations
            .expect_get()
            .returning(move |_| Ok(known.then(|| integration.clone())));
        let mut events = MockJiraEventRepository::new();
        let record = pages.clone();
        events
            .expect_list()
            .returning(move |integration_id, offset, limit| {
                record.lock().unwrap().push((offset, limit));
                let row = JiraEventRow {
                    id: Uuid::new_v4(),
                    integration_id,
                    received_at: Utc::now(),
                    issue_key: Some("PROJ-1".to_string()),
                    jira_status: Some("Done".to_string()),
                    jira_actor: Some(json!({"accountId": "acc", "displayName": "Jane"})),
                    delivery_hash: Some("hash".to_string()),
                    results: json!([]),
                    unknown_environments: vec!["Lab".to_string()],
                    unknown_features: vec![],
                    ignored: None,
                    error: None,
                };
                Ok((vec![row], 7))
            });
        let integrations: Box<dyn JiraIntegrationRepository> = Box::new(integrations);
        let events: Box<dyn JiraEventRepository> = Box::new(events);
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(integrations))
                .app_data(web::Data::new(events))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get()
            .uri(&format!(
                "/api/v1/jira-integrations/{id}/events{uri_suffix}"
            ))
            .to_request();
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let bytes = test::read_body(resp).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or_default(),
            pages,
        )
    }

    #[actix_web::test]
    async fn the_event_log_is_paged_and_404_for_an_unknown_integration() {
        let (status, body, pages) = list_events(true, "?offset=5&limit=500").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(*pages.lock().unwrap(), vec![(5, 200)]);
        assert_eq!(body["meta"], json!({"offset": 5, "limit": 200, "total": 7}));
        let item = &body["items"][0];
        assert_eq!(item["issueKey"], "PROJ-1");
        assert_eq!(item["jiraActor"]["displayName"], "Jane");
        assert_eq!(item["unknownEnvironments"], json!(["Lab"]));
        assert!(item.get("deliveryHash").is_none());

        let (status, _, pages) = list_events(false, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(pages.lock().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn a_wrong_secret_is_401_and_does_nothing() {
        let s = setup();
        let request = bearer(post(&s), "wrong").set_json(status_change("Done", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);
        assert!(out.applied.lock().unwrap().is_empty());
        assert!(out.inserted.lock().unwrap().is_empty());
    }

    const CLOUD_FIXTURE: &str =
        include_str!("../../tests/fixtures/jira/cloud_webhook_status_change.json");

    /// Seals a fresh native secret for the integration (AAD = integration id) and
    /// returns the plaintext. The key is the process-wide test key.
    fn with_native_secret(setup: &mut Setup) -> String {
        crate::rest::jira_integration::tests::ensure_encryption_key();
        let secret = crate::logic::jira_integration::generate_secret();
        setup.integration.native_webhook_secret_enc = Some(
            crate::logic::secret_box::encrypt_with_aad(&secret, setup.integration.id.as_bytes())
                .expect("seal native secret"),
        );
        secret
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("sha256={hex}")
    }

    fn signed(setup: &Setup, secret: &str, body: &str) -> test::TestRequest {
        post(setup)
            .insert_header((
                crate::logic::jira_signature::SIGNATURE_HEADER,
                sign(secret, body.as_bytes()),
            ))
            .insert_header(("Content-Type", "application/json"))
            .set_payload(body.to_string())
    }

    #[actix_web::test]
    async fn signed_native_webhook_is_accepted_without_the_bearer_secret() {
        let mut s = setup();
        let native = with_native_secret(&mut s);
        let request = signed(&s, &native, CLOUD_FIXTURE);
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::OK, "{}", out.body);
        let inserted = out.inserted.lock().unwrap();
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].issue_key.as_deref(), Some("PROJ-123"));
        assert!(inserted[0].error.is_none());
        assert!(inserted[0].ignored.is_none());
    }

    #[actix_web::test]
    async fn bad_signature_is_401_and_does_nothing() {
        let mut s = setup();
        with_native_secret(&mut s);
        let other = crate::logic::jira_integration::generate_secret();
        let wrong_secret = signed(&s, &other, CLOUD_FIXTURE);
        let out = send(s, wrong_secret).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);
        assert!(out.applied.lock().unwrap().is_empty());
        assert!(out.inserted.lock().unwrap().is_empty());

        // Signed body differs from the sent body by one byte.
        let mut s = setup();
        let native2 = with_native_secret(&mut s);
        let tampered = CLOUD_FIXTURE.replacen("Jane", "Jone", 1);
        let request = post(&s)
            .insert_header((
                crate::logic::jira_signature::SIGNATURE_HEADER,
                sign(&native2, CLOUD_FIXTURE.as_bytes()),
            ))
            .set_payload(tampered);
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);
        assert!(out.inserted.lock().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn signature_without_a_stored_native_secret_is_401() {
        let s = setup();
        let request = signed(
            &s,
            &crate::logic::jira_integration::generate_secret(),
            CLOUD_FIXTURE,
        );
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);
        assert!(out.inserted.lock().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn bearer_secret_still_works_when_a_native_secret_exists() {
        let mut s = setup();
        with_native_secret(&mut s);
        let request = bearer(post(&s), SECRET).set_json(status_change("Done", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::OK, "{}", out.body);
        assert_eq!(out.inserted.lock().unwrap().len(), 1);
    }

    #[actix_web::test]
    async fn undecryptable_native_secret_counts_as_none() {
        crate::rest::jira_integration::tests::ensure_encryption_key();
        let mut s = setup();
        s.integration.native_webhook_secret_enc = Some("not-a-sealed-value".to_string());
        let request = signed(
            &s,
            &crate::logic::jira_integration::generate_secret(),
            CLOUD_FIXTURE,
        );
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);
    }

    #[actix_web::test]
    async fn no_secret_is_401() {
        let s = setup();
        let request = post(&s).set_json(status_change("Done", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn an_unknown_or_disabled_integration_gets_the_same_401() {
        let mut unknown = setup();
        unknown.known = false;
        let request = bearer(post(&unknown), SECRET).set_json(status_change("Done", json!("QA")));
        let out = send(unknown, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);

        let mut disabled = setup();
        disabled.integration.enabled = false;
        let request = bearer(post(&disabled), SECRET).set_json(status_change("Done", json!("QA")));
        let out = send(disabled, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
        assert_eq!(out.body["message"], UNAUTHORIZED_MESSAGE);
        assert!(out.inserted.lock().unwrap().is_empty());

        let s = setup();
        let request = test::TestRequest::post()
            .uri("/api/v1/integrations/jira/not-a-uuid/events")
            .insert_header(("Authorization", format!("Bearer {SECRET}")))
            .set_json(status_change("Done", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn a_status_change_runs_each_rule_feature_and_environment_once() {
        let s = setup();
        let (qa, prod) = (s.qa.id, s.prod.id);
        let features: Vec<Uuid> = s.features.iter().map(|(id, _)| *id).collect();
        let request = bearer(post(&s), SECRET).set_json(status_change(
            "Ready for Release",
            json!([{"value": "QA"}, {"value": "Production"}, {"value": "Lab"}]),
        ));
        let integration_id = s.integration.id;

        let out = send(s, request).await;

        assert_eq!(out.status, StatusCode::OK, "{}", out.body);
        let applied = out.applied.lock().unwrap().clone();
        // 2 rules x 2 features x 2 environments.
        assert_eq!(applied.len(), 8);
        for (feature, env, action) in [
            (features[0], qa, "Approve"),
            (features[1], prod, "Approve"),
            (features[0], prod, "Request"),
            (features[1], qa, "Request"),
        ] {
            assert_eq!(
                applied
                    .iter()
                    .filter(|call| **call == (feature, env, action.to_string()))
                    .count(),
                1
            );
        }
        assert_eq!(out.body["results"].as_array().unwrap().len(), 8);
        assert_eq!(out.body["unknownEnvironments"], json!(["Lab"]));
        assert_eq!(out.body["duplicate"], false);
        assert!(out.body["eventId"].is_string());

        let inserted = out.inserted.lock().unwrap();
        assert_eq!(inserted.len(), 1);
        let event = &inserted[0];
        assert_eq!(event.integration_id, integration_id);
        assert_eq!(event.issue_key.as_deref(), Some("PROJ-123"));
        assert_eq!(event.jira_status.as_deref(), Some("Ready for Release"));
        assert_eq!(
            event.jira_actor.as_ref().unwrap()["displayName"],
            "Jane Doe"
        );
        assert_eq!(event.results.as_array().unwrap().len(), 8);
        assert_eq!(event.delivery_hash.as_ref().map(String::len), Some(64));
        assert!(event.error.is_none());
    }

    #[actix_web::test]
    async fn the_secret_header_works_too() {
        let s = setup();
        let request = post(&s)
            .insert_header((SECRET_HEADER, SECRET))
            .set_json(status_change("Done", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::OK, "{}", out.body);
        // deploy x 2 features x QA.
        assert_eq!(out.applied.lock().unwrap().len(), 2);
    }

    #[actix_web::test]
    async fn a_repeated_delivery_returns_the_stored_result_and_does_nothing() {
        let mut s = setup();
        let stored_id = Uuid::new_v4();
        s.stored_delivery = Some(JiraEventRow {
            id: stored_id,
            integration_id: s.integration.id,
            received_at: Utc::now(),
            issue_key: Some("PROJ-123".to_string()),
            jira_status: Some("Done".to_string()),
            jira_actor: None,
            delivery_hash: Some("hash".to_string()),
            results: json!([{
                "featureId": Uuid::nil().to_string(), "featureKey": "checkout",
                "environmentId": Uuid::nil().to_string(), "environment": "QA",
                "ruleId": Uuid::nil().to_string(), "ruleStatus": "Done", "action": "deploy",
                "outcome": "applied", "from": "DEPLOYMENT_APPROVED", "to": "DEPLOYED",
                "reason": null, "approvalRequestId": null
            }]),
            unknown_environments: vec![],
            unknown_features: vec![],
            ignored: None,
            error: None,
        });
        let request = bearer(post(&s), SECRET).set_json(status_change("Done", json!("QA")));

        let out = send(s, request).await;

        assert_eq!(out.status, StatusCode::OK);
        assert_eq!(out.body["eventId"], stored_id.to_string());
        assert_eq!(out.body["duplicate"], true);
        assert_eq!(out.body["results"][0]["to"], "DEPLOYED");
        assert!(out.applied.lock().unwrap().is_empty());
        assert!(out.inserted.lock().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn a_change_that_is_not_a_status_change_is_stored_and_ignored() {
        let s = setup();
        let mut body = status_change("Done", json!("QA"));
        body["changelog"]["items"] = json!([{"field": "summary", "toString": "x"}]);
        let request = bearer(post(&s), SECRET).set_json(body);

        let out = send(s, request).await;

        assert_eq!(out.status, StatusCode::OK);
        assert_eq!(out.body["ignored"], IGNORED_NO_STATUS_CHANGE);
        assert_eq!(out.body["results"], json!([]));
        assert!(out.applied.lock().unwrap().is_empty());
        let inserted = out.inserted.lock().unwrap();
        assert_eq!(
            inserted[0].ignored.as_deref(),
            Some(IGNORED_NO_STATUS_CHANGE)
        );
    }

    #[actix_web::test]
    async fn invalid_json_or_no_issue_is_400_and_stored_with_the_error() {
        for body in [
            "not json".to_string(),
            json!({"hello": "world"}).to_string(),
        ] {
            let s = setup();
            let request = bearer(post(&s), SECRET)
                .insert_header(("Content-Type", "application/json"))
                .set_payload(body.clone());
            let out = send(s, request).await;
            assert_eq!(out.status, StatusCode::BAD_REQUEST, "{body}");
            let inserted = out.inserted.lock().unwrap();
            assert_eq!(inserted.len(), 1, "{body}");
            assert!(inserted[0].error.is_some(), "{body}");
            assert!(out.applied.lock().unwrap().is_empty());
        }
    }

    #[actix_web::test]
    async fn processed_event_enqueues_one_comment_when_writeback_is_on() {
        let mut s = setup();
        s.integration.writeback_enabled = true;
        let integration_id = s.integration.id;
        let request = bearer(post(&s), SECRET).set_json(status_change(
            "Ready for Release",
            json!([{"value": "QA"}, {"value": "Lab"}]),
        ));

        let out = send(s, request).await;

        assert_eq!(out.status, StatusCode::OK, "{}", out.body);
        assert!(out.inserted.lock().unwrap().is_empty());
        let with_jobs = out.with_jobs.lock().unwrap();
        assert_eq!(with_jobs.len(), 1);
        let (event, jobs) = &with_jobs[0];
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.kind, OutboundKind::Comment);
        assert_eq!(job.integration_id, integration_id);
        assert_eq!(job.issue_key, "PROJ-123");
        assert_eq!(job.feature_id, None);
        assert_eq!(job.dedupe_key, format!("event:{}", event.id));
        assert_eq!(out.body["eventId"], event.id.to_string());
        let lines = job.payload["lines"].as_array().unwrap();
        assert_eq!(lines[0], "FluxGate: Jira status 'Ready for Release'");
        assert!(lines.iter().any(|l| l == "Unknown environment values: Lab"));
        assert!(
            lines
                .iter()
                .any(|l| l.as_str().unwrap().starts_with("checkout · QA: "))
        );
    }

    #[actix_web::test]
    async fn duplicate_and_ignored_events_enqueue_nothing() {
        let mut s = setup();
        s.integration.writeback_enabled = true;
        s.stored_delivery = Some(JiraEventRow {
            id: Uuid::new_v4(),
            integration_id: s.integration.id,
            received_at: Utc::now(),
            issue_key: Some("PROJ-123".to_string()),
            jira_status: Some("Done".to_string()),
            jira_actor: None,
            delivery_hash: Some("hash".to_string()),
            results: json!([]),
            unknown_environments: vec![],
            unknown_features: vec![],
            ignored: None,
            error: None,
        });
        let request = bearer(post(&s), SECRET).set_json(status_change("Done", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.body["duplicate"], true);
        assert!(out.with_jobs.lock().unwrap().is_empty());

        let mut s = setup();
        s.integration.writeback_enabled = true;
        let mut body = status_change("Done", json!("QA"));
        body["changelog"]["items"] = json!([{"field": "summary", "toString": "x"}]);
        let request = bearer(post(&s), SECRET).set_json(body);
        let out = send(s, request).await;
        assert_eq!(out.body["ignored"], IGNORED_NO_STATUS_CHANGE);
        assert!(out.with_jobs.lock().unwrap().is_empty());
        assert_eq!(out.inserted.lock().unwrap().len(), 1);

        // Nothing to report: a status with no rule and no unknown values.
        let mut s = setup();
        s.integration.writeback_enabled = true;
        let request = bearer(post(&s), SECRET).set_json(status_change("Unruled", json!("QA")));
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::OK);
        assert!(out.with_jobs.lock().unwrap().is_empty());
        assert_eq!(out.inserted.lock().unwrap().len(), 1);
    }

    #[actix_web::test]
    async fn writeback_off_uses_plain_insert() {
        for (enabled, comments) in [(false, true), (true, false)] {
            let mut s = setup();
            s.integration.writeback_enabled = enabled;
            s.integration.writeback_comments = comments;
            let request =
                bearer(post(&s), SECRET).set_json(status_change("Ready for Release", json!("QA")));
            let out = send(s, request).await;
            assert_eq!(out.status, StatusCode::OK, "{}", out.body);
            assert!(out.with_jobs.lock().unwrap().is_empty());
            assert_eq!(out.inserted.lock().unwrap().len(), 1);
        }
    }

    fn valid_request(s: &Setup, secret: &str) -> test::TestRequest {
        bearer(post(s), secret).set_json(status_change("Ready for Release", json!("QA")))
    }

    #[actix_web::test]
    async fn over_the_limit_is_429_with_retry_after_and_stores_nothing() {
        let mut s = setup();
        s.burst = 2;
        let requests = (0..3).map(|_| valid_request(&s, SECRET)).collect();
        let out = send_all(s, requests).await;
        assert_eq!(out.responses[0].0, StatusCode::OK);
        assert_eq!(out.responses[1].0, StatusCode::OK);
        assert_eq!(out.responses[2].0, StatusCode::TOO_MANY_REQUESTS);
        let retry_after: u64 = out.responses[2]
            .1
            .clone()
            .expect("Retry-After")
            .parse()
            .unwrap();
        assert!(retry_after >= 1);
        assert_eq!(out.body["error"], "rate_limited");
        assert_eq!(out.inserted.lock().unwrap().len(), 2);
    }

    #[actix_web::test]
    async fn limited_request_with_a_wrong_secret_is_still_429() {
        let mut s = setup();
        s.burst = 1;
        let requests = vec![valid_request(&s, SECRET), valid_request(&s, "wrong")];
        let out = send_all(s, requests).await;
        assert_eq!(out.responses[0].0, StatusCode::OK);
        assert_eq!(out.responses[1].0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(out.inserted.lock().unwrap().len(), 1);
    }

    #[actix_web::test]
    async fn unknown_integration_ids_hit_the_shared_bucket() {
        let mut s = setup();
        s.known = false;
        let requests = (0..11).map(|_| valid_request(&s, SECRET)).collect();
        let out = send_all(s, requests).await;
        assert!(
            out.responses[..10]
                .iter()
                .all(|(status, _)| *status == StatusCode::UNAUTHORIZED)
        );
        assert_eq!(out.responses[10].0, StatusCode::TOO_MANY_REQUESTS);
        assert!(out.responses[10].1.is_some());
        assert!(out.inserted.lock().unwrap().is_empty());
    }

    #[actix_web::test]
    async fn bad_uuid_counts_against_the_unknown_bucket() {
        let s = setup();
        let requests = (0..11)
            .map(|_| {
                test::TestRequest::post()
                    .uri("/api/v1/integrations/jira/not-a-uuid/events")
                    .set_json(status_change("Done", json!("QA")))
            })
            .collect();
        let out = send_all(s, requests).await;
        assert_eq!(out.responses[10].0, StatusCode::TOO_MANY_REQUESTS);
    }

    #[actix_web::test]
    async fn a_body_above_the_default_limit_but_under_1_mib_is_accepted() {
        let s = setup();
        let mut body = status_change("Done", json!("QA"));
        // actix-web's default payload limit is 256 KiB.
        body["issue"]["fields"]["description"] = json!("x".repeat(600 * 1024));
        let request = bearer(post(&s), SECRET).set_json(body);
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::OK, "{}", out.body);
    }

    #[actix_web::test]
    async fn a_body_over_1_mib_is_rejected() {
        let s = setup();
        let mut body = status_change("Done", json!("QA"));
        body["issue"]["fields"]["description"] = json!("x".repeat(MAX_EVENT_BODY_BYTES));
        let request = bearer(post(&s), SECRET).set_json(body);
        let out = send(s, request).await;
        assert_eq!(out.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(out.inserted.lock().unwrap().is_empty());
    }
}

/// End to end through the real logic and repositories on the seeded test DB:
/// Jira-shaped events approve and deploy a linked feature in a Jira-approved
/// environment, and the event log shows the results.
#[cfg(test)]
mod flow_tests {
    use super::*;
    use crate::database::activity_log::activity_log_repository;
    use crate::database::approval::approval_repository;
    use crate::database::environment::environment_repository;
    use crate::database::external_link::external_link_repository;
    use crate::database::feature::{CreateFeature, CreateFeatureStage, feature_repository};
    use crate::database::jira_event::jira_event_repository;
    use crate::database::jira_integration::{
        jira_integration_repository, jira_integration_repository_tx,
    };
    use crate::logic::ActorContext;
    use crate::logic::external_change::external_change_logic;
    use crate::logic::jira_integration::JiraStatusRuleInput;
    use crate::logic::jira_integration_tx::{
        JiraIntegrationInput, create_jira_integration_in_tx, replace_jira_status_rules_in_tx,
    };
    use crate::logic::{approval, environment, feature};
    use actix_web::{App, http::StatusCode, test};
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::BTreeMap;

    const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const SEED_APPROVER_ID: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

    fn uuid(value: &str) -> Uuid {
        Uuid::parse_str(value).unwrap()
    }

    struct Flow {
        pool: sqlx::PgPool,
        team_id: Uuid,
        integration_id: Uuid,
        shadow_user: Uuid,
        secret: String,
        feature_id: Uuid,
    }

    impl Flow {
        async fn new() -> Self {
            let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&db_url)
                .await
                .expect("connect");
            let team_id = Uuid::new_v4();
            sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira flow')")
                .bind(team_id)
                .bind(format!("jira-flow-{team_id}"))
                .execute(&pool)
                .await
                .expect("insert team");
            let qa = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO environments (id, name, active, team_id, environment_type) \
                 VALUES ($1, 'QA', TRUE, $2, 'Development')",
            )
            .bind(qa)
            .bind(team_id)
            .execute(&pool)
            .await
            .expect("insert environment");
            for user in [SEED_ADMIN_ID, SEED_APPROVER_ID] {
                sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
                    .bind(uuid(user))
                    .bind(team_id)
                    .execute(&pool)
                    .await
                    .expect("insert team member");
            }
            // QA needs an approval: Jira approves it because QA trusts Jira.
            sqlx::query(
                "INSERT INTO approval_policies (team_id, name, applies_to, environment_ids, \
                 required_approvers, approver_role_ids, enabled) \
                 VALUES ($1, 'QA approvals', 'specific_environments', $2, 1, $3, TRUE)",
            )
            .bind(team_id)
            .bind(vec![qa])
            .bind(vec![uuid(APPROVER_ROLE_ID)])
            .execute(&pool)
            .await
            .expect("insert policy");

            let actor = ActorContext::new(uuid(SEED_ADMIN_ID), "admin".to_string());
            let repo = jira_integration_repository_tx(pool.clone());
            let activity = activity_log_repository(pool.clone());
            let mut tx = pool.begin().await.expect("begin");
            let created = create_jira_integration_in_tx(
                &mut tx,
                &repo,
                activity.as_ref(),
                team_id,
                JiraIntegrationInput {
                    name: "Jira".to_string(),
                    jira_base_url: None,
                    environment_field: "labels".to_string(),
                    environment_aliases: BTreeMap::new(),
                    jira_approved_environment_ids: vec![qa.to_string()],
                    feature_key_field: None,
                    enabled: true,
                },
                actor.clone(),
            )
            .await
            .expect("create integration");
            let integration_id = created.integration.id;
            let rule = |status: &str, action: &str| JiraStatusRuleInput {
                jira_status: status.to_string(),
                action: action.to_string(),
                environment_ids: None,
                enabled: true,
            };
            replace_jira_status_rules_in_tx(
                &mut tx,
                &repo,
                activity.as_ref(),
                integration_id,
                vec![rule("Ready for Release", "approve"), rule("Done", "deploy")],
                actor,
            )
            .await
            .expect("rules");
            tx.commit().await.expect("commit");

            let feature_id = feature_repository(pool.clone())
                .create_feature(CreateFeature {
                    team_id,
                    key: format!("jira-flow-{}", Uuid::new_v4()),
                    description: None,
                    feature_type: crate::database::entity::FeatureType::Simple,
                    lifecycle_stage: "active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    stages: vec![CreateFeatureStage {
                        id: Uuid::new_v4(),
                        environment_id: qa,
                        order_index: 0,
                        parent_stage: None,
                        position: "{ \"x\": 0, \"y\": 0 }".to_string(),
                        enabled: false,
                    }],
                    dependencies: vec![],
                    variants: None,
                    flag_kind: None,
                })
                .await
                .expect("create feature");
            sqlx::query(
                "INSERT INTO feature_external_links (id, feature_id, system, external_key) \
                 VALUES ($1, $2, 'jira', 'PROJ-123')",
            )
            .bind(Uuid::new_v4())
            .bind(feature_id)
            .execute(&pool)
            .await
            .expect("link feature");

            Flow {
                pool,
                team_id,
                integration_id,
                shadow_user: created.integration.actor_user_id,
                secret: created.secret,
                feature_id,
            }
        }

        async fn send(&self, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
            let pool = self.pool.clone();
            let activity = activity_log_repository(pool.clone());
            let environment_logic = environment::environment_logic(
                environment_repository(pool.clone()),
                activity.clone_box(),
            );
            let (approval_events_tx, _) = tokio::sync::broadcast::channel(16);
            let (updates_tx, _) = tokio::sync::broadcast::channel(16);
            let approval_logic = approval::approval_logic_with_pool(
                pool.clone(),
                approval_repository(pool.clone()),
                feature_repository(pool.clone()),
                environment_logic.clone(),
                crate::database::role::role_repository(pool.clone()),
                approval_events_tx,
                updates_tx.clone(),
            );
            let feature_logic = feature::feature_logic_with_approval(
                feature_repository(pool.clone()),
                environment_logic,
                activity.clone_box(),
                crate::database::user::user_repository(pool.clone()),
                Some(approval_logic.clone()),
            );
            let external = external_change_logic(
                pool.clone(),
                feature_logic,
                approval_logic,
                feature_repository(pool.clone()),
                activity,
                updates_tx,
            );
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(jira_integration_repository(pool.clone())))
                    .app_data(web::Data::new(jira_event_repository(pool.clone())))
                    .app_data(web::Data::new(external))
                    .app_data(web::Data::new(external_link_repository(pool.clone())))
                    .app_data(web::Data::new(feature_repository(pool.clone())))
                    .app_data(web::Data::new(environment_repository(pool.clone())))
                    .app_data(web::Data::new(JiraInboundLimiter::new(60, 60, 30)))
                    .service(web::scope("/api/v1").configure(super::configure)),
            )
            .await;
            let req = test::TestRequest::post()
                .uri(&format!(
                    "/api/v1/integrations/jira/{}/events",
                    self.integration_id
                ))
                .insert_header(("Authorization", format!("Bearer {}", self.secret)))
                .set_json(body)
                .to_request();
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (status, serde_json::from_slice(&bytes).unwrap_or_default())
        }

        async fn stage_status(&self) -> String {
            sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE feature_id = $1")
                .bind(self.feature_id)
                .fetch_one(&self.pool)
                .await
                .expect("stage status")
        }

        async fn cleanup(self) {
            sqlx::query("DELETE FROM approval_requests WHERE feature_id = $1")
                .bind(self.feature_id)
                .execute(&self.pool)
                .await
                .expect("delete requests");
            sqlx::query("DELETE FROM approval_policies WHERE team_id = $1")
                .bind(self.team_id)
                .execute(&self.pool)
                .await
                .expect("delete policies");
            sqlx::query("DELETE FROM teams WHERE id = $1")
                .bind(self.team_id)
                .execute(&self.pool)
                .await
                .expect("delete team");
            sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(self.shadow_user)
                .execute(&self.pool)
                .await
                .expect("delete shadow user");
        }
    }

    fn webhook(status: &str, changelog_id: &str) -> serde_json::Value {
        json!({
            "webhookEvent": "jira:issue_updated",
            "user": {"accountId": "5b10ac8d82e05b22cc7d4ef5", "displayName": "Jane Doe"},
            "issue": {
                "key": "PROJ-123",
                "fields": {"status": {"name": status}, "labels": ["QA"]}
            },
            "changelog": {"id": changelog_id, "items": [{"field": "status", "toString": status}]}
        })
    }

    #[actix_web::test]
    async fn ready_for_release_approves_and_done_deploys() {
        let flow = Flow::new().await;

        let (status, body) = flow.send(webhook("Ready for Release", "1")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["outcome"], "applied", "{body}");
        assert_eq!(body["results"][0]["to"], "DEPLOYMENT_APPROVED");
        assert_eq!(flow.stage_status().await, "DEPLOYMENT_APPROVED");
        let source: String = sqlx::query_scalar(
            "SELECT approval_source FROM approval_requests WHERE feature_id = $1",
        )
        .bind(flow.feature_id)
        .fetch_one(&flow.pool)
        .await
        .unwrap();
        assert_eq!(source, "jira");

        // The same delivery again: stored result, nothing new.
        let (status, again) = flow.send(webhook("Ready for Release", "1")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["duplicate"], true);
        assert_eq!(again["eventId"], body["eventId"]);

        let (status, body) = flow.send(webhook("Done", "2")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["action"], "deploy");
        assert_eq!(body["results"][0]["to"], "DEPLOYED");
        assert_eq!(flow.stage_status().await, "DEPLOYED");

        let (events, total) = jira_event_repository(flow.pool.clone())
            .list(flow.integration_id, 0, 10)
            .await
            .unwrap();
        assert_eq!(total, 2, "a duplicate is not stored again");
        assert_eq!(events[0].jira_status.as_deref(), Some("Done"));
        assert_eq!(events[0].results[0]["to"], "DEPLOYED");
        assert_eq!(events[1].jira_status.as_deref(), Some("Ready for Release"));

        flow.cleanup().await;
    }

    #[actix_web::test]
    async fn done_before_approval_is_refused_and_logged() {
        let flow = Flow::new().await;

        let (status, body) = flow.send(webhook("Done", "9")).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["outcome"], "refused");
        assert_eq!(body["results"][0]["reason"], "not approved");
        assert_eq!(flow.stage_status().await, "NOT_DEPLOYED");
        flow.cleanup().await;
    }

    /// A Jira event gives one comment (Source 1) and, through the activity cursor,
    /// a remote link refresh (Source 2) but no second comment.
    #[actix_web::test]
    #[serial_test::serial(jira_capture)]
    async fn inbound_event_and_activity_give_one_comment_each() {
        let flow = Flow::new().await;
        sqlx::query("UPDATE jira_integrations SET writeback_enabled = TRUE WHERE id = $1")
            .bind(flow.integration_id)
            .execute(&flow.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO jira_writeback_cursor (id, last_created_at) \
             VALUES (TRUE, now() - interval '1 minute') \
             ON CONFLICT (id) DO UPDATE SET last_created_at = EXCLUDED.last_created_at",
        )
        .execute(&flow.pool)
        .await
        .unwrap();

        // Approve, then deploy: two Jira events.
        let (status, _) = flow.send(webhook("Ready for Release", "1")).await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = flow.send(webhook("Done", "2")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["results"][0]["to"], "DEPLOYED");

        crate::scheduler::JiraWritebackCapture::new(
            flow.pool.clone(),
            std::time::Duration::from_secs(5),
        )
        .with_lag(Duration::zero())
        .run_once()
        .await
        .unwrap();

        let jobs: Vec<(String, String)> = sqlx::query_as(
            "SELECT kind, dedupe_key FROM jira_outbound_jobs WHERE integration_id = $1",
        )
        .bind(flow.integration_id)
        .fetch_all(&flow.pool)
        .await
        .unwrap();
        let comments: Vec<&(String, String)> =
            jobs.iter().filter(|job| job.0 == "comment").collect();
        assert_eq!(
            comments.len(),
            2,
            "one per event, none from the activity rows: {jobs:?}"
        );
        assert!(comments.iter().all(|job| job.1.starts_with("event:")));
        let links = jobs.iter().filter(|job| job.0 == "remote_link").count();
        assert_eq!(links, 1, "one pending remote link refresh: {jobs:?}");

        let text: String = sqlx::query_scalar(
            "SELECT payload::text FROM jira_outbound_jobs \
             WHERE integration_id = $1 AND kind = 'comment' ORDER BY created_at DESC LIMIT 1",
        )
        .bind(flow.integration_id)
        .fetch_one(&flow.pool)
        .await
        .unwrap();
        assert!(
            text.contains("Jira status 'Done'") && text.contains("deploy applied"),
            "{text}"
        );

        flow.cleanup().await;
    }
}
