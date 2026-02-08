use actix_web::{HttpRequest, HttpResponse, get, web};
use actix_ws::{Message, Session};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::database::activity_log::{ActivityLogFilter, ActivityLogRepository};
use crate::database::approval::ApprovalRepository;
use crate::database::feature::FeatureRepository;
use crate::logic::approval::ApprovalRequestEvent;
use crate::logic::client::ClientLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::feature_evaluation::{FeatureEvaluationEvent, FeatureEvaluationLogic};
use crate::logic::pipeline::PipelineLogic;
use crate::model::ID;
use crate::rest::approval::{
    ApprovalRequestResponse, ApprovalRequestStatus, ApprovalRequestsResponse, ApprovalVoteResponse,
};
use crate::rest::error::ErrorResponse;
use crate::rest::metrics::{
    ActivityEntityDetailsResponse, ActivityLogPageResponse, ActivityLogResponse,
    EvaluationByFeatureResponse, EvaluationRateResponse, EvaluationSummaryResponse,
    EvaluationsByFeatureResponse, FeatureGrowthResponse, SystemMetricsResponse,
};
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};
use crate::rest::serde::deserialize_optional_string_or_vec;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamQuery {
    stream: String,
    team_id: Option<String>,
    feature_key: Option<String>,
    environment_id: Option<String>,
    client_id: Option<String>,
    period: Option<String>,
    interval_minutes: Option<i32>,
    from_time: Option<String>,
    to_time: Option<String>,
    duration_hours: Option<i64>,
    offset: Option<i64>,
    limit: Option<i64>,
    interval: Option<String>,
    statuses: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    activity_types: Option<Vec<String>>,
    entity_type: Option<String>,
    entity_id: Option<String>,
    actor_id: Option<String>,
    from_date: Option<String>,
    to_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamType {
    EvaluationSummary,
    EvaluationRates,
    EvaluationsByFeature,
    EvaluationDashboard,
    SystemMetrics,
    RecentActivities,
    FeatureGrowth,
    ApprovalRequests,
}

impl StreamType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "evaluationSummary" | "evaluation_summary" | "evaluation-summary" => {
                Some(StreamType::EvaluationSummary)
            }
            "evaluationRates" | "evaluation_rates" | "evaluation-rates" => {
                Some(StreamType::EvaluationRates)
            }
            "evaluationsByFeature" | "evaluations_by_feature" | "evaluations-by-feature" => {
                Some(StreamType::EvaluationsByFeature)
            }
            "evaluationDashboard" | "evaluation_dashboard" | "evaluation-dashboard" => {
                Some(StreamType::EvaluationDashboard)
            }
            "systemMetrics" | "system_metrics" | "system-metrics" => {
                Some(StreamType::SystemMetrics)
            }
            "recentActivities" | "recent_activities" | "recent-activities" => {
                Some(StreamType::RecentActivities)
            }
            "featureGrowth" | "feature_growth" | "feature-growth" => {
                Some(StreamType::FeatureGrowth)
            }
            "approvalRequests" | "approval_requests" | "approval-requests" => {
                Some(StreamType::ApprovalRequests)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvaluationDashboardResponse {
    generated_at: String,
    rates: Vec<EvaluationRateResponse>,
    summary: EvaluationSummaryResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamErrorPayload {
    error: String,
    message: String,
    code: Option<String>,
    details: Option<Value>,
}

fn error_payload(error: &str, message: impl Into<String>) -> StreamErrorPayload {
    StreamErrorPayload {
        error: error.to_string(),
        message: message.into(),
        code: None,
        details: None,
    }
}

fn parse_opt_uuid(value: Option<&String>, field: &str) -> Result<Option<Uuid>, String> {
    match value {
        Some(raw) => Uuid::parse_str(raw)
            .map(Some)
            .map_err(|_| format!("invalid {field}")),
        None => Ok(None),
    }
}

fn parse_datetime(value: &str, field: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| format!("invalid {field}"))
}

fn parse_time_period(value: &str) -> Result<crate::streaming::TimePeriod, String> {
    match value {
        "PERIOD_24H" | "H24" | "24H" => Ok(crate::streaming::TimePeriod::H24),
        "PERIOD_7D" | "D7" | "7D" => Ok(crate::streaming::TimePeriod::D7),
        "PERIOD_30D" | "D30" | "30D" => Ok(crate::streaming::TimePeriod::D30),
        _ => Err("invalid period".to_string()),
    }
}

fn round_pct(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn map_evaluation_rate(
    point: crate::database::feature_evaluation::EvaluationRatePoint,
) -> EvaluationRateResponse {
    let success_rate = if point.evaluation_count > 0 {
        (point.success_count as f64 / point.evaluation_count as f64) * 100.0
    } else {
        0.0
    };
    let cache_hit_rate = if point.evaluation_count > 0 {
        (point.prior_assignment_count as f64 / point.evaluation_count as f64) * 100.0
    } else {
        0.0
    };

    EvaluationRateResponse {
        time_bucket: point.time_bucket.to_rfc3339(),
        evaluation_count: point.evaluation_count,
        success_count: point.success_count,
        prior_assignment_count: point.prior_assignment_count,
        success_rate: round_pct(success_rate),
        cache_hit_rate: round_pct(cache_hit_rate),
    }
}

fn map_evaluation_summary(
    summary: crate::database::feature_evaluation::EvaluationSummary,
) -> EvaluationSummaryResponse {
    EvaluationSummaryResponse {
        total_evaluations: summary.total_evaluations,
        successful_evaluations: summary.successful_evaluations,
        cached_evaluations: summary.cached_evaluations,
        unique_users: summary.unique_users,
        top_feature_key: summary.top_feature_key,
        success_rate: round_pct(summary.success_rate),
        cache_hit_rate: round_pct(summary.cache_hit_rate),
    }
}

fn validate_interval_minutes(interval_minutes: i32) -> Result<(), String> {
    if !(1..=60).contains(&interval_minutes) {
        return Err("intervalMinutes must be between 1 and 60".to_string());
    }
    Ok(())
}

fn validate_time_range(from: DateTime<Utc>, to: DateTime<Utc>) -> Result<(), String> {
    if to < from {
        return Err("toTime must be >= fromTime".to_string());
    }
    Ok(())
}

fn validate_feature_growth_interval(interval: &str) -> Result<(), String> {
    match interval {
        "day" | "week" | "month" => Ok(()),
        _ => Err("interval must be day, week, or month".to_string()),
    }
}

async fn send_json<T: Serialize>(session: &mut Session, payload: &T) -> bool {
    match serde_json::to_string(payload) {
        Ok(text) => session.text(text).await.is_ok(),
        Err(_) => session
            .text("{\"error\":\"internal\",\"message\":\"serialization error\"}")
            .await
            .is_ok(),
    }
}

async fn send_error(session: &mut Session, error: &str, message: impl Into<String>) {
    let payload = error_payload(error, message);
    let _ = send_json(session, &payload).await;
}

#[derive(Default)]
struct ActivityEntityLookupCache {
    feature_by_id: HashMap<Uuid, Option<crate::database::entity::Feature>>,
    feature_id_by_stage_id: HashMap<Uuid, Option<Uuid>>,
    feature_stages_by_feature_id:
        HashMap<Uuid, Option<Vec<crate::database::entity::FeaturePipelineStage>>>,
    environment_by_id: HashMap<Uuid, Option<crate::model::Environment>>,
}

impl ActivityEntityLookupCache {
    async fn feature(
        &mut self,
        feature_id: Uuid,
        feature_repo: &dyn FeatureRepository,
    ) -> Option<crate::database::entity::Feature> {
        if let Some(cached) = self.feature_by_id.get(&feature_id) {
            return cached.clone();
        }

        let resolved = feature_repo.get_feature_by_id(feature_id).await.ok();
        self.feature_by_id.insert(feature_id, resolved.clone());
        resolved
    }

    async fn feature_id_for_stage(
        &mut self,
        stage_id: Uuid,
        feature_repo: &dyn FeatureRepository,
    ) -> Option<Uuid> {
        if let Some(cached) = self.feature_id_by_stage_id.get(&stage_id) {
            return *cached;
        }

        let resolved = feature_repo
            .get_feature_id_by_stage_id(stage_id)
            .await
            .ok()
            .flatten();
        self.feature_id_by_stage_id.insert(stage_id, resolved);
        resolved
    }

    async fn feature_stages(
        &mut self,
        feature_id: Uuid,
        feature_repo: &dyn FeatureRepository,
    ) -> Option<Vec<crate::database::entity::FeaturePipelineStage>> {
        if let Some(cached) = self.feature_stages_by_feature_id.get(&feature_id) {
            return cached.clone();
        }

        let resolved = feature_repo.get_feature_stages(feature_id).await.ok();
        self.feature_stages_by_feature_id
            .insert(feature_id, resolved.clone());
        resolved
    }

    async fn environment(
        &mut self,
        environment_id: Uuid,
        environment_logic: &dyn EnvironmentLogic,
    ) -> Option<crate::model::Environment> {
        if let Some(cached) = self.environment_by_id.get(&environment_id) {
            return cached.clone();
        }

        let resolved = environment_logic
            .get_environment_by_id(ID::from(environment_id))
            .await
            .ok();
        self.environment_by_id
            .insert(environment_id, resolved.clone());
        resolved
    }
}

async fn resolve_activity_entity_details(
    activity: &crate::database::activity_log::ActivityLogRow,
    feature_repo: &dyn FeatureRepository,
    environment_logic: &dyn EnvironmentLogic,
    cache: &mut ActivityEntityLookupCache,
) -> Option<ActivityEntityDetailsResponse> {
    let entity_type = activity.entity_type.as_str();
    let entity_id = activity.entity_id.as_str();

    match entity_type {
        "stage" => {
            if let Ok(stage_uuid) = Uuid::parse_str(entity_id)
                && let Some(feature_id) = cache.feature_id_for_stage(stage_uuid, feature_repo).await
                && let Some(feature) = cache.feature(feature_id, feature_repo).await
                && let Some(stages) = cache.feature_stages(feature_id, feature_repo).await
                && let Some(stage) = stages.iter().find(|s| s.id == stage_uuid)
            {
                let environment = cache
                    .environment(stage.environment_id, environment_logic)
                    .await;

                let environment_name = environment
                    .as_ref()
                    .map(|env| env.name.clone())
                    .unwrap_or_else(|| format!("Stage ({})", stage.status));

                let stage_details = serde_json::json!({
                    "id": stage.id.to_string(),
                    "status": stage.status,
                    "order_index": stage.order_index,
                    "position": stage.position,
                    "environment": environment.as_ref().map(|env| serde_json::json!({
                        "id": env.id.to_string(),
                        "name": env.name,
                        "active": env.active,
                    }))
                });

                return Some(ActivityEntityDetailsResponse {
                    id: entity_id.to_string(),
                    name: format!("{} - {}", feature.key, environment_name),
                    entity_type: entity_type.to_string(),
                    details: Some(serde_json::json!({
                        "feature_key": feature.key,
                        "feature_id": feature_id.to_string(),
                        "stage": stage_details,
                    })),
                });
            }

            if let Some(meta) = activity.metadata.as_ref()
                && let (Some(feature_key), Some(status)) = (
                    meta.get("feature_key").and_then(|v| v.as_str()),
                    meta.get("status").and_then(|v| v.as_str()),
                )
            {
                return Some(ActivityEntityDetailsResponse {
                    id: entity_id.to_string(),
                    name: format!("{} ({})", feature_key, status),
                    entity_type: entity_type.to_string(),
                    details: Some(meta.clone()),
                });
            }

            None
        }
        "feature" => {
            if let Ok(feature_uuid) = Uuid::parse_str(entity_id)
                && let Some(feature) = cache.feature(feature_uuid, feature_repo).await
            {
                return Some(ActivityEntityDetailsResponse {
                    id: entity_id.to_string(),
                    name: feature.key.clone(),
                    entity_type: entity_type.to_string(),
                    details: Some(serde_json::json!({
                        "feature_key": feature.key,
                        "feature_id": feature_uuid.to_string(),
                        "description": feature.description,
                    })),
                });
            }

            if let Some(meta) = activity.metadata.as_ref()
                && let Some(feature_key) = meta.get("feature_key").and_then(|v| v.as_str())
            {
                return Some(ActivityEntityDetailsResponse {
                    id: entity_id.to_string(),
                    name: feature_key.to_string(),
                    entity_type: entity_type.to_string(),
                    details: Some(meta.clone()),
                });
            }

            None
        }
        _ => {
            let name = activity
                .metadata
                .as_ref()
                .and_then(|m| m.get("name").or_else(|| m.get("key")))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| entity_id.to_string());

            Some(ActivityEntityDetailsResponse {
                id: entity_id.to_string(),
                name,
                entity_type: entity_type.to_string(),
                details: activity.metadata.clone(),
            })
        }
    }
}

fn parse_statuses(
    value: Option<&str>,
) -> Result<Option<Vec<crate::database::entity::ApprovalStatus>>, String> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let statuses = raw
        .split(',')
        .map(|item| item.trim().to_lowercase())
        .map(|status| match status.as_str() {
            "pending" => Ok(crate::database::entity::ApprovalStatus::Pending),
            "approved" => Ok(crate::database::entity::ApprovalStatus::Approved),
            "rejected" => Ok(crate::database::entity::ApprovalStatus::Rejected),
            "cancelled" => Ok(crate::database::entity::ApprovalStatus::Cancelled),
            "auto_approved" | "autoapproved" | "auto-approved" => {
                Ok(crate::database::entity::ApprovalStatus::AutoApproved)
            }
            _ => Err(format!("invalid status: {}", status)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(statuses))
}

fn map_vote(vote: crate::database::entity::ApprovalVote) -> ApprovalVoteResponse {
    ApprovalVoteResponse {
        id: vote.id.to_string(),
        approver_id: vote.approver_id.to_string(),
        vote: vote.vote.as_str().to_string(),
        comment: vote.comment,
        created_at: vote.created_at,
    }
}

fn map_request(
    request: crate::database::entity::ApprovalRequest,
    votes: Vec<crate::database::entity::ApprovalVote>,
) -> ApprovalRequestResponse {
    ApprovalRequestResponse {
        id: request.id.to_string(),
        policy_id: request.policy_id.to_string(),
        feature_id: request.feature_id.to_string(),
        environment_id: request.environment_id.map(|id| id.to_string()),
        change_type: request.change_type,
        change_payload: request.change_payload,
        change_description: request.change_description,
        requested_by: request.requested_by.to_string(),
        status: ApprovalRequestStatus::from(request.status),
        approved_count: request.approved_count,
        rejected_count: request.rejected_count,
        executed_at: request.executed_at,
        created_at: request.created_at,
        updated_at: request.updated_at,
        votes: votes.into_iter().map(map_vote).collect(),
    }
}

async fn send_evaluation_summary(
    session: &mut Session,
    logic: &Box<dyn FeatureEvaluationLogic>,
    query: &StreamQuery,
) -> Result<(), String> {
    let period = parse_time_period(
        query
            .period
            .as_deref()
            .ok_or_else(|| "period is required".to_string())?,
    )?;
    let (from_time, to_time) = crate::streaming::calculate_time_range(period, Utc::now());
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "clientId")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")?;

    let summary = logic
        .get_evaluation_summary(
            query.feature_key.clone(),
            query.environment_id.clone(),
            client_id,
            team_id,
            from_time,
            to_time,
        )
        .await
        .map_err(|e| format!("Failed to load evaluation summary: {e}"))?;

    let response = map_evaluation_summary(summary);
    if !send_json(session, &response).await {
        return Err("failed to send summary".to_string());
    }
    Ok(())
}

async fn send_evaluation_rates(
    session: &mut Session,
    logic: &Box<dyn FeatureEvaluationLogic>,
    query: &StreamQuery,
) -> Result<(), String> {
    let interval_minutes = query
        .interval_minutes
        .ok_or_else(|| "intervalMinutes is required".to_string())?;
    validate_interval_minutes(interval_minutes)?;
    let period = parse_time_period(
        query
            .period
            .as_deref()
            .ok_or_else(|| "period is required".to_string())?,
    )?;
    let (from_time, to_time) = crate::streaming::calculate_time_range(period, Utc::now());
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "clientId")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")?;

    let rates = logic
        .get_evaluation_rates(
            query.feature_key.clone(),
            query.environment_id.clone(),
            client_id,
            team_id,
            from_time,
            to_time,
            interval_minutes,
        )
        .await
        .map_err(|e| format!("Failed to load evaluation rates: {e}"))?;

    let response = rates
        .into_iter()
        .map(map_evaluation_rate)
        .collect::<Vec<_>>();
    if !send_json(session, &response).await {
        return Err("failed to send rates".to_string());
    }
    Ok(())
}

async fn send_evaluations_by_feature(
    session: &mut Session,
    logic: &Box<dyn FeatureEvaluationLogic>,
    query: &StreamQuery,
) -> Result<(), String> {
    let (from_time, to_time) = match (query.from_time.as_deref(), query.to_time.as_deref()) {
        (Some(from_raw), Some(to_raw)) => (
            parse_datetime(from_raw, "fromTime")?,
            parse_datetime(to_raw, "toTime")?,
        ),
        _ => {
            let period = parse_time_period(
                query
                    .period
                    .as_deref()
                    .ok_or_else(|| "fromTime/toTime or period is required".to_string())?,
            )?;
            crate::streaming::calculate_time_range(period, Utc::now())
        }
    };
    validate_time_range(from_time, to_time)?;
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "clientId")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")?;

    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let results = logic
        .get_evaluations_by_feature(
            from_time,
            to_time,
            query.environment_id.clone(),
            client_id,
            team_id,
            Some(limit as i32),
            Some(offset as i32),
        )
        .await
        .map_err(|e| format!("Failed to load evaluations by feature: {e}"))?;

    let items = results
        .into_iter()
        .map(|row| EvaluationByFeatureResponse {
            feature_key: row.feature_key,
            total_evaluations: row.total_evaluations,
            successful_evaluations: row.successful_evaluations,
            cached_evaluations: row.cached_evaluations,
            unique_users: row.unique_users,
            last_evaluated_at: row.last_evaluated_at,
        })
        .collect::<Vec<_>>();

    let total = items.len() as i64;
    let response = EvaluationsByFeatureResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    };

    if !send_json(session, &response).await {
        return Err("failed to send evaluations by feature".to_string());
    }
    Ok(())
}

async fn send_system_metrics(
    session: &mut Session,
    feature_logic: &Box<dyn FeatureLogic>,
    client_logic: &Box<dyn ClientLogic>,
    evaluation_logic: &Box<dyn FeatureEvaluationLogic>,
    team_id: Option<Uuid>,
) -> Result<(), String> {
    let now = Utc::now();
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let today_end = now;
    let yesterday_start = (now - chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let yesterday_end = today_start;
    let (from_7d, to_7d) =
        crate::streaming::calculate_time_range(crate::streaming::TimePeriod::D7, now);

    let team_id_arg = team_id.map(ID::from);

    let (
        total_features_result,
        active_clients_result,
        total_clients_result,
        evaluations_today_result,
        evaluations_yesterday_result,
        summary_7d_result,
    ) = tokio::join!(
        feature_logic.count_features(team_id_arg.clone()),
        client_logic.count_clients(team_id_arg.clone(), Some(true)),
        client_logic.count_clients(team_id_arg.clone(), None),
        evaluation_logic.count_evaluations(today_start, today_end, None, None, None, team_id),
        evaluation_logic.count_evaluations(
            yesterday_start,
            yesterday_end,
            None,
            None,
            None,
            team_id
        ),
        evaluation_logic.get_evaluation_summary(None, None, None, team_id, from_7d, to_7d)
    );

    let (
        total_features,
        active_clients,
        total_clients,
        evaluations_today,
        evaluations_yesterday,
        summary_7d,
    ) = match (
        total_features_result,
        active_clients_result,
        total_clients_result,
        evaluations_today_result,
        evaluations_yesterday_result,
        summary_7d_result,
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d), Ok(e), Ok(f)) => (a, b, c, d, e, f),
        _ => return Err("Failed to fetch system metrics".to_string()),
    };

    let response = SystemMetricsResponse {
        total_features,
        active_clients,
        total_clients,
        evaluations_today,
        evaluations_yesterday,
        success_rate: round_pct(summary_7d.success_rate),
        total_evaluations_7d: summary_7d.total_evaluations,
        successful_evaluations_7d: summary_7d.successful_evaluations,
        generated_at: now.to_rfc3339(),
    };

    if !send_json(session, &response).await {
        return Err("failed to send system metrics".to_string());
    }
    Ok(())
}

async fn send_recent_activities(
    session: &mut Session,
    repo: &Box<dyn ActivityLogRepository>,
    feature_repo: &Box<dyn FeatureRepository>,
    environment_logic: &Box<dyn EnvironmentLogic>,
    client_logic: &Box<dyn ClientLogic>,
    pipeline_logic: &Box<dyn PipelineLogic>,
    query: &StreamQuery,
) -> Result<(), String> {
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let actor_id = parse_opt_uuid(query.actor_id.as_ref(), "actorId")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")?;
    let from_date = match query.from_date.as_deref() {
        Some(value) => Some(parse_datetime(value, "fromDate")?),
        None => None,
    };
    let to_date = match query.to_date.as_deref() {
        Some(value) => Some(parse_datetime(value, "toDate")?),
        None => None,
    };

    let filter = ActivityLogFilter {
        activity_types: query.activity_types.clone(),
        entity_type: query.entity_type.clone(),
        entity_id: query.entity_id.clone(),
        actor_id,
        from_date,
        to_date,
        limit: Some(limit as i32),
        offset: Some(offset as i32),
        team_id,
    };

    let (activities, total) = repo
        .get_activities_paginated(filter.clone())
        .await
        .map_err(|e| format!("Failed to load activities: {e}"))?;

    let mut items = Vec::new();
    let mut filtered_count: i64 = 0;
    let feature_repo_arc = std::sync::Arc::new(feature_repo.clone());
    let feature_repo_ref = feature_repo.as_ref();
    let environment_logic_ref = environment_logic.as_ref();
    let client_logic_ref = client_logic.as_ref();
    let pipeline_logic_ref = pipeline_logic.as_ref();
    let mut team_cache = crate::streaming::ActivityTeamMatchCache::default();
    let mut entity_cache = ActivityEntityLookupCache::default();

    for activity in activities.into_iter() {
        if let Some(team_id) = team_id
            && !crate::streaming::activity_matches_team_cached(
                &activity,
                team_id,
                &feature_repo_arc,
                environment_logic_ref,
                client_logic_ref,
                pipeline_logic_ref,
                &mut team_cache,
            )
            .await
        {
            continue;
        }

        filtered_count += 1;
        let entity_details = resolve_activity_entity_details(
            &activity,
            feature_repo_ref,
            environment_logic_ref,
            &mut entity_cache,
        )
        .await;

        items.push(ActivityLogResponse {
            id: activity.id.to_string(),
            activity_type: activity.activity_type,
            entity_type: activity.entity_type.clone(),
            entity_id: activity.entity_id.clone(),
            entity_details,
            actor_id: activity.actor_id.map(|id| id.to_string()),
            actor_name: activity.actor_name,
            description: activity.description,
            metadata: activity.metadata,
            created_at: activity.created_at,
        });
    }

    let response = ActivityLogPageResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total: if team_id.is_some() {
                filtered_count
            } else {
                total
            },
        },
    };

    if !send_json(session, &response).await {
        return Err("failed to send activities".to_string());
    }
    Ok(())
}

async fn send_feature_growth(
    session: &mut Session,
    repo: &Box<dyn FeatureRepository>,
    query: &StreamQuery,
) -> Result<(), String> {
    let from_time = parse_datetime(
        query
            .from_time
            .as_deref()
            .ok_or_else(|| "fromTime is required".to_string())?,
        "fromTime",
    )?;
    let to_time = parse_datetime(
        query
            .to_time
            .as_deref()
            .ok_or_else(|| "toTime is required".to_string())?,
        "toTime",
    )?;
    validate_time_range(from_time, to_time)?;
    let interval = query
        .interval
        .clone()
        .ok_or_else(|| "interval is required".to_string())?;
    validate_feature_growth_interval(&interval)?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")?;

    let results = repo
        .get_feature_growth(from_time, to_time, interval, team_id)
        .await
        .map_err(|e| format!("Failed to load feature growth: {e}"))?;

    let response = results
        .into_iter()
        .map(|row| FeatureGrowthResponse {
            time_bucket: row.time_bucket,
            team_id: row.team_id.map(|id| id.to_string()),
            team_name: row.team_name,
            feature_count: row.feature_count,
            cumulative_count: row.cumulative_count,
        })
        .collect::<Vec<_>>();

    if !send_json(session, &response).await {
        return Err("failed to send feature growth".to_string());
    }
    Ok(())
}

async fn send_approval_requests(
    session: &mut Session,
    repo: &Box<dyn ApprovalRepository>,
    query: &StreamQuery,
) -> Result<(), String> {
    let team_id = query
        .team_id
        .as_deref()
        .ok_or_else(|| "teamId is required".to_string())?;
    let team_uuid = Uuid::parse_str(team_id).map_err(|_| "invalid teamId".to_string())?;

    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let statuses = parse_statuses(query.statuses.as_deref())?;

    let (requests, total) = repo
        .list_requests_for_team_with_offset(Some(team_uuid), statuses, offset, limit)
        .await
        .map_err(|e| format!("Failed to load approval requests: {e}"))?;

    let mut items = Vec::with_capacity(requests.len());
    for request in requests {
        let votes = repo
            .list_votes_for_request(request.id)
            .await
            .map_err(|e| format!("Failed to load approval votes: {e}"))?;
        items.push(map_request(request, votes));
    }

    let response = ApprovalRequestsResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    };

    if !send_json(session, &response).await {
        return Err("failed to send approval requests".to_string());
    }
    Ok(())
}

async fn send_evaluation_dashboard(
    session: &mut Session,
    logic: &Box<dyn FeatureEvaluationLogic>,
    query: &StreamQuery,
) -> Result<(), String> {
    let interval_minutes = query
        .interval_minutes
        .ok_or_else(|| "intervalMinutes is required".to_string())?;
    validate_interval_minutes(interval_minutes)?;
    let duration_hours = query
        .duration_hours
        .ok_or_else(|| "durationHours is required".to_string())?;
    if duration_hours <= 0 || duration_hours > 24 {
        return Err("durationHours must be between 1 and 24".to_string());
    }

    let now = Utc::now();
    let from_time = now - chrono::Duration::hours(duration_hours);
    let to_time = now;
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "clientId")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")?;

    let (rates_result, summary_result) = tokio::join!(
        logic.get_evaluation_rates(
            query.feature_key.clone(),
            query.environment_id.clone(),
            client_id,
            team_id,
            from_time,
            to_time,
            interval_minutes
        ),
        logic.get_evaluation_summary(
            query.feature_key.clone(),
            query.environment_id.clone(),
            client_id,
            team_id,
            from_time,
            to_time
        )
    );

    let rates = rates_result.map_err(|e| format!("Failed to load evaluation rates: {e}"))?;
    let summary = summary_result.map_err(|e| format!("Failed to load evaluation summary: {e}"))?;

    let response = EvaluationDashboardResponse {
        generated_at: now.to_rfc3339(),
        rates: rates.into_iter().map(map_evaluation_rate).collect(),
        summary: map_evaluation_summary(summary),
    };

    if !send_json(session, &response).await {
        return Err("failed to send evaluation dashboard".to_string());
    }
    Ok(())
}

#[get("/ws")]
pub async fn stream_ws(
    req: HttpRequest,
    payload: web::Payload,
    evaluation_logic: web::Data<Box<dyn FeatureEvaluationLogic>>,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    client_logic: web::Data<Box<dyn ClientLogic>>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    approval_repo: web::Data<Box<dyn ApprovalRepository>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    environment_logic: web::Data<Box<dyn EnvironmentLogic>>,
    pipeline_logic: web::Data<Box<dyn PipelineLogic>>,
    evaluation_events: web::Data<broadcast::Sender<FeatureEvaluationEvent>>,
    approval_events: web::Data<broadcast::Sender<ApprovalRequestEvent>>,
) -> Result<HttpResponse, actix_web::Error> {
    let query = match web::Query::<StreamQuery>::from_query(req.query_string()) {
        Ok(value) => value,
        Err(_) => {
            let response = HttpResponse::BadRequest().json(ErrorResponse::new(
                "invalid_input",
                "Invalid query parameters",
            ));
            return Ok(response);
        }
    };

    let stream_type = match StreamType::parse(&query.stream) {
        Some(stream_type) => stream_type,
        None => {
            let response = HttpResponse::BadRequest()
                .json(ErrorResponse::new("invalid_input", "Unknown stream type"));
            return Ok(response);
        }
    };

    let (response, session, mut msg_stream) = actix_ws::handle(&req, payload)?;

    let evaluation_logic = evaluation_logic.into_inner();
    let feature_logic = feature_logic.into_inner();
    let client_logic = client_logic.into_inner();
    let activity_repo = activity_repo.into_inner();
    let approval_repo = approval_repo.into_inner();
    let feature_repo = feature_repo.into_inner();
    let environment_logic = environment_logic.into_inner();
    let pipeline_logic = pipeline_logic.into_inner();

    let mut eval_events_rx = evaluation_events.subscribe();
    let mut approval_events_rx = approval_events.subscribe();
    let query = query.into_inner();
    let mut session_clone = session.clone();

    actix_web::rt::spawn(async move {
        let mut interval = match stream_type {
            StreamType::RecentActivities => {
                Some(tokio::time::interval(tokio::time::Duration::from_secs(45)))
            }
            StreamType::FeatureGrowth => {
                Some(tokio::time::interval(tokio::time::Duration::from_secs(60)))
            }
            StreamType::SystemMetrics => {
                Some(tokio::time::interval(tokio::time::Duration::from_secs(30)))
            }
            _ => None,
        };

        if let Some(timer) = interval.as_mut() {
            timer.tick().await;
        }

        let send_result = match stream_type {
            StreamType::EvaluationSummary => {
                send_evaluation_summary(&mut session_clone, &evaluation_logic, &query).await
            }
            StreamType::EvaluationRates => {
                send_evaluation_rates(&mut session_clone, &evaluation_logic, &query).await
            }
            StreamType::EvaluationsByFeature => {
                send_evaluations_by_feature(&mut session_clone, &evaluation_logic, &query).await
            }
            StreamType::EvaluationDashboard => {
                send_evaluation_dashboard(&mut session_clone, &evaluation_logic, &query).await
            }
            StreamType::SystemMetrics => {
                let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId")
                    .ok()
                    .flatten();
                send_system_metrics(
                    &mut session_clone,
                    &feature_logic,
                    &client_logic,
                    &evaluation_logic,
                    team_id,
                )
                .await
            }
            StreamType::RecentActivities => {
                send_recent_activities(
                    &mut session_clone,
                    &activity_repo,
                    &feature_repo,
                    &environment_logic,
                    &client_logic,
                    &pipeline_logic,
                    &query,
                )
                .await
            }
            StreamType::FeatureGrowth => {
                send_feature_growth(&mut session_clone, &feature_repo, &query).await
            }
            StreamType::ApprovalRequests => {
                send_approval_requests(&mut session_clone, &approval_repo, &query).await
            }
        };

        if let Err(err) = send_result {
            send_error(&mut session_clone, "invalid_input", err).await;
            let _ = session_clone.close(None).await;
            return;
        }

        loop {
            tokio::select! {
                maybe_msg = msg_stream.next() => {
                    match maybe_msg {
                        Some(Ok(Message::Ping(bytes))) => {
                            let _ = session_clone.pong(&bytes).await;
                        }
                        Some(Ok(Message::Close(reason))) => {
                            let _ = session_clone.close(reason).await;
                            break;
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => {
                            break;
                        }
                    }
                }
                event = eval_events_rx.recv(), if matches!(stream_type, StreamType::EvaluationSummary | StreamType::EvaluationRates | StreamType::EvaluationsByFeature | StreamType::EvaluationDashboard | StreamType::SystemMetrics) => {
                    match event {
                        Ok(_) => {
                            let result = match stream_type {
                                StreamType::EvaluationSummary => send_evaluation_summary(&mut session_clone, &evaluation_logic, &query).await,
                                StreamType::EvaluationRates => send_evaluation_rates(&mut session_clone, &evaluation_logic, &query).await,
                                StreamType::EvaluationsByFeature => send_evaluations_by_feature(&mut session_clone, &evaluation_logic, &query).await,
                                StreamType::EvaluationDashboard => send_evaluation_dashboard(&mut session_clone, &evaluation_logic, &query).await,
                                StreamType::SystemMetrics => {
                                    let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId").ok().flatten();
                                    send_system_metrics(&mut session_clone, &feature_logic, &client_logic, &evaluation_logic, team_id).await
                                }
                                _ => Ok(())
                            };
                            if result.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                event = approval_events_rx.recv(), if matches!(stream_type, StreamType::ApprovalRequests) => {
                    match event {
                        Ok(_) => {
                            if send_approval_requests(&mut session_clone, &approval_repo, &query).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = async {
                    if let Some(timer) = interval.as_mut() {
                        timer.tick().await;
                    }
                }, if interval.is_some() => {
                    let result = match stream_type {
                        StreamType::RecentActivities => send_recent_activities(
                            &mut session_clone,
                            &activity_repo,
                            &feature_repo,
                            &environment_logic,
                            &client_logic,
                            &pipeline_logic,
                            &query,
                        )
                        .await,
                        StreamType::FeatureGrowth => send_feature_growth(&mut session_clone, &feature_repo, &query).await,
                        StreamType::SystemMetrics => {
                            let team_id = parse_opt_uuid(query.team_id.as_ref(), "teamId").ok().flatten();
                            send_system_metrics(&mut session_clone, &feature_logic, &client_logic, &evaluation_logic, team_id).await
                        }
                        _ => Ok(())
                    };
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(response)
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(stream_ws);
}
