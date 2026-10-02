//! AI judgment endpoints: subsystem status (AI-00) and per-team settings (AI-01).

use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, post, put, web};
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::ai::AiFeature;
use crate::database::ai::{StoredTeamAiSettings, TeamAiSettings, TeamAiSettingsRepository};
use crate::database::feature::FeatureRepository;
use crate::judgment::flag_kind;
use crate::judgment::justification::{self, MAX_REASON_CHARS, ReasonKind};
use crate::judgment::{AiRuntime, JudgmentKind, SubjectType};
use crate::model::FlagKind;
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

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureSuggestionsRequest {
    pub key: String,
    pub description: Option<String>,
    pub purpose: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KindSuggestionResponse {
    /// `release`, `experiment`, `ops`, `permission`, or `config`.
    pub value: FlagKind,
    /// Probability per option, including `unknown`.
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagSuggestionResponse {
    pub tag: String,
    pub probability: f64,
}

/// `{ "available": false }` when suggestions cannot run. `kind` and `tags` are
/// present only when `available` is true; `kind` is null when the model is not
/// sure.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureSuggestionsResponse {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<KindSuggestionResponse>, nullable = true)]
    pub kind: Option<Option<KindSuggestionResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<TagSuggestionResponse>>,
}

impl FeatureSuggestionsResponse {
    fn unavailable() -> Self {
        Self {
            available: false,
            kind: None,
            tags: None,
        }
    }
}

const MAX_SUGGESTION_KEY_CHARS: usize = 255;
const MAX_SUGGESTION_BODY_TAGS: usize = 50;

fn normalized_tags(tags: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/ai/feature-suggestions",
    request_body = FeatureSuggestionsRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Kind and tag suggestions, or `available: false` when they cannot run", body = FeatureSuggestionsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[post("/teams/{team_id}/ai/feature-suggestions")]
pub(crate) async fn suggest_feature_details(
    runtime: web::Data<AiRuntime>,
    settings: web::Data<Box<dyn TeamAiSettingsRepository>>,
    features: web::Data<Box<dyn FeatureRepository>>,
    team_id: web::Path<String>,
    payload: web::Json<FeatureSuggestionsRequest>,
) -> Result<impl Responder, RestError> {
    let team_id = parse_team_id(&team_id)?;
    let key = payload.key.trim();
    if key.is_empty() || key.chars().count() > MAX_SUGGESTION_KEY_CHARS {
        return Err(RestError::invalid_input(format!(
            "key must be 1 to {MAX_SUGGESTION_KEY_CHARS} characters"
        )));
    }
    let tags = normalized_tags(payload.tags.as_deref().unwrap_or_default());
    if tags.len() > MAX_SUGGESTION_BODY_TAGS {
        return Err(RestError::invalid_input(format!(
            "at most {MAX_SUGGESTION_BODY_TAGS} tags are accepted"
        )));
    }

    let Some(client) = runtime.client.as_ref() else {
        return Ok(HttpResponse::Ok().json(FeatureSuggestionsResponse::unavailable()));
    };
    let enabled = match settings.get(team_id).await {
        Ok(stored) => stored.settings.is_enabled(AiFeature::FlagKind),
        Err(err) => {
            warn!("Could not read AI settings for team {team_id}: {err}");
            false
        }
    };
    if !enabled {
        return Ok(HttpResponse::Ok().json(FeatureSuggestionsResponse::unavailable()));
    }

    let candidates = match features
        .get_team_tag_candidates(team_id, tags.clone(), flag_kind::MAX_TAG_CANDIDATES)
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            warn!("Could not read tag candidates for team {team_id}: {err}");
            return Ok(HttpResponse::Ok().json(FeatureSuggestionsResponse::unavailable()));
        }
    };

    let input = flag_kind::SuggestionInput {
        key: key.to_string(),
        description: payload.description.clone(),
        purpose: payload.purpose.clone(),
        tags,
    };
    let parts = flag_kind::suggestion_parts(&input, &candidates);
    let response = match client.evaluate(parts.state, parts.questions).await {
        Ok(response) => FeatureSuggestionsResponse {
            available: true,
            kind: Some(flag_kind::classify(&response.answers).map(|answer| {
                KindSuggestionResponse {
                    value: answer.value,
                    probabilities: answer.probabilities,
                    confidence: answer.confidence,
                }
            })),
            tags: Some(
                flag_kind::suggested_tags(&response.answers, &candidates)
                    .into_iter()
                    .map(|item| TagSuggestionResponse {
                        tag: item.tag,
                        probability: item.probability,
                    })
                    .collect(),
            ),
        },
        Err(err) => {
            warn!("Feature suggestions failed: {}", err.log_label());
            FeatureSuggestionsResponse::unavailable()
        }
    };
    Ok(HttpResponse::Ok().json(response))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlagKindBackfillResponse {
    /// Judgments queued by this call.
    pub queued: u64,
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/ai/flag-kind/backfill",
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Judgments queued; 0 when AI is off for the team", body = FlagKindBackfillResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[post("/teams/{team_id}/ai/flag-kind/backfill")]
pub(crate) async fn backfill_flag_kind(
    runtime: web::Data<AiRuntime>,
    features: web::Data<Box<dyn FeatureRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    ensure_admin(&req)?;
    let team_id = parse_team_id(&team_id)?;
    let Some(service) = runtime.judgments.as_ref() else {
        return Ok(HttpResponse::Ok().json(FlagKindBackfillResponse { queued: 0 }));
    };
    if !service
        .team_enabled(team_id, JudgmentKind::FlagKind.feature())
        .await
    {
        return Ok(HttpResponse::Ok().json(FlagKindBackfillResponse { queued: 0 }));
    }
    let pending = features
        .get_features_needing_flag_kind(team_id, flag_kind::BACKFILL_LIMIT)
        .await
        .map_err(RestError::from)?;
    let queued = flag_kind::backfill(service, team_id, &pending).await;
    Ok(HttpResponse::Ok().json(FlagKindBackfillResponse {
        queued: queued as u64,
    }))
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

/// Queues a flag kind classification after a feature was created or edited. A
/// no-op without an AI runtime (some apps and tests register none); never fails
/// the request.
pub(crate) async fn record_flag_kind(
    ai: &Option<web::Data<AiRuntime>>,
    team_id: Uuid,
    feature: &crate::model::Feature,
) {
    flag_kind::record_flag_kind(
        ai.as_ref().and_then(|ai| ai.judgments.as_ref()),
        team_id,
        feature,
    )
    .await;
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_ai_status)
        .service(get_team_ai_settings)
        .service(update_team_ai_settings)
        .service(check_justification)
        .service(suggest_feature_details)
        .service(backfill_flag_kind);
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

    mod flag_kind_endpoints {
        use std::collections::BTreeMap;
        use std::sync::Mutex;

        use super::*;
        use crate::database::feature::{FeatureRepository, MockFeatureRepository};
        use crate::judgment::flag_kind::test_support::recording_service;
        use crate::judgment::types::{Answer, Answers, ChoiceAnswer, SystemOneResponse, Usage};

        fn settings(flag_kind: bool) -> MockTeamAiSettingsRepository {
            let mut settings = MockTeamAiSettingsRepository::new();
            settings.expect_get().returning(move |_| {
                Ok(StoredTeamAiSettings {
                    settings: TeamAiSettings {
                        flag_kind,
                        ..TeamAiSettings::default()
                    },
                    ..StoredTeamAiSettings::default()
                })
            });
            settings
        }

        fn response(kind: &str, confidence: f64, tags: &[f64]) -> SystemOneResponse {
            let mut answers = BTreeMap::from([(
                "kind".to_string(),
                Answer::Choice(ChoiceAnswer {
                    choice: kind.to_string(),
                    probabilities: BTreeMap::from([(kind.to_string(), confidence)]),
                    confidence,
                }),
            )]);
            for (index, probability) in tags.iter().enumerate() {
                answers.insert(format!("t{index}"), Answer::Noul { noul: *probability });
            }
            SystemOneResponse {
                model: "jev-1.13.0".to_string(),
                answers: Answers(answers),
                usage: Usage::default(),
            }
        }

        async fn suggest(
            client: Option<MockJudgmentClient>,
            settings: MockTeamAiSettingsRepository,
            features: MockFeatureRepository,
            body: Value,
        ) -> (StatusCode, Value) {
            let client = client.map(|client| Arc::new(client) as Arc<dyn JudgmentClient>);
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(AiRuntime::new(client, "jev-1.13.0")))
                    .app_data(web::Data::new(
                        Box::new(settings) as Box<dyn TeamAiSettingsRepository>
                    ))
                    .app_data(web::Data::new(
                        Box::new(features) as Box<dyn FeatureRepository>
                    ))
                    .service(web::scope("/api/v1").configure(super::super::configure)),
            )
            .await;
            let req = test::TestRequest::post()
                .uri(&format!(
                    "/api/v1/teams/{}/ai/feature-suggestions",
                    Uuid::new_v4()
                ))
                .set_json(body)
                .to_request();
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }

        fn untouched_client() -> MockJudgmentClient {
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            client
        }

        #[actix_web::test]
        async fn off_returns_available_false_without_reading_or_calling() {
            let mut features = MockFeatureRepository::new();
            features.expect_get_team_tag_candidates().times(0);
            let (status, body) = suggest(
                Some(untouched_client()),
                settings(false),
                features,
                json!({ "key": "kill-checkout" }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn no_client_returns_available_false() {
            let (status, body) = suggest(
                None,
                settings(true),
                MockFeatureRepository::new(),
                json!({ "key": "kill-checkout" }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn client_failure_returns_available_false() {
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .returning(|_, _| Err(crate::judgment::client::JudgmentError::Timeout));
            let mut features = MockFeatureRepository::new();
            features
                .expect_get_team_tag_candidates()
                .returning(|_, _, _| Ok(vec![]));
            let (status, body) = suggest(
                Some(client),
                settings(true),
                features,
                json!({ "key": "kill-checkout" }),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "available": false }));
        }

        #[actix_web::test]
        async fn maps_answers_and_excludes_the_tags_already_on_the_feature() {
            let asked = Arc::new(Mutex::new(None));
            let asked_by_repo = asked.clone();
            let mut features = MockFeatureRepository::new();
            features
                .expect_get_team_tag_candidates()
                .returning(move |_, exclude, limit| {
                    *asked_by_repo.lock().unwrap() = Some((exclude, limit));
                    Ok(vec!["billing".into(), "beta".into(), "ops".into()])
                });
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .withf(|state, questions| {
                    questions.len() == 4
                        && state["feature"]["key"] == "kill-checkout"
                        && state["feature"]["tags"] == json!(["payments"])
                })
                .times(1)
                .returning(|_, _| Ok(response("ops", 0.9, &[0.95, 0.3, 0.6])));

            let (status, body) = suggest(
                Some(client),
                settings(true),
                features,
                json!({
                    "key": " kill-checkout ",
                    "description": "Kill switch",
                    "tags": ["Payments", " payments "]
                }),
            )
            .await;

            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body,
                json!({
                    "available": true,
                    "kind": {
                        "value": "ops",
                        "probabilities": { "ops": 0.9 },
                        "confidence": 0.9,
                    },
                    "tags": [
                        { "tag": "billing", "probability": 0.95 },
                        { "tag": "ops", "probability": 0.6 },
                    ],
                })
            );
            assert_eq!(
                asked.lock().unwrap().clone(),
                Some((vec!["payments".to_string()], 50))
            );
        }

        #[actix_web::test]
        async fn an_unsure_kind_is_null_but_tags_still_come_back() {
            let mut features = MockFeatureRepository::new();
            features
                .expect_get_team_tag_candidates()
                .returning(|_, _, _| Ok(vec!["billing".into()]));
            let mut client = MockJudgmentClient::new();
            client
                .expect_evaluate()
                .returning(|_, _| Ok(response("unknown", 0.95, &[0.8])));
            let (_, body) = suggest(
                Some(client),
                settings(true),
                features,
                json!({ "key": "x1" }),
            )
            .await;
            assert_eq!(body["available"], true);
            assert_eq!(body["kind"], Value::Null);
            assert_eq!(
                body["tags"],
                json!([{ "tag": "billing", "probability": 0.8 }])
            );
        }

        #[actix_web::test]
        async fn a_blank_key_is_rejected() {
            let (status, _) = suggest(
                None,
                settings(true),
                MockFeatureRepository::new(),
                json!({ "key": "   " }),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        async fn backfill(
            admin: bool,
            runtime: AiRuntime,
            features: MockFeatureRepository,
        ) -> (StatusCode, Value) {
            let app = test::init_service(
                App::new()
                    .app_data(web::Data::new(runtime))
                    .app_data(web::Data::new(
                        Box::new(features) as Box<dyn FeatureRepository>
                    ))
                    .service(web::scope("/api/v1").configure(super::super::configure)),
            )
            .await;
            let req = test::TestRequest::post()
                .uri(&format!(
                    "/api/v1/teams/{}/ai/flag-kind/backfill",
                    Uuid::new_v4()
                ))
                .to_request();
            req.extensions_mut().insert(jwt(admin));
            let resp = test::call_service(&app, req).await;
            let status = resp.status();
            let bytes = test::read_body(resp).await;
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }

        fn runtime(on: bool) -> (AiRuntime, crate::judgment::flag_kind::test_support::Upserts) {
            let (service, upserts) = recording_service(on, vec![]);
            let client: Arc<dyn JudgmentClient> = Arc::new(MockJudgmentClient::new());
            (
                AiRuntime::new(Some(client), "jev-1.13.0").with_judgments(Some(service)),
                upserts,
            )
        }

        fn feature(key: &str) -> crate::database::entity::Feature {
            crate::judgment::flag_kind::test_support::entity_feature(key)
        }

        #[actix_web::test]
        async fn backfill_requires_an_admin() {
            let (runtime, upserts) = runtime(true);
            let mut features = MockFeatureRepository::new();
            features.expect_get_features_needing_flag_kind().times(0);
            let (status, _) = backfill(false, runtime, features).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert!(upserts.lock().unwrap().is_empty());
        }

        #[actix_web::test]
        async fn backfill_queues_each_unclassified_feature() {
            let (runtime, upserts) = runtime(true);
            let mut features = MockFeatureRepository::new();
            features
                .expect_get_features_needing_flag_kind()
                .withf(|_, limit| *limit == 500)
                .times(1)
                .returning(|_, _| Ok(vec![feature("a"), feature("b"), feature("c")]));
            let (status, body) = backfill(true, runtime, features).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "queued": 3 }));
            assert_eq!(upserts.lock().unwrap().len(), 3);
        }

        #[actix_web::test]
        async fn backfill_is_a_no_op_with_the_team_toggle_off_or_no_service() {
            let (runtime_off, upserts) = runtime(false);
            let mut features = MockFeatureRepository::new();
            features.expect_get_features_needing_flag_kind().times(0);
            let (status, body) = backfill(true, runtime_off, features).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({ "queued": 0 }));
            assert!(upserts.lock().unwrap().is_empty());

            let mut features = MockFeatureRepository::new();
            features.expect_get_features_needing_flag_kind().times(0);
            let (_, body) = backfill(true, AiRuntime::new(None, "jev-1.13.0"), features).await;
            assert_eq!(body, json!({ "queued": 0 }));
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
