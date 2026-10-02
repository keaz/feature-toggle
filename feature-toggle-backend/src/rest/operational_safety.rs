use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, patch, post, web};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::judgment::justification::ReasonKind;
use crate::judgment::{AiRuntime, SubjectType};
use crate::logic::authorization::RoleAuthorizer;
use crate::logic::policy::{PolicyActor, PolicyError};
use crate::rest::ai::record_reason;
use crate::rest::error::RestError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct FreezeWindowRow {
    pub id: Uuid,
    pub team_id: Uuid,
    pub name: String,
    pub environment_id: Option<Uuid>,
    pub environment_type: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub recurrence: String,
    pub reason: Option<String>,
    pub active: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ScheduledChangeRow {
    pub id: Uuid,
    pub team_id: Uuid,
    pub feature_id: Uuid,
    pub stage_id: Option<Uuid>,
    pub environment_id: Option<Uuid>,
    pub action: String,
    pub requested_status: Option<String>,
    pub payload: serde_json::Value,
    pub reason: String,
    pub scheduled_at: DateTime<Utc>,
    pub timezone: String,
    pub status: String,
    pub requested_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub executed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub result_message: Option<String>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FreezeRecurrence {
    None,
    Daily,
    Weekly,
}

impl FreezeRecurrence {
    fn as_str(self) -> &'static str {
        match self {
            FreezeRecurrence::None => "NONE",
            FreezeRecurrence::Daily => "DAILY",
            FreezeRecurrence::Weekly => "WEEKLY",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScheduledChangeAction {
    EnableFeature,
    DisableFeature,
    StageChange,
    ArchiveFeature,
}

impl ScheduledChangeAction {
    fn as_str(self) -> &'static str {
        match self {
            ScheduledChangeAction::EnableFeature => "ENABLE_FEATURE",
            ScheduledChangeAction::DisableFeature => "DISABLE_FEATURE",
            ScheduledChangeAction::StageChange => "STAGE_CHANGE",
            ScheduledChangeAction::ArchiveFeature => "ARCHIVE_FEATURE",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScheduledChangeStatus {
    Pending,
    Executing,
    Executed,
    Cancelled,
    Failed,
    Blocked,
}

impl ScheduledChangeStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ScheduledChangeStatus::Pending => "PENDING",
            ScheduledChangeStatus::Executing => "EXECUTING",
            ScheduledChangeStatus::Executed => "EXECUTED",
            ScheduledChangeStatus::Cancelled => "CANCELLED",
            ScheduledChangeStatus::Failed => "FAILED",
            ScheduledChangeStatus::Blocked => "BLOCKED",
        }
    }
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FreezeWindowResponse {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub environment_id: Option<String>,
    pub environment_type: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub recurrence: String,
    pub reason: Option<String>,
    pub active: bool,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FreezeWindowsResponse {
    pub items: Vec<FreezeWindowResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateFreezeWindowRequest {
    pub name: String,
    pub environment_id: Option<String>,
    pub environment_type: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: Option<String>,
    pub recurrence: Option<FreezeRecurrence>,
    pub reason: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFreezeWindowRequest {
    pub name: Option<String>,
    pub environment_id: Option<String>,
    pub environment_type: Option<String>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub timezone: Option<String>,
    pub recurrence: Option<FreezeRecurrence>,
    pub reason: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveFreezeQuery {
    pub environment_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveFreezeResponse {
    pub active: bool,
    pub window: Option<FreezeWindowResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlastRadiusPreviewRequest {
    pub change_type: Option<String>,
    pub environment_ids: Option<Vec<String>>,
    pub rollout_percentage_delta: Option<f64>,
    pub proposed_enabled: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlastRadiusEnvironmentResponse {
    pub id: String,
    pub name: String,
    pub environment_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlastRadiusPreviewResponse {
    pub risk_level: String,
    pub summary: String,
    pub affected_environments: Vec<BlastRadiusEnvironmentResponse>,
    pub affected_clients: i64,
    pub affected_contexts: i64,
    pub dependency_count: i64,
    pub evaluation_volume_7d: i64,
    pub warnings: Vec<String>,
    pub risk_markers: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledChangeResponse {
    pub id: String,
    pub team_id: String,
    pub feature_id: String,
    pub stage_id: Option<String>,
    pub environment_id: Option<String>,
    pub action: String,
    pub requested_status: Option<String>,
    pub payload: serde_json::Value,
    pub reason: String,
    pub scheduled_at: DateTime<Utc>,
    pub timezone: String,
    pub status: String,
    pub requested_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub executed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub result_message: Option<String>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledChangesResponse {
    pub items: Vec<ScheduledChangeResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateScheduledChangeRequest {
    pub action: ScheduledChangeAction,
    pub stage_id: Option<String>,
    pub requested_status: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub reason: String,
    pub scheduled_at: DateTime<Utc>,
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RescheduleScheduledChangeRequest {
    pub scheduled_at: DateTime<Utc>,
    pub timezone: Option<String>,
    pub reason: Option<String>,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn can_operate_safety(jwt: &JwtUser) -> bool {
    jwt.is_admin || jwt.roles.iter().any(|role| role == "Team Admin")
}

/// Authorization for a scheduled change, checked when it is created and again when it
/// is about to run. `ENABLE_FEATURE`, `DISABLE_FEATURE` and `ARCHIVE_FEATURE` need what
/// `POST /features/{id}/emergency-*` and `PATCH /features/{id}` need (system admin, or
/// `Team Admin` of the feature's team; system clients never). `STAGE_CHANGE` needs the
/// role rule `POST /stages/{id}/request-change` applies to the requested status.
pub(crate) async fn authorize_scheduled_action(
    pool: &PgPool,
    actor: PolicyActor,
    team_id: Uuid,
    feature_id: Uuid,
    action: &str,
    requested_status: Option<&str>,
) -> Result<(), PolicyError> {
    match action {
        "STAGE_CHANGE" => {
            let status = requested_status
                .ok_or_else(|| PolicyError::Forbidden("requested_status_missing".to_string()))?;
            RoleAuthorizer::authorize_stage_change_request(&actor.roles, status)
                .map_err(|err| PolicyError::Forbidden(err.to_string()))
        }
        "ENABLE_FEATURE" | "DISABLE_FEATURE" | "ARCHIVE_FEATURE" => {
            crate::logic::policy::authorize_feature_update(pool, feature_id, team_id, actor).await
        }
        other => Err(PolicyError::Forbidden(format!(
            "unsupported_scheduled_action_{other}"
        ))),
    }
}

/// Rebuilds the policy actor for the creator of a scheduled change from current
/// database state, so execution never trusts what was true at creation time. `None`
/// when the creator no longer exists, is disabled, or (system client) is disabled,
/// expired, or belongs to another team.
pub(crate) async fn load_scheduled_change_creator(
    pool: &PgPool,
    change: &ScheduledChangeRow,
) -> Result<Option<PolicyActor>, sqlx::Error> {
    let Some(creator_id) = change.requested_by else {
        return Ok(None);
    };
    let Some(user) =
        sqlx::query("SELECT username, is_admin FROM users WHERE id = $1 AND enabled = TRUE")
            .bind(creator_id)
            .fetch_optional(pool)
            .await?
    else {
        return Ok(None);
    };
    let username: String = user.get("username");
    let is_admin: bool = user.get("is_admin");
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM user_roles ur JOIN roles r ON r.id = ur.role_id WHERE ur.user_id = $1",
    )
    .bind(creator_id)
    .fetch_all(pool)
    .await?;

    let system_client =
        sqlx::query("SELECT team_id, enabled, expires_at FROM system_clients WHERE id = $1")
            .bind(creator_id)
            .fetch_optional(pool)
            .await?;
    match system_client {
        Some(client) => {
            let active = client.get::<bool, _>("enabled")
                && client.get::<DateTime<Utc>, _>("expires_at") > Utc::now()
                && client.get::<Uuid, _>("team_id") == change.team_id;
            Ok(active.then(|| PolicyActor::system_client(creator_id, username, roles)))
        }
        None => Ok(Some(PolicyActor::user(
            creator_id, username, is_admin, roles,
        ))),
    }
}

async fn policy_actor_for_request(pool: &PgPool, jwt: &JwtUser) -> Result<PolicyActor, RestError> {
    let is_system_client: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM system_clients WHERE id = $1)")
            .bind(jwt.id)
            .fetch_one(pool)
            .await
            .map_err(RestError::from)?;
    Ok(if is_system_client {
        PolicyActor::system_client(jwt.id, jwt.username.clone(), jwt.roles.clone())
    } else {
        PolicyActor::user(
            jwt.id,
            jwt.username.clone(),
            jwt.is_admin,
            jwt.roles.clone(),
        )
    })
}

fn rest_error_from_policy(err: PolicyError) -> RestError {
    match err {
        PolicyError::Unauthorized => RestError::unauthorized("User authentication not found"),
        PolicyError::Forbidden(reason) => RestError::policy_denied(reason),
        PolicyError::Internal(err) => {
            log::error!("Scheduled change authorization failed: {err:?}");
            RestError::internal("Authorization service is temporarily unavailable")
        }
    }
}

fn validate_reason(reason: &str, field: &str) -> Result<String, RestError> {
    let trimmed = reason.trim();
    if trimmed.len() < 5 {
        return Err(RestError::invalid_input(format!(
            "{field} must be at least 5 characters"
        )));
    }
    if trimmed.len() > 500 {
        return Err(RestError::invalid_input(format!(
            "{field} must be at most 500 characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn validate_freeze_scope(
    environment_id: Option<&str>,
    environment_type: Option<&str>,
) -> Result<(), RestError> {
    if environment_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
        && environment_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        return Err(RestError::invalid_input(
            "environmentId or environmentType is required",
        ));
    }
    Ok(())
}

fn map_freeze_window(row: FreezeWindowRow) -> FreezeWindowResponse {
    FreezeWindowResponse {
        id: row.id.to_string(),
        team_id: row.team_id.to_string(),
        name: row.name,
        environment_id: row.environment_id.map(|id| id.to_string()),
        environment_type: row.environment_type,
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        timezone: row.timezone,
        recurrence: row.recurrence,
        reason: row.reason,
        active: row.active,
        created_by: row.created_by.map(|id| id.to_string()),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn map_scheduled_change(row: ScheduledChangeRow) -> ScheduledChangeResponse {
    ScheduledChangeResponse {
        id: row.id.to_string(),
        team_id: row.team_id.to_string(),
        feature_id: row.feature_id.to_string(),
        stage_id: row.stage_id.map(|id| id.to_string()),
        environment_id: row.environment_id.map(|id| id.to_string()),
        action: row.action,
        requested_status: row.requested_status,
        payload: row.payload,
        reason: row.reason,
        scheduled_at: row.scheduled_at,
        timezone: row.timezone,
        status: row.status,
        requested_by: row.requested_by.map(|id| id.to_string()),
        created_at: row.created_at,
        updated_at: row.updated_at,
        executed_at: row.executed_at,
        cancelled_at: row.cancelled_at,
        result_message: row.result_message,
        failure_message: row.failure_message,
    }
}

pub(crate) fn freeze_window_active(row: &FreezeWindowRow, now: DateTime<Utc>) -> bool {
    if !row.active || now < row.starts_at {
        return false;
    }
    let duration = row.ends_at - row.starts_at;
    if duration <= ChronoDuration::zero() {
        return false;
    }

    match row.recurrence.as_str() {
        "NONE" => now >= row.starts_at && now < row.ends_at,
        "DAILY" => {
            let window_seconds = duration.num_seconds().min(86_400);
            let start_seconds = i64::from(row.starts_at.time().num_seconds_from_midnight());
            let now_seconds = i64::from(now.time().num_seconds_from_midnight());
            let elapsed = (now_seconds - start_seconds).rem_euclid(86_400);
            elapsed < window_seconds
        }
        "WEEKLY" => {
            let window_seconds = duration.num_seconds().min(604_800);
            let elapsed = now.signed_duration_since(row.starts_at).num_seconds();
            elapsed >= 0 && elapsed.rem_euclid(604_800) < window_seconds
        }
        _ => false,
    }
}

pub(crate) async fn active_freeze_for_environment(
    pool: &PgPool,
    team_id: Uuid,
    environment_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<FreezeWindowRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, FreezeWindowRow>(
        r#"
        SELECT fw.id, fw.team_id, fw.name, fw.environment_id, fw.environment_type,
               fw.starts_at, fw.ends_at, fw.timezone, fw.recurrence, fw.reason,
               fw.active, fw.created_by, fw.created_at, fw.updated_at
        FROM change_freeze_windows fw
        JOIN environments e ON e.id = $2
        WHERE fw.team_id = $1
          AND fw.active = TRUE
          AND (
              fw.environment_id = $2
              OR (
                  fw.environment_id IS NULL
                  AND fw.environment_type IS NOT NULL
                  AND LOWER(fw.environment_type) = LOWER(e.environment_type)
              )
          )
        ORDER BY fw.starts_at ASC
        "#,
    )
    .bind(team_id)
    .bind(environment_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().find(|row| freeze_window_active(row, now)))
}

async fn log_freeze_attempt(
    activity_repo: &dyn ActivityLogRepository,
    activity_type: &str,
    team_id: Uuid,
    feature_id: Uuid,
    feature_key: &str,
    environment_id: Uuid,
    jwt: &JwtUser,
    window: &FreezeWindowRow,
    override_reason: Option<&str>,
) -> Result<Uuid, RestError> {
    let activity = activity_repo
        .create_activity(CreateActivityLog {
            activity_type: activity_type.to_string(),
            entity_type: "feature".to_string(),
            entity_id: feature_id.to_string(),
            actor_id: Some(jwt.id),
            actor_name: Some(jwt.username.clone()),
            description: if activity_type == "freeze_override" {
                format!(
                    "Change freeze override used for feature '{}' during '{}'",
                    feature_key, window.name
                )
            } else {
                format!(
                    "Change blocked for feature '{}' during freeze '{}'",
                    feature_key, window.name
                )
            },
            metadata: Some(serde_json::json!({
                "team_id": team_id.to_string(),
                "feature_id": feature_id.to_string(),
                "feature_key": feature_key,
                "environment_id": environment_id.to_string(),
                "freeze_window_id": window.id.to_string(),
                "freeze_window_name": window.name.clone(),
                "override_reason": override_reason,
            })),
        })
        .await
        .map_err(RestError::from)?;
    Ok(activity.id)
}

/// A freeze was overridden with a reason. The activity row is already written;
/// the caller hands `reason` to the justification check.
#[derive(Debug, Clone)]
pub(crate) struct FreezeOverride {
    pub activity_id: Uuid,
    pub team_id: Uuid,
    pub feature_key: String,
    pub reason: String,
}

impl FreezeOverride {
    pub(crate) async fn record(&self, ai: &Option<web::Data<AiRuntime>>) {
        record_reason(
            ai,
            self.team_id,
            SubjectType::Activity,
            self.activity_id,
            ReasonKind::FreezeOverride,
            &self.reason,
            Some(self.feature_key.as_str()),
        )
        .await;
    }
}

/// Blocks a change during an active freeze unless the caller may override it.
/// Returns the override that was used, if any.
pub(crate) async fn enforce_freeze_for_feature_environment(
    pool: &PgPool,
    activity_repo: &dyn ActivityLogRepository,
    team_id: Uuid,
    feature_id: Uuid,
    feature_key: &str,
    environment_id: Uuid,
    jwt: &JwtUser,
    override_reason: Option<&str>,
) -> Result<Option<FreezeOverride>, RestError> {
    let Some(window) = active_freeze_for_environment(pool, team_id, environment_id, Utc::now())
        .await
        .map_err(RestError::from)?
    else {
        return Ok(None);
    };

    if can_operate_safety(jwt) {
        let reason = validate_reason(
            override_reason.unwrap_or_default(),
            "freeze override reason",
        )?;
        let activity_id = log_freeze_attempt(
            activity_repo,
            "freeze_override",
            team_id,
            feature_id,
            feature_key,
            environment_id,
            jwt,
            &window,
            Some(&reason),
        )
        .await?;
        return Ok(Some(FreezeOverride {
            activity_id,
            team_id,
            feature_key: feature_key.to_string(),
            reason,
        }));
    }

    log_freeze_attempt(
        activity_repo,
        "freeze_blocked",
        team_id,
        feature_id,
        feature_key,
        environment_id,
        jwt,
        &window,
        None,
    )
    .await?;

    Err(RestError::forbidden(format!(
        "Change blocked by active freeze window '{}'",
        window.name
    )))
}

pub(crate) async fn enforce_freeze_for_stage(
    pool: &PgPool,
    activity_repo: &dyn ActivityLogRepository,
    stage_id: Uuid,
    jwt: &JwtUser,
    override_reason: Option<&str>,
) -> Result<Option<FreezeOverride>, RestError> {
    let row = sqlx::query(
        r#"
        SELECT f.id AS feature_id, f.key AS feature_key, f.team_id, fs.environment_id
        FROM features_pipeline_stages fs
        JOIN features f ON f.id = fs.feature_id
        WHERE fs.id = $1
        "#,
    )
    .bind(stage_id)
    .fetch_optional(pool)
    .await
    .map_err(RestError::from)?
    .ok_or_else(|| RestError::not_found("stage not found"))?;

    let team_id: Uuid = row.get("team_id");
    let feature_id: Uuid = row.get("feature_id");
    let feature_key: String = row.get("feature_key");
    let environment_id: Uuid = row.get("environment_id");

    enforce_freeze_for_feature_environment(
        pool,
        activity_repo,
        team_id,
        feature_id,
        feature_key.as_str(),
        environment_id,
        jwt,
        override_reason,
    )
    .await
}

pub(crate) async fn scheduled_change_hits_freeze(
    pool: &PgPool,
    change: &ScheduledChangeRow,
) -> Result<Option<FreezeWindowRow>, sqlx::Error> {
    let environment_ids = if let Some(environment_id) = change.environment_id {
        vec![environment_id]
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT DISTINCT environment_id
            FROM features_pipeline_stages
            WHERE feature_id = $1
            "#,
        )
        .bind(change.feature_id)
        .fetch_all(pool)
        .await?
    };

    let now = Utc::now();
    for environment_id in environment_ids {
        if let Some(window) =
            active_freeze_for_environment(pool, change.team_id, environment_id, now).await?
        {
            return Ok(Some(window));
        }
    }
    Ok(None)
}

pub(crate) async fn claim_due_scheduled_changes(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<ScheduledChangeRow>, sqlx::Error> {
    sqlx::query_as::<_, ScheduledChangeRow>(
        r#"
        WITH due AS (
            SELECT id
            FROM scheduled_feature_changes
            WHERE status = 'PENDING'
              AND scheduled_at <= NOW()
            ORDER BY scheduled_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE scheduled_feature_changes s
        SET status = 'EXECUTING', updated_at = NOW()
        FROM due
        WHERE s.id = due.id
        RETURNING s.id, s.team_id, s.feature_id, s.stage_id, s.environment_id,
                  s.action, s.requested_status, s.payload, s.reason,
                  s.scheduled_at, s.timezone, s.status, s.requested_by,
                  s.created_at, s.updated_at, s.executed_at, s.cancelled_at,
                  s.result_message, s.failure_message
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub(crate) async fn mark_scheduled_change_status(
    pool: &PgPool,
    id: Uuid,
    status: ScheduledChangeStatus,
    result_message: Option<&str>,
    failure_message: Option<&str>,
) -> Result<ScheduledChangeRow, sqlx::Error> {
    sqlx::query_as::<_, ScheduledChangeRow>(
        r#"
        UPDATE scheduled_feature_changes
        SET status = $2,
            result_message = $3,
            failure_message = $4,
            executed_at = CASE WHEN $2 IN ('EXECUTED', 'FAILED', 'BLOCKED') THEN NOW() ELSE executed_at END,
            cancelled_at = CASE WHEN $2 = 'CANCELLED' THEN NOW() ELSE cancelled_at END,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, team_id, feature_id, stage_id, environment_id,
                  action, requested_status, payload, reason,
                  scheduled_at, timezone, status, requested_by,
                  created_at, updated_at, executed_at, cancelled_at,
                  result_message, failure_message
        "#,
    )
    .bind(id)
    .bind(status.as_str())
    .bind(result_message)
    .bind(failure_message)
    .fetch_one(pool)
    .await
}

pub(crate) fn stage_request_from_status(
    status: &str,
) -> Option<crate::logic::feature::StageChangeRequestType> {
    match status {
        "DEPLOYMENT_REQUESTED" => {
            Some(crate::logic::feature::StageChangeRequestType::DeploymentRequested)
        }
        "DEPLOYMENT_REJECTED" => {
            Some(crate::logic::feature::StageChangeRequestType::DeploymentRejected)
        }
        "DEPLOYED" => Some(crate::logic::feature::StageChangeRequestType::Deployed),
        "ROLLBACK_REQUESTED" => {
            Some(crate::logic::feature::StageChangeRequestType::RollbackRequested)
        }
        "ROLLBACK_REJECTED" => {
            Some(crate::logic::feature::StageChangeRequestType::RollbackRejected)
        }
        "ROLLBACKED" => Some(crate::logic::feature::StageChangeRequestType::Rollbacked),
        _ => None,
    }
}

fn calculate_risk_level(
    production: bool,
    dependency_count: i64,
    evaluation_volume_7d: i64,
    rollout_delta: f64,
    proposed_enabled: Option<bool>,
) -> (String, Vec<String>) {
    let mut score = 0;
    let mut markers = Vec::new();

    if production {
        score += 3;
        markers.push("production-impact".to_string());
    }
    if dependency_count >= 3 {
        score += 2;
        markers.push("dependency-heavy".to_string());
    }
    if evaluation_volume_7d >= 10_000 {
        score += 2;
        markers.push("high-traffic".to_string());
    }
    if rollout_delta.abs() >= 50.0 {
        score += 2;
        markers.push("large-rollout-delta".to_string());
    }
    if proposed_enabled == Some(false) {
        score += 2;
        markers.push("disable-impact".to_string());
    }

    let level = if score >= 5 {
        "high"
    } else if score >= 2 {
        "medium"
    } else {
        "low"
    };
    (level.to_string(), markers)
}

pub(crate) async fn compute_blast_radius(
    pool: &PgPool,
    feature_id: Uuid,
    input: BlastRadiusPreviewRequest,
) -> Result<BlastRadiusPreviewResponse, RestError> {
    let feature = sqlx::query(
        r#"
        SELECT id, key, team_id, evaluation_count_7d
        FROM features
        WHERE id = $1
        "#,
    )
    .bind(feature_id)
    .fetch_optional(pool)
    .await
    .map_err(RestError::from)?
    .ok_or_else(|| RestError::not_found("feature not found"))?;

    let team_id: Uuid = feature.get("team_id");
    let requested_environment_ids = input
        .environment_ids
        .unwrap_or_default()
        .into_iter()
        .map(|value| parse_uuid(&value, "environment_id"))
        .collect::<Result<Vec<_>, _>>()?;

    let environments = if requested_environment_ids.is_empty() {
        sqlx::query(
            r#"
            SELECT DISTINCT e.id, e.name, e.environment_type
            FROM features_pipeline_stages fs
            JOIN environments e ON e.id = fs.environment_id
            WHERE fs.feature_id = $1
            ORDER BY e.name
            "#,
        )
        .bind(feature_id)
        .fetch_all(pool)
        .await
        .map_err(RestError::from)?
    } else {
        sqlx::query(
            r#"
            SELECT id, name, environment_type
            FROM environments
            WHERE team_id = $1
              AND id = ANY($2)
            ORDER BY name
            "#,
        )
        .bind(team_id)
        .bind(&requested_environment_ids)
        .fetch_all(pool)
        .await
        .map_err(RestError::from)?
    };

    let environment_ids = environments
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    let production = environments.iter().any(|row| {
        row.get::<String, _>("environment_type")
            .eq_ignore_ascii_case("production")
    });
    let affected_environments = environments
        .iter()
        .map(|row| BlastRadiusEnvironmentResponse {
            id: row.get::<Uuid, _>("id").to_string(),
            name: row.get("name"),
            environment_type: row.get("environment_type"),
        })
        .collect::<Vec<_>>();

    let affected_clients = if environment_ids.is_empty() {
        0
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM clients WHERE team_id = $1 AND environment_id = ANY($2)",
        )
        .bind(team_id)
        .bind(&environment_ids)
        .fetch_one(pool)
        .await
        .map_err(RestError::from)?
    };

    let affected_contexts =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM contexts WHERE team_id = $1")
            .bind(team_id)
            .fetch_one(pool)
            .await
            .map_err(RestError::from)?;

    let dependency_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM feature_dependencies WHERE feature_id = $1 OR depends_on_id = $1",
    )
    .bind(feature_id)
    .fetch_one(pool)
    .await
    .map_err(RestError::from)?;

    let evaluation_volume_7d: i64 = feature.get("evaluation_count_7d");
    let rollout_delta = input.rollout_percentage_delta.unwrap_or(0.0);
    let (risk_level, risk_markers) = calculate_risk_level(
        production,
        dependency_count,
        evaluation_volume_7d,
        rollout_delta,
        input.proposed_enabled,
    );

    let mut warnings = Vec::new();
    if evaluation_volume_7d == 0 {
        warnings.push(
            "No recent traffic data is available; impact estimate may be incomplete".to_string(),
        );
    }
    if affected_clients == 0 {
        warnings.push("No clients are currently mapped to affected environments".to_string());
    }

    let summary = format!(
        "{} risk: {} environment(s), {} client(s), {} context(s), {} dependent feature link(s), {} evaluations in 7d",
        risk_level,
        affected_environments.len(),
        affected_clients,
        affected_contexts,
        dependency_count,
        evaluation_volume_7d
    );

    Ok(BlastRadiusPreviewResponse {
        risk_level,
        summary,
        affected_environments,
        affected_clients,
        affected_contexts,
        dependency_count,
        evaluation_volume_7d,
        warnings,
        risk_markers,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/freeze-windows",
    params(("team_id" = String, Path, description = "Team ID")),
    responses((status = 200, description = "Freeze windows", body = FreezeWindowsResponse)),
    tag = "Operational Safety"
)]
#[get("/teams/{team_id}/freeze-windows")]
pub(crate) async fn list_freeze_windows(
    pool: web::Data<PgPool>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let rows = sqlx::query_as::<_, FreezeWindowRow>(
        r#"
        SELECT id, team_id, name, environment_id, environment_type, starts_at, ends_at,
               timezone, recurrence, reason, active, created_by, created_at, updated_at
        FROM change_freeze_windows
        WHERE team_id = $1
        ORDER BY active DESC, starts_at DESC
        "#,
    )
    .bind(team_uuid)
    .fetch_all(pool.get_ref())
    .await
    .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(FreezeWindowsResponse {
        items: rows.into_iter().map(map_freeze_window).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/freeze-windows/active",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ActiveFreezeQuery
    ),
    responses((status = 200, description = "Active freeze", body = ActiveFreezeResponse)),
    tag = "Operational Safety"
)]
#[get("/teams/{team_id}/freeze-windows/active")]
pub(crate) async fn active_freeze_window(
    pool: web::Data<PgPool>,
    team_id: web::Path<String>,
    query: web::Query<ActiveFreezeQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let environment_uuid = parse_uuid(&query.environment_id, "environment_id")?;
    let window =
        active_freeze_for_environment(pool.get_ref(), team_uuid, environment_uuid, Utc::now())
            .await
            .map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(ActiveFreezeResponse {
        active: window.is_some(),
        window: window.map(map_freeze_window),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/freeze-windows",
    request_body = CreateFreezeWindowRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses((status = 201, description = "Freeze window created", body = FreezeWindowResponse)),
    tag = "Operational Safety"
)]
#[post("/teams/{team_id}/freeze-windows")]
pub(crate) async fn create_freeze_window(
    pool: web::Data<PgPool>,
    ai: Option<web::Data<AiRuntime>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<CreateFreezeWindowRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    if !can_operate_safety(&jwt) {
        return Err(RestError::forbidden(
            "Only system admins or Team Admins can manage freeze windows",
        ));
    }
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    validate_freeze_scope(
        payload.environment_id.as_deref(),
        payload.environment_type.as_deref(),
    )?;
    if payload.ends_at <= payload.starts_at {
        return Err(RestError::invalid_input("endsAt must be after startsAt"));
    }
    let environment_id = payload
        .environment_id
        .as_deref()
        .map(|value| parse_uuid(value, "environment_id"))
        .transpose()?;

    let row = sqlx::query_as::<_, FreezeWindowRow>(
        r#"
        INSERT INTO change_freeze_windows (
            team_id, name, environment_id, environment_type, starts_at, ends_at,
            timezone, recurrence, reason, active, created_by
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        RETURNING id, team_id, name, environment_id, environment_type, starts_at, ends_at,
                  timezone, recurrence, reason, active, created_by, created_at, updated_at
        "#,
    )
    .bind(team_uuid)
    .bind(payload.name.trim())
    .bind(environment_id)
    .bind(payload.environment_type.as_deref().map(str::trim))
    .bind(payload.starts_at)
    .bind(payload.ends_at)
    .bind(payload.timezone.as_deref().unwrap_or("UTC"))
    .bind(
        payload
            .recurrence
            .unwrap_or(FreezeRecurrence::None)
            .as_str(),
    )
    .bind(payload.reason.as_deref().map(str::trim))
    .bind(payload.active.unwrap_or(true))
    .bind(jwt.id)
    .fetch_one(pool.get_ref())
    .await
    .map_err(RestError::from)?;

    if let Some(reason) = row.reason.as_deref() {
        record_reason(
            &ai,
            row.team_id,
            SubjectType::FreezeWindow,
            row.id,
            ReasonKind::FreezeWindow,
            reason,
            None,
        )
        .await;
    }

    Ok(HttpResponse::Created().json(map_freeze_window(row)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/freeze-windows/{id}",
    request_body = UpdateFreezeWindowRequest,
    params(("id" = String, Path, description = "Freeze window ID")),
    responses((status = 200, description = "Freeze window updated", body = FreezeWindowResponse)),
    tag = "Operational Safety"
)]
#[patch("/freeze-windows/{id}")]
pub(crate) async fn update_freeze_window(
    pool: web::Data<PgPool>,
    ai: Option<web::Data<AiRuntime>>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<UpdateFreezeWindowRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    if !can_operate_safety(&jwt) {
        return Err(RestError::forbidden(
            "Only system admins or Team Admins can manage freeze windows",
        ));
    }
    let freeze_id = parse_uuid(&id, "freeze_window_id")?;
    let environment_id = payload
        .environment_id
        .as_deref()
        .map(|value| parse_uuid(value, "environment_id"))
        .transpose()?;

    let row = sqlx::query_as::<_, FreezeWindowRow>(
        r#"
        UPDATE change_freeze_windows
        SET name = COALESCE($2, name),
            environment_id = COALESCE($3, environment_id),
            environment_type = COALESCE($4, environment_type),
            starts_at = COALESCE($5, starts_at),
            ends_at = COALESCE($6, ends_at),
            timezone = COALESCE($7, timezone),
            recurrence = COALESCE($8, recurrence),
            reason = COALESCE($9, reason),
            active = COALESCE($10, active),
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, team_id, name, environment_id, environment_type, starts_at, ends_at,
                  timezone, recurrence, reason, active, created_by, created_at, updated_at
        "#,
    )
    .bind(freeze_id)
    .bind(payload.name.as_deref().map(str::trim))
    .bind(environment_id)
    .bind(payload.environment_type.as_deref().map(str::trim))
    .bind(payload.starts_at)
    .bind(payload.ends_at)
    .bind(payload.timezone.as_deref())
    .bind(payload.recurrence.map(FreezeRecurrence::as_str))
    .bind(payload.reason.as_deref().map(str::trim))
    .bind(payload.active)
    .fetch_optional(pool.get_ref())
    .await
    .map_err(RestError::from)?
    .ok_or_else(|| RestError::not_found("freeze window not found"))?;

    if row.ends_at <= row.starts_at {
        return Err(RestError::invalid_input("endsAt must be after startsAt"));
    }

    // Only a reason sent with this change is checked, not one kept from before.
    if let Some(reason) = payload.reason.as_deref() {
        record_reason(
            &ai,
            row.team_id,
            SubjectType::FreezeWindow,
            row.id,
            ReasonKind::FreezeWindow,
            reason,
            None,
        )
        .await;
    }

    Ok(HttpResponse::Ok().json(map_freeze_window(row)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/freeze-windows/{id}",
    params(("id" = String, Path, description = "Freeze window ID")),
    responses((status = 204, description = "Freeze window deleted")),
    tag = "Operational Safety"
)]
#[delete("/freeze-windows/{id}")]
pub(crate) async fn delete_freeze_window(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    if !can_operate_safety(&jwt) {
        return Err(RestError::forbidden(
            "Only system admins or Team Admins can manage freeze windows",
        ));
    }
    let freeze_id = parse_uuid(&id, "freeze_window_id")?;
    sqlx::query("DELETE FROM change_freeze_windows WHERE id = $1")
        .bind(freeze_id)
        .execute(pool.get_ref())
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/impact-preview",
    request_body = BlastRadiusPreviewRequest,
    params(("id" = String, Path, description = "Feature ID")),
    responses((status = 200, description = "Blast radius preview", body = BlastRadiusPreviewResponse)),
    tag = "Operational Safety"
)]
#[post("/features/{id}/impact-preview")]
pub(crate) async fn preview_feature_impact(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
    payload: web::Json<BlastRadiusPreviewRequest>,
) -> Result<impl Responder, RestError> {
    let feature_id = parse_uuid(&id, "feature_id")?;
    let preview = compute_blast_radius(pool.get_ref(), feature_id, payload.into_inner()).await?;
    Ok(HttpResponse::Ok().json(preview))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/{id}/scheduled-changes",
    params(("id" = String, Path, description = "Feature ID")),
    responses((status = 200, description = "Scheduled changes", body = ScheduledChangesResponse)),
    tag = "Operational Safety"
)]
#[get("/features/{id}/scheduled-changes")]
pub(crate) async fn list_scheduled_changes(
    pool: web::Data<PgPool>,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let feature_id = parse_uuid(&id, "feature_id")?;
    let rows = sqlx::query_as::<_, ScheduledChangeRow>(
        r#"
        SELECT id, team_id, feature_id, stage_id, environment_id, action, requested_status,
               payload, reason, scheduled_at, timezone, status, requested_by,
               created_at, updated_at, executed_at, cancelled_at, result_message, failure_message
        FROM scheduled_feature_changes
        WHERE feature_id = $1
        ORDER BY scheduled_at DESC
        "#,
    )
    .bind(feature_id)
    .fetch_all(pool.get_ref())
    .await
    .map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(ScheduledChangesResponse {
        items: rows.into_iter().map(map_scheduled_change).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/scheduled-changes",
    request_body = CreateScheduledChangeRequest,
    params(("id" = String, Path, description = "Feature ID")),
    responses((status = 201, description = "Scheduled change created", body = ScheduledChangeResponse)),
    tag = "Operational Safety"
)]
#[post("/features/{id}/scheduled-changes")]
pub(crate) async fn create_scheduled_change(
    pool: web::Data<PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    ai: Option<web::Data<AiRuntime>>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<CreateScheduledChangeRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    let feature_id = parse_uuid(&id, "feature_id")?;
    if payload.scheduled_at <= Utc::now() {
        return Err(RestError::invalid_input(
            "scheduledAt must be in the future",
        ));
    }
    let reason = validate_reason(&payload.reason, "schedule reason")?;
    let feature = sqlx::query("SELECT id, key, team_id FROM features WHERE id = $1")
        .bind(feature_id)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(RestError::from)?
        .ok_or_else(|| RestError::not_found("feature not found"))?;
    let team_id: Uuid = feature.get("team_id");

    let stage_id = payload
        .stage_id
        .as_deref()
        .map(|value| parse_uuid(value, "stage_id"))
        .transpose()?;
    let mut environment_id = None;
    let stage_change_target = if payload.action == ScheduledChangeAction::StageChange {
        let stage_id = stage_id.ok_or_else(|| {
            RestError::invalid_input("stageId is required for STAGE_CHANGE schedules")
        })?;
        let requested_status = payload.requested_status.as_deref().ok_or_else(|| {
            RestError::invalid_input("requestedStatus is required for STAGE_CHANGE schedules")
        })?;
        if stage_request_from_status(requested_status).is_none() {
            return Err(RestError::invalid_input("requestedStatus is not supported"));
        }
        Some(stage_id)
    } else {
        None
    };

    // Same authorization the direct endpoints apply, before anything else about the
    // feature (such as its stages) is revealed.
    let actor = policy_actor_for_request(pool.get_ref(), &jwt).await?;
    authorize_scheduled_action(
        pool.get_ref(),
        actor,
        team_id,
        feature_id,
        payload.action.as_str(),
        payload.requested_status.as_deref(),
    )
    .await
    .map_err(rest_error_from_policy)?;

    if let Some(stage_id) = stage_change_target {
        let stage_row = sqlx::query(
            "SELECT environment_id FROM features_pipeline_stages WHERE id = $1 AND feature_id = $2",
        )
        .bind(stage_id)
        .bind(feature_id)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(RestError::from)?
        .ok_or_else(|| RestError::not_found("stage not found for feature"))?;
        environment_id = Some(stage_row.get::<Uuid, _>("environment_id"));
    }

    let row = sqlx::query_as::<_, ScheduledChangeRow>(
        r#"
        INSERT INTO scheduled_feature_changes (
            team_id, feature_id, stage_id, environment_id, action, requested_status,
            payload, reason, scheduled_at, timezone, requested_by
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        RETURNING id, team_id, feature_id, stage_id, environment_id, action, requested_status,
                  payload, reason, scheduled_at, timezone, status, requested_by,
                  created_at, updated_at, executed_at, cancelled_at, result_message, failure_message
        "#,
    )
    .bind(team_id)
    .bind(feature_id)
    .bind(stage_id)
    .bind(environment_id)
    .bind(payload.action.as_str())
    .bind(payload.requested_status.as_deref())
    .bind(
        payload
            .payload
            .clone()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .bind(&reason)
    .bind(payload.scheduled_at)
    .bind(payload.timezone.as_deref().unwrap_or("UTC"))
    .bind(jwt.id)
    .fetch_one(pool.get_ref())
    .await
    .map_err(RestError::from)?;

    let feature_key: String = feature.get("key");
    let activity = activity_repo
        .create_activity(CreateActivityLog {
            activity_type: "scheduled_change_created".to_string(),
            entity_type: "feature".to_string(),
            entity_id: feature_id.to_string(),
            actor_id: Some(jwt.id),
            actor_name: Some(jwt.username),
            description: format!(
                "Scheduled {} for feature '{}'",
                payload.action.as_str(),
                feature_key
            ),
            metadata: Some(serde_json::json!({
                "scheduled_change_id": row.id.to_string(),
                "team_id": team_id.to_string(),
                "feature_id": feature_id.to_string(),
                "action": row.action.clone(),
                "scheduled_at": row.scheduled_at.to_rfc3339(),
                "reason": reason,
            })),
        })
        .await;
    // Without an activity row there is nothing to attach the verdict to.
    if let Ok(activity) = activity {
        record_reason(
            &ai,
            team_id,
            SubjectType::Activity,
            activity.id,
            ReasonKind::ScheduledChange,
            &reason,
            Some(feature_key.as_str()),
        )
        .await;
    }

    Ok(HttpResponse::Created().json(map_scheduled_change(row)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/scheduled-changes/{id}/cancel",
    params(("id" = String, Path, description = "Scheduled change ID")),
    responses((status = 200, description = "Scheduled change cancelled", body = ScheduledChangeResponse)),
    tag = "Operational Safety"
)]
#[patch("/scheduled-changes/{id}/cancel")]
pub(crate) async fn cancel_scheduled_change(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    let schedule_id = parse_uuid(&id, "scheduled_change_id")?;
    let existing = sqlx::query_as::<_, ScheduledChangeRow>(
        r#"
        SELECT id, team_id, feature_id, stage_id, environment_id, action, requested_status,
               payload, reason, scheduled_at, timezone, status, requested_by,
               created_at, updated_at, executed_at, cancelled_at, result_message, failure_message
        FROM scheduled_feature_changes
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .fetch_optional(pool.get_ref())
    .await
    .map_err(RestError::from)?
    .ok_or_else(|| RestError::not_found("scheduled change not found"))?;
    if existing.status != "PENDING" {
        return Err(RestError::invalid_input(
            "Only pending scheduled changes can be cancelled",
        ));
    }
    if existing.requested_by != Some(jwt.id) && !can_operate_safety(&jwt) {
        return Err(RestError::forbidden(
            "Only creator, system admin, or Team Admin can cancel",
        ));
    }
    let row = mark_scheduled_change_status(
        pool.get_ref(),
        schedule_id,
        ScheduledChangeStatus::Cancelled,
        Some("Cancelled by user"),
        None,
    )
    .await
    .map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(map_scheduled_change(row)))
}

#[utoipa::path(
    patch,
    path = "/api/v1/scheduled-changes/{id}/reschedule",
    request_body = RescheduleScheduledChangeRequest,
    params(("id" = String, Path, description = "Scheduled change ID")),
    responses((status = 200, description = "Scheduled change rescheduled", body = ScheduledChangeResponse)),
    tag = "Operational Safety"
)]
#[patch("/scheduled-changes/{id}/reschedule")]
pub(crate) async fn reschedule_scheduled_change(
    pool: web::Data<PgPool>,
    ai: Option<web::Data<AiRuntime>>,
    req: HttpRequest,
    id: web::Path<String>,
    payload: web::Json<RescheduleScheduledChangeRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    if payload.scheduled_at <= Utc::now() {
        return Err(RestError::invalid_input(
            "scheduledAt must be in the future",
        ));
    }
    let schedule_id = parse_uuid(&id, "scheduled_change_id")?;
    let row = sqlx::query_as::<_, ScheduledChangeRow>(
        r#"
        UPDATE scheduled_feature_changes
        SET scheduled_at = $2,
            timezone = COALESCE($3, timezone),
            reason = COALESCE($4, reason),
            updated_at = NOW()
        WHERE id = $1
          AND status = 'PENDING'
          AND (requested_by = $5 OR $6 = TRUE)
        RETURNING id, team_id, feature_id, stage_id, environment_id, action, requested_status,
                  payload, reason, scheduled_at, timezone, status, requested_by,
                  created_at, updated_at, executed_at, cancelled_at, result_message, failure_message
        "#,
    )
    .bind(schedule_id)
    .bind(payload.scheduled_at)
    .bind(payload.timezone.as_deref())
    .bind(payload.reason.as_deref().map(str::trim))
    .bind(jwt.id)
    .bind(can_operate_safety(&jwt))
    .fetch_optional(pool.get_ref())
    .await
    .map_err(RestError::from)?
    .ok_or_else(|| RestError::not_found("pending scheduled change not found or not editable"))?;

    // A reschedule writes no activity row, so the scheduled change is the subject.
    if let Some(reason) = payload.reason.as_deref() {
        record_reason(
            &ai,
            row.team_id,
            SubjectType::ScheduledChange,
            row.id,
            ReasonKind::ScheduledChange,
            reason,
            None,
        )
        .await;
    }

    Ok(HttpResponse::Ok().json(map_scheduled_change(row)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_freeze_windows)
        .service(active_freeze_window)
        .service(create_freeze_window)
        .service(update_freeze_window)
        .service(delete_freeze_window)
        .service(preview_feature_impact)
        .service(list_scheduled_changes)
        .service(create_scheduled_change)
        .service(cancel_scheduled_change)
        .service(reschedule_scheduled_change);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn freeze_row(
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
        recurrence: &str,
    ) -> FreezeWindowRow {
        FreezeWindowRow {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            name: "Release freeze".to_string(),
            environment_id: Some(Uuid::new_v4()),
            environment_type: None,
            starts_at,
            ends_at,
            timezone: "UTC".to_string(),
            recurrence: recurrence.to_string(),
            reason: None,
            active: true,
            created_by: None,
            created_at: starts_at,
            updated_at: starts_at,
        }
    }

    #[test]
    fn one_off_freeze_window_uses_absolute_bounds() {
        let start = DateTime::parse_from_rfc3339("2026-06-09T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let row = freeze_row(start, start + ChronoDuration::hours(2), "NONE");
        assert!(freeze_window_active(
            &row,
            start + ChronoDuration::minutes(30)
        ));
        assert!(!freeze_window_active(
            &row,
            start + ChronoDuration::hours(3)
        ));
    }

    #[test]
    fn recurring_daily_window_reuses_time_of_day() {
        let start = DateTime::parse_from_rfc3339("2026-06-01T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let row = freeze_row(start, start + ChronoDuration::hours(1), "DAILY");
        let next_day = DateTime::parse_from_rfc3339("2026-06-09T02:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let outside = DateTime::parse_from_rfc3339("2026-06-09T04:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(freeze_window_active(&row, next_day));
        assert!(!freeze_window_active(&row, outside));
    }
}

/// Authorization of scheduled changes, at creation (REST) and at execution (scheduler).
/// These need the seeded and migrated database.
#[cfg(test)]
mod authorization_tests {
    use super::*;
    use crate::database::activity_log::activity_log_repository;
    use crate::database::entity::FeatureType;
    use crate::database::feature::{CreateFeature, CreateFeatureStage, feature_repository};
    use crate::database::system_client::{CreateSystemClient, system_client_repository};
    use crate::scheduler::scheduled_changes::{CREATOR_NOT_AUTHORIZED, ScheduledChangeScheduler};
    use actix_web::{App, http::StatusCode, test};
    use sqlx::postgres::PgPoolOptions;

    const APPROVER_ROLE: &str = "00000000-0000-0000-0000-000000000001";
    const REQUESTER_ROLE: &str = "00000000-0000-0000-0000-000000000002";
    const TEAM_ADMIN_ROLE: &str = "00000000-0000-0000-0000-000000000003";

    /// Scheduler runs claim every due change in the database, so tests that run it
    /// must not overlap.
    static SCHEDULER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn test_pool() -> PgPool {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("connect")
    }

    struct World {
        pool: PgPool,
        team_id: Uuid,
        other_team_id: Uuid,
        feature_id: Uuid,
        stage_id: Uuid,
        users: Vec<Uuid>,
    }

    impl World {
        async fn new() -> Self {
            let pool = test_pool().await;
            let team_id = insert_team(&pool).await;
            let other_team_id = insert_team(&pool).await;
            let env_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO environments (id, name, active, team_id, environment_type) VALUES ($1, $2, true, $3, 'Production')",
            )
            .bind(env_id)
            .bind(format!("sched-auth-env-{env_id}"))
            .bind(team_id)
            .execute(&pool)
            .await
            .expect("insert environment");
            let stage_id = Uuid::new_v4();
            let feature_id = feature_repository(pool.clone())
                .create_feature(CreateFeature {
                    team_id,
                    key: format!("sched-auth-{}", Uuid::new_v4()),
                    description: None,
                    feature_type: FeatureType::Simple,
                    lifecycle_stage: "active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    stages: vec![CreateFeatureStage {
                        id: stage_id,
                        environment_id: env_id,
                        order_index: 0,
                        parent_stage: None,
                        position: "{ \"x\": 0, \"y\": 0 }".to_string(),
                        enabled: true,
                    }],
                    dependencies: vec![],
                    variants: None,
                    flag_kind: None,
                })
                .await
                .expect("create feature");
            Self {
                pool,
                team_id,
                other_team_id,
                feature_id,
                stage_id,
                users: Vec::new(),
            }
        }

        async fn user(&mut self, team: Option<Uuid>, is_admin: bool, roles: &[&str]) -> JwtUser {
            let id = Uuid::new_v4();
            let username = format!("sched_auth_{id}");
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin)
                 VALUES ($1, $2, 'x', 'S', 'A', $3, $4)",
            )
            .bind(id)
            .bind(&username)
            .bind(format!("{username}@example.com"))
            .bind(is_admin)
            .execute(&self.pool)
            .await
            .expect("insert user");
            if let Some(team) = team {
                sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
                    .bind(id)
                    .bind(team)
                    .execute(&self.pool)
                    .await
                    .expect("membership");
            }
            let mut role_names = Vec::new();
            for role in roles {
                let role_id = Uuid::parse_str(role).unwrap();
                sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
                    .bind(id)
                    .bind(role_id)
                    .execute(&self.pool)
                    .await
                    .expect("assign role");
                role_names.push(
                    sqlx::query_scalar::<_, String>("SELECT name FROM roles WHERE id = $1")
                        .bind(role_id)
                        .fetch_one(&self.pool)
                        .await
                        .expect("role name"),
                );
            }
            self.users.push(id);
            JwtUser {
                id,
                username,
                is_admin,
                roles: role_names,
                team_id: team,
                token_hash: "hash".to_string(),
            }
        }

        async fn system_client(&mut self) -> JwtUser {
            let client = system_client_repository(self.pool.clone())
                .create_system_client(
                    self.team_id,
                    CreateSystemClient {
                        name: format!("sched-auth-{}", Uuid::new_v4().simple()),
                        description: None,
                        enabled: true,
                        expires_at: Utc::now() + ChronoDuration::days(1),
                    },
                )
                .await
                .expect("create system client");
            self.users.push(client.id);
            JwtUser {
                id: client.id,
                username: client.name,
                is_admin: false,
                roles: vec!["Requester".to_string(), "Approver".to_string()],
                team_id: Some(self.team_id),
                token_hash: "hash".to_string(),
            }
        }

        async fn insert_due_change(&self, creator: Option<Uuid>, action: &str) -> Uuid {
            sqlx::query_scalar(
                r#"INSERT INTO scheduled_feature_changes
                       (team_id, feature_id, action, reason, scheduled_at, requested_by)
                   VALUES ($1, $2, $3, 'scheduled for test', NOW() - INTERVAL '1 minute', $4)
                   RETURNING id"#,
            )
            .bind(self.team_id)
            .bind(self.feature_id)
            .bind(action)
            .bind(creator)
            .fetch_one(&self.pool)
            .await
            .expect("insert scheduled change")
        }

        /// Inserts a PENDING change that is not yet due. Unlike `insert_due_change`, no
        /// concurrent scheduler run (`claim_due_scheduled_changes` claims every due
        /// change in the database) can claim it, so tests that only touch the REST
        /// layer stay independent of tests that run the scheduler.
        async fn insert_future_change(&self, creator: Option<Uuid>, action: &str) -> Uuid {
            sqlx::query_scalar(
                r#"INSERT INTO scheduled_feature_changes
                       (team_id, feature_id, action, reason, scheduled_at, requested_by)
                   VALUES ($1, $2, $3, 'scheduled for test', NOW() + INTERVAL '1 hour', $4)
                   RETURNING id"#,
            )
            .bind(self.team_id)
            .bind(self.feature_id)
            .bind(action)
            .bind(creator)
            .fetch_one(&self.pool)
            .await
            .expect("insert scheduled change")
        }

        async fn change_state(&self, id: Uuid) -> (String, Option<String>) {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT status, failure_message FROM scheduled_feature_changes WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .expect("load change")
        }

        async fn lifecycle_stage(&self) -> String {
            sqlx::query_scalar("SELECT lifecycle_stage FROM features WHERE id = $1")
                .bind(self.feature_id)
                .fetch_one(&self.pool)
                .await
                .expect("lifecycle")
        }

        async fn run_scheduler(&self) {
            let activity = activity_log_repository(self.pool.clone());
            let environment_logic = crate::logic::environment::environment_logic(
                crate::database::environment::environment_repository(self.pool.clone()),
                activity.clone_box(),
            );
            let feature_logic = crate::logic::feature::feature_logic(
                feature_repository(self.pool.clone()),
                environment_logic,
                activity.clone_box(),
                crate::database::user::user_repository(self.pool.clone()),
            );
            ScheduledChangeScheduler::new(
                self.pool.clone(),
                feature_logic,
                activity,
                std::time::Duration::from_secs(60),
            )
            .run_once(25)
            .await
            .expect("scheduler run");
        }

        async fn cleanup(self) {
            let _ = sqlx::query("DELETE FROM scheduled_feature_changes WHERE feature_id = $1")
                .bind(self.feature_id)
                .execute(&self.pool)
                .await;
            let _ = feature_repository(self.pool.clone())
                .delete_feature(self.feature_id)
                .await;
            let _ = sqlx::query("DELETE FROM teams WHERE id = ANY($1)")
                .bind(vec![self.team_id, self.other_team_id])
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM users WHERE id = ANY($1)")
                .bind(&self.users)
                .execute(&self.pool)
                .await;
        }
    }

    async fn insert_team(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sched auth test')")
            .bind(id)
            .bind(format!("sched-auth-{id}"))
            .execute(pool)
            .await
            .expect("insert team");
        id
    }

    async fn create_status(
        world: &World,
        jwt: JwtUser,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(world.pool.clone()))
                .app_data(web::Data::new(activity_log_repository(world.pool.clone())))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::post()
            .uri(&format!(
                "/api/v1/features/{}/scheduled-changes",
                world.feature_id
            ))
            .set_json(body)
            .to_request();
        req.extensions_mut().insert(jwt);
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body = test::read_body(resp).await;
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    fn schedule_body(action: &str) -> serde_json::Value {
        serde_json::json!({
            "action": action,
            "reason": "scheduled for test",
            "scheduledAt": (Utc::now() + ChronoDuration::hours(1)).to_rfc3339(),
        })
    }

    fn stage_body(stage_id: Uuid, status: &str) -> serde_json::Value {
        let mut body = schedule_body("STAGE_CHANGE");
        body["stageId"] = serde_json::json!(stage_id.to_string());
        body["requestedStatus"] = serde_json::json!(status);
        body
    }

    #[actix_web::test]
    async fn system_client_cannot_schedule_non_stage_change_actions() {
        let mut world = World::new().await;
        let client = world.system_client().await;

        let mut results = Vec::new();
        for action in ["DISABLE_FEATURE", "ENABLE_FEATURE", "ARCHIVE_FEATURE"] {
            results.push(create_status(&world, client.clone(), schedule_body(action)).await);
        }
        let stage_change = create_status(
            &world,
            client,
            stage_body(world.stage_id, "DEPLOYMENT_REQUESTED"),
        )
        .await;
        let created: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM scheduled_feature_changes WHERE feature_id = $1",
        )
        .bind(world.feature_id)
        .fetch_one(&world.pool)
        .await
        .unwrap();
        world.cleanup().await;

        for (status, body) in results {
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(body["code"], "policy_denied");
        }
        assert_eq!(stage_change.0, StatusCode::CREATED, "{}", stage_change.1);
        assert_eq!(created, 1, "only the STAGE_CHANGE was stored");
    }

    #[actix_web::test]
    async fn team_admin_of_another_team_cannot_schedule_feature_actions() {
        let mut world = World::new().await;
        let outsider = world
            .user(Some(world.other_team_id), false, &[TEAM_ADMIN_ROLE])
            .await;
        let plain_member = world
            .user(Some(world.team_id), false, &[REQUESTER_ROLE])
            .await;

        let mut denied = Vec::new();
        for action in ["DISABLE_FEATURE", "ENABLE_FEATURE", "ARCHIVE_FEATURE"] {
            denied.push(create_status(&world, outsider.clone(), schedule_body(action)).await);
        }
        denied.push(create_status(&world, plain_member, schedule_body("DISABLE_FEATURE")).await);
        world.cleanup().await;

        for (status, body) in denied {
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(body["code"], "policy_denied");
        }
    }

    #[actix_web::test]
    async fn admin_and_own_team_admin_can_schedule_feature_actions() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        let team_admin = world
            .user(Some(world.team_id), false, &[TEAM_ADMIN_ROLE])
            .await;

        let mut results = Vec::new();
        for action in ["DISABLE_FEATURE", "ENABLE_FEATURE", "ARCHIVE_FEATURE"] {
            results.push(create_status(&world, admin.clone(), schedule_body(action)).await);
            results.push(create_status(&world, team_admin.clone(), schedule_body(action)).await);
        }
        world.cleanup().await;

        for (status, body) in results {
            assert_eq!(status, StatusCode::CREATED, "{body}");
        }
    }

    #[actix_web::test]
    async fn stage_change_schedule_requires_the_role_for_the_requested_status() {
        let mut world = World::new().await;
        let requester = world
            .user(Some(world.team_id), false, &[REQUESTER_ROLE])
            .await;
        let approver = world
            .user(Some(world.team_id), false, &[APPROVER_ROLE])
            .await;

        let requester_deploy = create_status(
            &world,
            requester.clone(),
            stage_body(world.stage_id, "DEPLOYMENT_REQUESTED"),
        )
        .await;
        let requester_reject = create_status(
            &world,
            requester,
            stage_body(world.stage_id, "DEPLOYMENT_REJECTED"),
        )
        .await;
        let approver_request = create_status(
            &world,
            approver.clone(),
            stage_body(world.stage_id, "DEPLOYMENT_REQUESTED"),
        )
        .await;
        let approver_reject = create_status(
            &world,
            approver,
            stage_body(world.stage_id, "DEPLOYMENT_REJECTED"),
        )
        .await;
        world.cleanup().await;

        assert_eq!(
            requester_deploy.0,
            StatusCode::CREATED,
            "{}",
            requester_deploy.1
        );
        assert_eq!(requester_reject.0, StatusCode::FORBIDDEN);
        assert_eq!(requester_reject.1["code"], "policy_denied");
        assert_eq!(approver_request.0, StatusCode::FORBIDDEN);
        assert_eq!(
            approver_reject.0,
            StatusCode::CREATED,
            "{}",
            approver_reject.1
        );
    }

    #[tokio::test]
    async fn scheduled_change_from_disabled_creator_is_not_executed() {
        let _guard = SCHEDULER_LOCK.lock().await;
        let mut world = World::new().await;
        let creator = world
            .user(Some(world.team_id), false, &[TEAM_ADMIN_ROLE])
            .await;
        let change = world
            .insert_due_change(Some(creator.id), "ARCHIVE_FEATURE")
            .await;
        sqlx::query("UPDATE users SET enabled = FALSE WHERE id = $1")
            .bind(creator.id)
            .execute(&world.pool)
            .await
            .unwrap();

        world.run_scheduler().await;

        let state = world.change_state(change).await;
        let lifecycle = world.lifecycle_stage().await;
        world.cleanup().await;
        assert_eq!(
            state,
            (
                "BLOCKED".to_string(),
                Some(CREATOR_NOT_AUTHORIZED.to_string())
            )
        );
        assert_eq!(lifecycle, "active", "feature must not be archived");
    }

    #[tokio::test]
    async fn scheduled_change_from_deleted_or_demoted_creator_is_not_executed() {
        let _guard = SCHEDULER_LOCK.lock().await;
        let mut world = World::new().await;
        let deleted = world
            .user(Some(world.team_id), false, &[TEAM_ADMIN_ROLE])
            .await;
        let demoted = world
            .user(Some(world.team_id), false, &[TEAM_ADMIN_ROLE])
            .await;
        let moved = world
            .user(Some(world.team_id), false, &[TEAM_ADMIN_ROLE])
            .await;
        let deleted_change = world
            .insert_due_change(Some(deleted.id), "ARCHIVE_FEATURE")
            .await;
        let demoted_change = world
            .insert_due_change(Some(demoted.id), "ARCHIVE_FEATURE")
            .await;
        let moved_change = world
            .insert_due_change(Some(moved.id), "ARCHIVE_FEATURE")
            .await;
        // Deleting the user nulls requested_by (ON DELETE SET NULL).
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(deleted.id)
            .execute(&world.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM user_roles WHERE user_id = $1")
            .bind(demoted.id)
            .execute(&world.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM user_teams WHERE user_id = $1")
            .bind(moved.id)
            .execute(&world.pool)
            .await
            .unwrap();

        world.run_scheduler().await;

        let states = [
            world.change_state(deleted_change).await,
            world.change_state(demoted_change).await,
            world.change_state(moved_change).await,
        ];
        let lifecycle = world.lifecycle_stage().await;
        world.cleanup().await;
        for state in states {
            assert_eq!(
                state,
                (
                    "BLOCKED".to_string(),
                    Some(CREATOR_NOT_AUTHORIZED.to_string())
                )
            );
        }
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    async fn scheduled_change_from_authorized_creator_still_executes() {
        let _guard = SCHEDULER_LOCK.lock().await;
        let mut world = World::new().await;
        let creator = world
            .user(Some(world.team_id), false, &[TEAM_ADMIN_ROLE])
            .await;
        let change = world
            .insert_due_change(Some(creator.id), "ARCHIVE_FEATURE")
            .await;

        world.run_scheduler().await;

        let state = world.change_state(change).await;
        let lifecycle = world.lifecycle_stage().await;
        world.cleanup().await;
        assert_eq!(state.0, "EXECUTED", "{state:?}");
        assert_eq!(lifecycle, "archived");
    }

    #[tokio::test]
    async fn scheduled_non_stage_change_from_system_client_is_not_executed() {
        let _guard = SCHEDULER_LOCK.lock().await;
        let mut world = World::new().await;
        let client = world.system_client().await;
        let change = world
            .insert_due_change(Some(client.id), "ARCHIVE_FEATURE")
            .await;

        world.run_scheduler().await;

        let state = world.change_state(change).await;
        let lifecycle = world.lifecycle_stage().await;
        world.cleanup().await;
        // System clients may only schedule STAGE_CHANGE, so even a stored ARCHIVE
        // from one (for example created before this rule) must not run.
        assert_eq!(
            state,
            (
                "BLOCKED".to_string(),
                Some(CREATOR_NOT_AUTHORIZED.to_string())
            )
        );
        assert_eq!(lifecycle, "active");
    }

    #[tokio::test]
    async fn scheduled_stage_change_needs_creator_to_keep_the_role() {
        let _guard = SCHEDULER_LOCK.lock().await;
        let mut world = World::new().await;
        let creator = world.user(Some(world.team_id), false, &[]).await;
        let change: Uuid = sqlx::query_scalar(
            r#"INSERT INTO scheduled_feature_changes
                   (team_id, feature_id, stage_id, action, requested_status, reason,
                    scheduled_at, requested_by)
               VALUES ($1, $2, $3, 'STAGE_CHANGE', 'DEPLOYMENT_REQUESTED', 'scheduled for test',
                       NOW() - INTERVAL '1 minute', $4)
               RETURNING id"#,
        )
        .bind(world.team_id)
        .bind(world.feature_id)
        .bind(world.stage_id)
        .bind(creator.id)
        .fetch_one(&world.pool)
        .await
        .unwrap();

        world.run_scheduler().await;

        let state = world.change_state(change).await;
        let stage_status: String =
            sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE id = $1")
                .bind(world.stage_id)
                .fetch_one(&world.pool)
                .await
                .unwrap();
        world.cleanup().await;
        assert_eq!(
            state,
            (
                "BLOCKED".to_string(),
                Some(CREATOR_NOT_AUTHORIZED.to_string())
            )
        );
        assert_ne!(stage_status, "DEPLOYMENT_REQUESTED");
    }
    // --- AI-20: reasons are handed to the justification check after the write ---

    use crate::judgment::SubjectType;
    use crate::judgment::justification::test_support::{Recorded, recording_runtime};

    /// Sends one request through the router with an AI runtime registered.
    async fn send_with_ai(
        world: &World,
        method: actix_web::http::Method,
        uri: &str,
        jwt: JwtUser,
        body: serde_json::Value,
        ai: crate::judgment::AiRuntime,
    ) -> (StatusCode, serde_json::Value) {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(world.pool.clone()))
                .app_data(web::Data::new(activity_log_repository(world.pool.clone())))
                .app_data(web::Data::new(ai))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::default()
            .method(method)
            .uri(uri)
            .set_json(body)
            .to_request();
        req.extensions_mut().insert(jwt);
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body = test::read_body(resp).await;
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    fn recorded_entries(recorded: &Recorded) -> Vec<(SubjectType, Uuid, serde_json::Value)> {
        recorded.lock().unwrap().clone()
    }

    impl World {
        async fn scheduled_change_activity_id(&self) -> Uuid {
            sqlx::query_scalar(
                "SELECT id FROM activity_log WHERE activity_type = 'scheduled_change_created' AND entity_id = $1",
            )
            .bind(self.feature_id.to_string())
            .fetch_one(&self.pool)
            .await
            .expect("scheduled change activity")
        }

        async fn delete_freeze_windows(&self) {
            let _ = sqlx::query("DELETE FROM change_freeze_windows WHERE team_id = $1")
                .bind(self.team_id)
                .execute(&self.pool)
                .await;
        }
    }

    #[actix_web::test]
    async fn scheduled_change_reason_is_recorded_against_its_activity_row() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        let recorded: Recorded = Default::default();

        let (status, body) = send_with_ai(
            &world,
            actix_web::http::Method::POST,
            &format!("/api/v1/features/{}/scheduled-changes", world.feature_id),
            admin,
            schedule_body("DISABLE_FEATURE"),
            recording_runtime(true, false, recorded.clone()),
        )
        .await;
        let activity_id = world.scheduled_change_activity_id().await;
        let entries = recorded_entries(&recorded);
        world.cleanup().await;

        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].0, entries[0].1),
            (SubjectType::Activity, activity_id)
        );
        assert_eq!(entries[0].2["reason_kind"], "scheduled_change");
        assert_eq!(entries[0].2["reason"], "scheduled for test");
    }

    #[actix_web::test]
    async fn nothing_is_recorded_for_a_schedule_when_the_team_setting_is_off() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        let recorded: Recorded = Default::default();

        let (status, _) = send_with_ai(
            &world,
            actix_web::http::Method::POST,
            &format!("/api/v1/features/{}/scheduled-changes", world.feature_id),
            admin,
            schedule_body("DISABLE_FEATURE"),
            recording_runtime(false, false, recorded.clone()),
        )
        .await;
        let entries = recorded_entries(&recorded);
        world.cleanup().await;

        assert_eq!(status, StatusCode::CREATED);
        assert!(entries.is_empty());
    }

    #[actix_web::test]
    async fn reschedule_with_a_reason_records_against_the_scheduled_change() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        // Not due: a due change would be claimed by a scheduler run in a parallel test,
        // leaving it non-PENDING and making the reschedule answer 404.
        let change = world
            .insert_future_change(Some(admin.id), "DISABLE_FEATURE")
            .await;
        let recorded: Recorded = Default::default();
        let when = (Utc::now() + ChronoDuration::hours(2)).to_rfc3339();

        let (with_reason, body) = send_with_ai(
            &world,
            actix_web::http::Method::PATCH,
            &format!("/api/v1/scheduled-changes/{change}/reschedule"),
            admin.clone(),
            serde_json::json!({ "scheduledAt": when, "reason": "Moved after the release freeze" }),
            recording_runtime(true, false, recorded.clone()),
        )
        .await;
        let after_first = recorded_entries(&recorded);
        let (without_reason, _) = send_with_ai(
            &world,
            actix_web::http::Method::PATCH,
            &format!("/api/v1/scheduled-changes/{change}/reschedule"),
            admin,
            serde_json::json!({ "scheduledAt": when }),
            recording_runtime(true, false, recorded.clone()),
        )
        .await;
        let after_second = recorded_entries(&recorded);
        world.cleanup().await;

        assert_eq!(with_reason, StatusCode::OK, "{body}");
        assert_eq!(after_first.len(), 1);
        assert_eq!(
            (after_first[0].0, after_first[0].1),
            (SubjectType::ScheduledChange, change)
        );
        assert_eq!(after_first[0].2["reason_kind"], "scheduled_change");
        assert_eq!(without_reason, StatusCode::OK);
        assert_eq!(
            after_second.len(),
            1,
            "no reason sent, nothing new recorded"
        );
    }

    #[actix_web::test]
    async fn freeze_window_reason_is_recorded_on_create_and_update() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        let recorded: Recorded = Default::default();
        let starts = Utc::now() + ChronoDuration::days(1);

        let (created, body) = send_with_ai(
            &world,
            actix_web::http::Method::POST,
            &format!("/api/v1/teams/{}/freeze-windows", world.team_id),
            admin.clone(),
            serde_json::json!({
                "name": "Holiday freeze",
                "environmentType": "Production",
                "startsAt": starts.to_rfc3339(),
                "endsAt": (starts + ChronoDuration::hours(4)).to_rfc3339(),
                "reason": "Holiday traffic peak",
            }),
            recording_runtime(true, false, recorded.clone()),
        )
        .await;
        let window_id =
            Uuid::parse_str(body["id"].as_str().unwrap_or_default()).unwrap_or_default();
        let after_create = recorded_entries(&recorded);

        let (updated, _) = send_with_ai(
            &world,
            actix_web::http::Method::PATCH,
            &format!("/api/v1/freeze-windows/{window_id}"),
            admin.clone(),
            serde_json::json!({ "reason": "Extended through the weekend" }),
            recording_runtime(true, false, recorded.clone()),
        )
        .await;
        let after_update = recorded_entries(&recorded);

        let (renamed, _) = send_with_ai(
            &world,
            actix_web::http::Method::PATCH,
            &format!("/api/v1/freeze-windows/{window_id}"),
            admin,
            serde_json::json!({ "name": "Holiday freeze 2" }),
            recording_runtime(true, false, recorded.clone()),
        )
        .await;
        let after_rename = recorded_entries(&recorded);
        world.delete_freeze_windows().await;
        world.cleanup().await;

        assert_eq!(created, StatusCode::CREATED, "{body}");
        assert_eq!(after_create.len(), 1);
        assert_eq!(
            (after_create[0].0, after_create[0].1),
            (SubjectType::FreezeWindow, window_id)
        );
        assert_eq!(after_create[0].2["reason_kind"], "freeze_window");
        assert_eq!(after_create[0].2["reason"], "Holiday traffic peak");
        assert_eq!(updated, StatusCode::OK);
        assert_eq!(after_update.len(), 2);
        assert_eq!(after_update[1].2["reason"], "Extended through the weekend");
        assert_eq!(renamed, StatusCode::OK);
        assert_eq!(
            after_rename.len(),
            2,
            "no reason in the patch, nothing recorded"
        );
    }

    #[actix_web::test]
    async fn freeze_override_returns_the_activity_row_it_wrote() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        let environment_id: Uuid =
            sqlx::query_scalar("SELECT environment_id FROM features_pipeline_stages WHERE id = $1")
                .bind(world.stage_id)
                .fetch_one(&world.pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO change_freeze_windows (team_id, name, environment_id, starts_at, ends_at, timezone, recurrence, active) \
             VALUES ($1, 'Active freeze', $2, NOW() - INTERVAL '1 hour', NOW() + INTERVAL '1 hour', 'UTC', 'NONE', true)",
        )
        .bind(world.team_id)
        .bind(environment_id)
        .execute(&world.pool)
        .await
        .unwrap();
        let activity = activity_log_repository(world.pool.clone());

        let overridden = enforce_freeze_for_feature_environment(
            &world.pool,
            activity.as_ref(),
            world.team_id,
            world.feature_id,
            "checkout-v2",
            environment_id,
            &admin,
            Some("Customer outage, hotfix needed"),
        )
        .await
        .expect("admin may override")
        .expect("an override was used");
        let row: (String, Option<serde_json::Value>) =
            sqlx::query_as("SELECT activity_type, metadata FROM activity_log WHERE id = $1")
                .bind(overridden.activity_id)
                .fetch_one(&world.pool)
                .await
                .unwrap();
        world.delete_freeze_windows().await;
        let team_id = world.team_id;
        world.cleanup().await;

        assert_eq!(overridden.team_id, team_id);
        assert_eq!(overridden.feature_key, "checkout-v2");
        assert_eq!(overridden.reason, "Customer outage, hotfix needed");
        assert_eq!(row.0, "freeze_override");
        assert_eq!(
            row.1.unwrap()["override_reason"],
            "Customer outage, hotfix needed"
        );
    }

    #[actix_web::test]
    async fn no_freeze_means_no_override() {
        let mut world = World::new().await;
        let admin = world.user(None, true, &[]).await;
        let activity = activity_log_repository(world.pool.clone());
        let outcome = enforce_freeze_for_feature_environment(
            &world.pool,
            activity.as_ref(),
            world.team_id,
            world.feature_id,
            "checkout-v2",
            Uuid::new_v4(),
            &admin,
            None,
        )
        .await
        .expect("no freeze, no error");
        world.cleanup().await;
        assert!(outcome.is_none());
    }
}
