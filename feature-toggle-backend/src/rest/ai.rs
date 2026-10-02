//! AI judgment endpoints: subsystem status (AI-00) and per-team settings (AI-01).

use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, post, put, web};
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::ai::AiFeature;
use crate::database::ai::{StoredTeamAiSettings, TeamAiSettings, TeamAiSettingsRepository};
use crate::judgment::justification::{self, MAX_REASON_CHARS, ReasonKind};
use crate::judgment::{AiRuntime, SubjectType};
use crate::rest::error::RestError;
use crate::utils::activity_logger::{activity_types, log_team_activity};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiStatusResponse {
    /// True when the server has a TypeSafe API key.
    pub available: bool,
    /// Pinned model id; null when unavailable.
    pub model: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ai/status",
    responses(
        (status = 200, description = "AI judgment subsystem status", body = AiStatusResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[get("/ai/status")]
pub(crate) async fn get_ai_status(runtime: web::Data<AiRuntime>) -> impl Responder {
    HttpResponse::Ok().json(AiStatusResponse {
        available: runtime.available(),
        model: runtime.status_model(),
    })
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamAiSettingsResponse {
    /// False when the server has no TypeSafe API key; toggles then have no effect.
    pub available: bool,
    pub approval_risk: bool,
    pub justification_check: bool,
    pub flag_kind: bool,
    pub nl_search: bool,
    /// RFC 3339; null when the team never saved settings.
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamAiSettingsRequest {
    pub approval_risk: bool,
    pub justification_check: bool,
    pub flag_kind: bool,
    pub nl_search: bool,
}

impl From<UpdateTeamAiSettingsRequest> for TeamAiSettings {
    fn from(value: UpdateTeamAiSettingsRequest) -> Self {
        Self {
            approval_risk: value.approval_risk,
            justification_check: value.justification_check,
            flag_kind: value.flag_kind,
            nl_search: value.nl_search,
        }
    }
}

fn settings_response(runtime: &AiRuntime, stored: &StoredTeamAiSettings) -> TeamAiSettingsResponse {
    TeamAiSettingsResponse {
        available: runtime.available(),
        approval_risk: stored.settings.approval_risk,
        justification_check: stored.settings.justification_check,
        flag_kind: stored.settings.flag_kind,
        nl_search: stored.settings.nl_search,
        updated_at: stored.updated_at.map(|at| at.to_rfc3339()),
    }
}

fn parse_team_id(raw: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(raw).map_err(|_| RestError::invalid_input("Invalid team_id"))
}

fn ensure_admin(req: &HttpRequest) -> Result<JwtUser, RestError> {
    let jwt = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    if !jwt.is_admin {
        return Err(RestError::forbidden(
            "Only system administrators can manage AI settings",
        ));
    }
    Ok(jwt)
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/ai-settings",
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Team AI settings", body = TeamAiSettingsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[get("/teams/{team_id}/ai-settings")]
pub(crate) async fn get_team_ai_settings(
    runtime: web::Data<AiRuntime>,
    settings: web::Data<Box<dyn TeamAiSettingsRepository>>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_id = parse_team_id(&team_id)?;
    let stored = settings.get(team_id).await.map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(settings_response(&runtime, &stored)))
}

#[utoipa::path(
    put,
    path = "/api/v1/teams/{team_id}/ai-settings",
    request_body = UpdateTeamAiSettingsRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Updated team AI settings", body = TeamAiSettingsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Team not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[put("/teams/{team_id}/ai-settings")]
pub(crate) async fn update_team_ai_settings(
    runtime: web::Data<AiRuntime>,
    settings: web::Data<Box<dyn TeamAiSettingsRepository>>,
    activity: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<UpdateTeamAiSettingsRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = ensure_admin(&req)?;
    let team_id = parse_team_id(&team_id)?;
    let previous = settings.get(team_id).await.map_err(RestError::from)?;
    let stored = settings
        .upsert(team_id, payload.into_inner().into(), Some(jwt.id))
        .await
        .map_err(RestError::from)?;

    if let Err(err) = log_team_activity(
        activity.get_ref(),
        activity_types::AI_SETTINGS_UPDATED,
        &team_id.to_string(),
        Some(jwt.id),
        Some(jwt.username.clone()),
        "AI settings updated".to_string(),
        Some(json!({ "old": previous.settings, "new": stored.settings })),
    )
    .await
    {
        warn!("Could not record AI settings activity for team {team_id}: {err}");
    }

    Ok(HttpResponse::Ok().json(settings_response(&runtime, &stored)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JustificationCheckRequest {
    pub reason_kind: ReasonKind,
    /// 1 to 1000 characters.
    pub reason: String,
    pub feature_key: Option<String>,
}

/// `{ "available": false }` when the check cannot run. Every other field is
/// present only when `available` is true.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JustificationCheckResponse {
    pub available: bool,
    /// `ok` or `weak`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// How likely the reason names a real cause, from 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<String>>,
    /// `rule` or `model`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl JustificationCheckResponse {
    fn unavailable() -> Self {
        Self {
            available: false,
            verdict: None,
            probability: None,
            hints: None,
            source: None,
        }
    }

    fn rule_pass() -> Self {
        Self {
            available: true,
            verdict: Some("ok".to_string()),
            probability: Some(1.0),
            hints: Some(Vec::new()),
            source: Some("rule".to_string()),
        }
    }

    fn from_derived(derived: &serde_json::Value) -> Self {
        Self {
            available: true,
            verdict: derived
                .get("verdict")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            probability: derived
                .get("probability")
                .and_then(serde_json::Value::as_f64),
            hints: derived.get("hints").and_then(|hints| {
                hints.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(|hint| hint.as_str().map(str::to_string))
                        .collect()
                })
            }),
            source: derived
                .get("source")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/ai/justification-check",
    request_body = JustificationCheckRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Verdict, or `available: false` when the check cannot run", body = JustificationCheckResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[post("/teams/{team_id}/ai/justification-check")]
pub(crate) async fn check_justification(
    runtime: web::Data<AiRuntime>,
    settings: web::Data<Box<dyn TeamAiSettingsRepository>>,
    team_id: web::Path<String>,
    payload: web::Json<JustificationCheckRequest>,
) -> Result<impl Responder, RestError> {
    let team_id = parse_team_id(&team_id)?;
    let reason = payload.reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
        return Err(RestError::invalid_input(format!(
            "reason must be 1 to {MAX_REASON_CHARS} characters"
        )));
    }

    let Some(client) = runtime.client.as_ref() else {
        return Ok(HttpResponse::Ok().json(JustificationCheckResponse::unavailable()));
    };
    let enabled = match settings.get(team_id).await {
        Ok(stored) => stored.settings.is_enabled(AiFeature::JustificationCheck),
        Err(err) => {
            warn!("Could not read AI settings for team {team_id}: {err}");
            false
        }
    };
    if !enabled {
        return Ok(HttpResponse::Ok().json(JustificationCheckResponse::unavailable()));
    }

    if justification::rule_check(reason) {
        return Ok(HttpResponse::Ok().json(JustificationCheckResponse::rule_pass()));
    }

    let parts = justification::build(payload.reason_kind, payload.feature_key.as_deref(), reason);
    let response = match client.evaluate(parts.state, parts.questions).await {
        Ok(response) => {
            JustificationCheckResponse::from_derived(&justification::derive(&response.answers))
        }
        Err(err) => {
            warn!("Justification check failed: {}", err.log_label());
            JustificationCheckResponse::unavailable()
        }
    };
    Ok(HttpResponse::Ok().json(response))
}

/// Hands a free-text reason to the justification check after the user's change
/// committed. A no-op without an AI runtime (some apps and tests register none);
/// never fails the request.
pub(crate) async fn record_reason(
    ai: &Option<web::Data<AiRuntime>>,
    team_id: Uuid,
    subject_type: SubjectType,
    subject_id: Uuid,
    kind: ReasonKind,
    reason: &str,
    feature_key: Option<&str>,
) {
    justification::record_justification(
        ai.as_ref().and_then(|ai| ai.judgments.as_ref()),
        team_id,
        subject_type,
        subject_id,
        kind,
        reason,
        feature_key,
    )
    .await;
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_ai_status)
        .service(get_team_ai_settings)
        .service(update_team_ai_settings)
        .service(check_justification);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix_web::{App, test, web};
    use serde_json::{Value, json};

    use crate::judgment::client::MockJudgmentClient;
    use crate::judgment::{AiRuntime, JudgmentClient};

    use actix_web::HttpMessage;
    use actix_web::http::StatusCode;
    use chrono::Utc;
    use uuid::Uuid;

    use crate::JwtUser;
    use crate::database::activity_log::{ActivityLogRepository, MockActivityLogRepository};
    use crate::database::ai::{
        MockTeamAiSettingsRepository, StoredTeamAiSettings, TeamAiSettings,
        TeamAiSettingsRepository,
    };

    fn jwt(is_admin: bool) -> JwtUser {
        JwtUser {
            id: Uuid::new_v4(),
            username: if is_admin { "admin" } else { "dev" }.to_string(),
            is_admin,
            roles: vec![],
            team_id: None,
            token_hash: "hash".to_string(),
        }
    }

    macro_rules! settings_app {
        ($settings:expr, $activity:expr) => {
            test::init_service(
                App::new()
                    .app_data(web::Data::new(AiRuntime::new(None, "jev-1.13.0")))
                    .app_data(web::Data::new(
                        Box::new($settings) as Box<dyn TeamAiSettingsRepository>
                    ))
                    .app_data(web::Data::new(
                        Box::new($activity) as Box<dyn ActivityLogRepository>
                    ))
                    .service(web::scope("/api/v1").configure(super::configure)),
            )
            .await
        };
    }

    #[actix_web::test]
    async fn get_without_row_returns_all_off() {
        let team_id = Uuid::new_v4();
        let mut settings = MockTeamAiSettingsRepository::new();
        settings
            .expect_get()
            .withf(move |id| *id == team_id)
            .returning(|_| Ok(StoredTeamAiSettings::default()));
        let app = settings_app!(settings, MockActivityLogRepository::new());

        let req = test::TestRequest::get()
            .uri(&format!("/api/v1/teams/{team_id}/ai-settings"))
            .to_request();
        req.extensions_mut().insert(jwt(false));
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(
            body,
            json!({
                "available": false,
                "approvalRisk": false,
                "justificationCheck": false,
                "flagKind": false,
                "nlSearch": false,
                "updatedAt": null
            })
        );
    }

    #[actix_web::test]
    async fn get_with_bad_team_id_returns_400() {
        let app = settings_app!(
            MockTeamAiSettingsRepository::new(),
            MockActivityLogRepository::new()
        );
        let req = test::TestRequest::get()
            .uri("/api/v1/teams/not-a-uuid/ai-settings")
            .to_request();
        req.extensions_mut().insert(jwt(true));
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn put_as_non_admin_returns_403() {
        let mut settings = MockTeamAiSettingsRepository::new();
        settings.expect_upsert().times(0);
        let app = settings_app!(settings, MockActivityLogRepository::new());
        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/teams/{}/ai-settings", Uuid::new_v4()))
            .set_json(json!({
                "approvalRisk": true, "justificationCheck": false,
                "flagKind": false, "nlSearch": false
            }))
            .to_request();
        req.extensions_mut().insert(jwt(false));
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn put_as_admin_persists_and_logs_activity() {
        let team_id = Uuid::new_v4();
        let admin = jwt(true);
        let admin_id = admin.id;
        let now = Utc::now();
        let mut settings = MockTeamAiSettingsRepository::new();
        settings
            .expect_get()
            .returning(|_| Ok(StoredTeamAiSettings::default()));
        settings
            .expect_upsert()
            .withf(move |id, wanted, by| {
                *id == team_id
                    && *by == Some(admin_id)
                    && *wanted
                        == TeamAiSettings {
                            approval_risk: true,
                            justification_check: false,
                            flag_kind: true,
                            nl_search: false,
                        }
            })
            .times(1)
            .returning(move |_, wanted, by| {
                Ok(StoredTeamAiSettings {
                    settings: wanted,
                    updated_at: Some(now),
                    updated_by: by,
                })
            });
        let mut activity = MockActivityLogRepository::new();
        activity
            .expect_create_activity()
            .withf(move |entry| {
                entry.activity_type == "ai_settings_updated"
                    && entry.entity_type == "team"
                    && entry.entity_id == team_id.to_string()
                    && entry.metadata.as_ref().is_some_and(|m| {
                        m["old"]["approval_risk"] == false && m["new"]["approval_risk"] == true
                    })
            })
            .times(1)
            .returning(|_| Err(sqlx::Error::RowNotFound));
        let app = settings_app!(settings, activity);

        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/teams/{team_id}/ai-settings"))
            .set_json(json!({
                "approvalRisk": true, "justificationCheck": false,
                "flagKind": true, "nlSearch": false
            }))
            .to_request();
        req.extensions_mut().insert(admin);
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["approvalRisk"], true);
        assert_eq!(body["flagKind"], true);
        assert_eq!(body["nlSearch"], false);
        assert_eq!(body["available"], false);
        assert_eq!(body["updatedAt"], now.to_rfc3339());
    }

    #[actix_web::test]
    async fn put_unknown_team_returns_404() {
        let team_id = Uuid::new_v4();
        let mut settings = MockTeamAiSettingsRepository::new();
        settings
            .expect_get()
            .returning(|_| Ok(StoredTeamAiSettings::default()));
        settings
            .expect_upsert()
            .times(1)
            .returning(move |_, _, _| Err(crate::Error::NotFound(team_id)));
        let mut activity = MockActivityLogRepository::new();
        activity.expect_create_activity().times(0);
        let app = settings_app!(settings, activity);

        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/teams/{team_id}/ai-settings"))
            .set_json(json!({
                "approvalRisk": false, "justificationCheck": false,
                "flagKind": false, "nlSearch": false
            }))
            .to_request();
        req.extensions_mut().insert(jwt(true));
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    mod justification_check {
        use super::*;
        use crate::judgment::client::JudgmentError;
        use crate::judgment::types::{Answer, Answers, SystemOneResponse, Usage};
        use std::collections::BTreeMap;

        fn settings(on: bool) -> MockTeamAiSettingsRepository {
            let mut settings = MockTeamAiSettingsRepository::new();
            settings.expect_get().returning(move |_| {
                Ok(StoredTeamAiSettings {
                    settings: TeamAiSettings {
                        justification_check: on,
                        ..TeamAiSettings::default()
                    },
                    ..StoredTeamAiSettings::default()
                })
            });
            settings
        }

        fn model_answers(concrete: f64, placeholder: f64) -> SystemOneResponse {
            SystemOneResponse {
                model: "jev-1.13.0".to_string(),
                answers: Answers(BTreeMap::from([
                    (
                        "concrete_cause".to_string(),
                        Answer::Noul { noul: concrete },
                    ),
                    (
                        "placeholder_text".to_string(),
                        Answer::Noul { noul: placeholder },
                    ),
                ])),
                usage: Usage::default(),
            }
        }

        async fn post(
            client: Option<MockJudgmentClient>,
            settings: MockTeamAiSettingsRepository,
            body: Value,
        ) -> (StatusCode, Value) {
            let client = client.map(|client| Arc::new(client) as Arc<dyn JudgmentClient>);
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(AiRuntime::new(client, "jev-1.13.0")))
                    .app_data(web::Data::new(
                        Box::new(settings) as Box<dyn TeamAiSettingsRepository>
                    ))
                    .service(web::scope("/api/v1").configure(super::super::configure)),
            )
            .await;
            let req = test::TestRequest::post()
                .uri(&format!(
                    "/api/v1/teams/{}/ai/justification-check",
                    Uuid::new_v4()
                ))
                .set_json(body)
                .to_request();
            req.extensions_mut().insert(jwt(false));
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }

        fn request(reason: &str) -> Value {
            json!({ "reasonKind": "emergency_disable", "reason": reason, "featureKey": "checkout-v2" })
        }

        #[actix_web::test]
        async fn subsystem_off_is_unavailable() {
            let (status, body) = post(None, settings(true), request("urgent")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn team_setting_off_is_unavailable_without_a_call() {
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            let (status, body) = post(Some(client), settings(false), request("urgent")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn rule_pass_answers_ok_without_calling_the_api() {
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            let (status, body) = post(
                Some(client),
                settings(true),
                request("Checkout 500s, see INC12345"),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body,
                json!({
                    "available": true, "verdict": "ok", "probability": 1.0,
                    "hints": [], "source": "rule"
                })
            );
        }

        #[actix_web::test]
        async fn model_path_maps_answers_to_a_verdict() {
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .withf(|state, questions| {
                    state["reason"] == "urgent"
                        && state["feature_key"] == "checkout-v2"
                        && state["action"]
                            == "Turn off a feature flag in an emergency (kill switch)"
                        && questions.len() == 2
                })
                .times(1)
                .returning(|_, _| Ok(model_answers(0.1, 0.9)));
            let (status, body) = post(Some(client), settings(true), request("urgent")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body,
                json!({
                    "available": true, "verdict": "weak", "probability": 0.1,
                    "hints": [
                        "This looks like placeholder text.",
                        "Add the cause: an incident, ticket, bug, or customer impact."
                    ],
                    "source": "model"
                })
            );
        }

        #[actix_web::test]
        async fn model_path_can_answer_ok() {
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .times(1)
                .returning(|_, _| Ok(model_answers(0.92, 0.05)));
            let (_, body) = post(
                Some(client),
                settings(true),
                request("Disabling: checkout 500s since 14:00"),
            )
            .await;
            assert_eq!(body["verdict"], "ok");
            assert_eq!(body["hints"], json!([]));
            assert_eq!(body["source"], "model");
        }

        #[actix_web::test]
        async fn client_error_is_unavailable_not_a_server_error() {
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .times(1)
                .returning(|_, _| Err(JudgmentError::Http(500, "boom".into())));
            let (status, body) = post(Some(client), settings(true), request("urgent")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn settings_read_error_is_unavailable() {
            let mut failing = MockTeamAiSettingsRepository::new();
            failing
                .expect_get()
                .returning(|_| Err(crate::Error::InvalidInput("db down".into())));
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            let (status, body) = post(Some(client), failing, request("urgent")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn reason_length_is_validated() {
            for reason in ["x".repeat(1001), String::new(), "   ".to_string()] {
                let mut client = MockJudgmentClient::new();
                client.expect_evaluate().times(0);
                let (status, _) = post(Some(client), settings(true), request(&reason)).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "reason {:?}", reason.len());
            }
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .times(1)
                .returning(|_, _| Ok(model_answers(0.9, 0.0)));
            let (status, _) = post(Some(client), settings(true), request(&"x".repeat(1000))).await;
            assert_eq!(status, StatusCode::OK);
        }

        #[actix_web::test]
        async fn unknown_reason_kind_is_rejected() {
            let (status, _) = post(
                None,
                settings(true),
                json!({ "reasonKind": "nope", "reason": "urgent" }),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        #[actix_web::test]
        async fn bad_team_id_is_rejected() {
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(AiRuntime::new(None, "jev-1.13.0")))
                    .app_data(web::Data::new(
                        Box::new(settings(true)) as Box<dyn TeamAiSettingsRepository>
                    ))
                    .service(web::scope("/api/v1").configure(super::super::configure)),
            )
            .await;
            let req = test::TestRequest::post()
                .uri("/api/v1/teams/not-a-uuid/ai/justification-check")
                .set_json(request("urgent"))
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[actix_web::test]
    async fn status_without_key_is_unavailable() {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AiRuntime::new(None, "jev-1.13.0")))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/api/v1/ai/status")
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body, json!({ "available": false, "model": null }));
    }

    #[actix_web::test]
    async fn status_with_client_reports_model() {
        let client: Arc<dyn JudgmentClient> = Arc::new(MockJudgmentClient::new());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AiRuntime::new(Some(client), "jev-1.13.0")))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/api/v1/ai/status")
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body, json!({ "available": true, "model": "jev-1.13.0" }));
    }
}
