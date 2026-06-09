use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, patch, post, web};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
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
) -> Result<(), RestError> {
    activity_repo
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
    Ok(())
}

pub(crate) async fn enforce_freeze_for_feature_environment(
    pool: &PgPool,
    activity_repo: &dyn ActivityLogRepository,
    team_id: Uuid,
    feature_id: Uuid,
    feature_key: &str,
    environment_id: Uuid,
    jwt: &JwtUser,
    override_reason: Option<&str>,
) -> Result<(), RestError> {
    let Some(window) = active_freeze_for_environment(pool, team_id, environment_id, Utc::now())
        .await
        .map_err(RestError::from)?
    else {
        return Ok(());
    };

    if can_operate_safety(jwt) {
        let reason = validate_reason(
            override_reason.unwrap_or_default(),
            "freeze override reason",
        )?;
        log_freeze_attempt(
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
        return Ok(());
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
) -> Result<(), RestError> {
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
    if payload.action == ScheduledChangeAction::StageChange {
        let stage_id = stage_id.ok_or_else(|| {
            RestError::invalid_input("stageId is required for STAGE_CHANGE schedules")
        })?;
        let requested_status = payload.requested_status.as_deref().ok_or_else(|| {
            RestError::invalid_input("requestedStatus is required for STAGE_CHANGE schedules")
        })?;
        if stage_request_from_status(requested_status).is_none() {
            return Err(RestError::invalid_input("requestedStatus is not supported"));
        }
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

    let _ = activity_repo
        .create_activity(CreateActivityLog {
            activity_type: "scheduled_change_created".to_string(),
            entity_type: "feature".to_string(),
            entity_id: feature_id.to_string(),
            actor_id: Some(jwt.id),
            actor_name: Some(jwt.username),
            description: format!(
                "Scheduled {} for feature '{}'",
                payload.action.as_str(),
                feature.get::<String, _>("key")
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
