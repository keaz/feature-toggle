use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, post, put, web};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::activity_log::ActivityLogRepository;
use crate::database::feature::FeatureRepository;
use crate::database::metrics::{MetricType as DbMetricType, metric_repository_tx};
use crate::logic::canary::{CanaryDirection as LogicCanaryDirection, CanaryGateInput, CanaryLogic};
use crate::logic::client::ClientLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::feature_evaluation::FeatureEvaluationLogic;
use crate::logic::metrics::MetricLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::model::ID;
use crate::rest::error::RestError;
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};
use crate::rest::serde::{deserialize_optional_string_or_vec, deserialize_string_or_vec};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricsByFeatureQuery {
    pub feature_key: String,
    pub environment_id: String,
    pub time_period: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentResultsQuery {
    pub feature_key: String,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub metric_keys: Vec<String>,
    pub team_id: Option<String>,
    pub environment_id: Option<String>,
    pub time_period: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationSummaryQuery {
    pub period: String,
    pub feature_key: Option<String>,
    pub environment_id: Option<String>,
    pub client_id: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationRatesQuery {
    pub period: String,
    pub interval_minutes: i32,
    pub feature_key: Option<String>,
    pub environment_id: Option<String>,
    pub client_id: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationCountQuery {
    pub from_time: String,
    pub to_time: String,
    pub environment_id: Option<String>,
    pub client_id: Option<String>,
    pub feature_key: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationsByFeatureQuery {
    pub from_time: String,
    pub to_time: String,
    pub environment_id: Option<String>,
    pub client_id: Option<String>,
    pub team_id: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureGrowthQuery {
    pub from_time: String,
    pub to_time: String,
    pub interval: String,
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecentQuery {
    #[serde(default, deserialize_with = "deserialize_optional_string_or_vec")]
    pub activity_types: Option<Vec<String>>,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub actor_id: Option<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub team_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetricType {
    #[serde(alias = "conversion", alias = "Conversion")]
    Conversion,
    #[serde(alias = "numeric", alias = "Numeric")]
    Numeric,
    #[serde(alias = "duration", alias = "Duration")]
    Duration,
}

impl From<DbMetricType> for MetricType {
    fn from(value: DbMetricType) -> Self {
        match value {
            DbMetricType::Conversion => MetricType::Conversion,
            DbMetricType::Numeric => MetricType::Numeric,
            DbMetricType::Duration => MetricType::Duration,
        }
    }
}

impl From<MetricType> for DbMetricType {
    fn from(value: MetricType) -> Self {
        match value {
            MetricType::Conversion => DbMetricType::Conversion,
            MetricType::Numeric => DbMetricType::Numeric,
            MetricType::Duration => DbMetricType::Duration,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricResponse {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub metric_type: MetricType,
    pub unit: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MetricsResponse {
    pub items: Vec<MetricResponse>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateMetricRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(alias = "metric_type")]
    pub metric_type: MetricType,
    pub unit: Option<String>,
    #[serde(alias = "success_criteria")]
    pub success_criteria: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricResultResponse {
    pub metric_key: String,
    pub variant: Option<String>,
    pub sample_size: i32,
    pub conversion_rate: Option<f64>,
    pub lift: Option<f64>,
    pub confidence: Option<f64>,
    pub mean_value: Option<f64>,
    pub p95_value: Option<f64>,
    pub time_bucket: DateTime<Utc>,
    pub confidence_interval: Option<Vec<f64>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricAnalysisResponse {
    pub metric_key: String,
    pub results: Vec<MetricResultResponse>,
    pub winner: Option<String>,
    pub statistical_significance: Option<f64>,
    pub recommendation: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentAnalysisResponse {
    pub feature_key: String,
    pub metrics: Vec<MetricAnalysisResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationRateResponse {
    pub time_bucket: String,
    pub evaluation_count: i64,
    pub success_count: i64,
    pub prior_assignment_count: i64,
    pub success_rate: f64,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationSummaryResponse {
    pub total_evaluations: i64,
    pub successful_evaluations: i64,
    pub cached_evaluations: i64,
    pub unique_users: i64,
    pub top_feature_key: Option<String>,
    pub success_rate: f64,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationByFeatureResponse {
    pub feature_key: String,
    pub total_evaluations: i64,
    pub successful_evaluations: i64,
    pub cached_evaluations: i64,
    pub unique_users: i64,
    pub last_evaluated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationsByFeatureResponse {
    pub items: Vec<EvaluationByFeatureResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureGrowthResponse {
    pub time_bucket: DateTime<Utc>,
    pub team_id: Option<String>,
    pub team_name: Option<String>,
    pub feature_count: i64,
    pub cumulative_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntityDetailsResponse {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLogResponse {
    pub id: String,
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub entity_details: Option<ActivityEntityDetailsResponse>,
    pub actor_id: Option<String>,
    pub actor_name: Option<String>,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

const MIN_EXPERIMENT_VARIANT_SAMPLE: i64 = 100;
const MIN_EXPERIMENT_TOTAL_SAMPLE: i64 = 200;
const MIN_EXPERIMENT_CONFIDENCE: f64 = 0.95;
const SAMPLE_RATIO_MISMATCH_THRESHOLD: f64 = 0.15;

#[derive(Debug, Clone)]
struct ExperimentVariantAggregate {
    variant: Option<String>,
    metric_type: DbMetricType,
    sample_size: i64,
    conversion_count: i64,
    sum_value: f64,
    p95_value: Option<f64>,
    latest_bucket: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct ExperimentAnalysisDetails {
    winner: Option<String>,
    p_value: Option<f64>,
    recommendation: String,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLogPageResponse {
    pub items: Vec<ActivityLogResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetricsResponse {
    pub total_features: i64,
    pub active_clients: i64,
    pub total_clients: i64,
    pub evaluations_today: i64,
    pub evaluations_yesterday: i64,
    pub success_rate: f64,
    pub total_evaluations_7d: i64,
    pub successful_evaluations_7d: i64,
    pub generated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetricsResponse {
    pub processed: usize,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetricEventRequest {
    #[serde(alias = "metric_key")]
    pub metric_key: String,
    #[serde(alias = "feature_key")]
    pub feature_key: Option<String>,
    #[serde(alias = "environment_id")]
    pub environment_id: Option<String>,
    #[serde(alias = "user_context")]
    pub user_context: String,
    pub variant: Option<String>,
    pub value: f64,
    pub metadata: Option<serde_json::Value>,
    #[serde(alias = "timestamp_unix_ms")]
    pub timestamp_unix_ms: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetricsRequest {
    #[serde(alias = "client_id")]
    pub client_id: String,
    #[serde(alias = "client_secret")]
    pub client_secret: String,
    pub events: Vec<TrackMetricEventRequest>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetricsWithTokenRequest {
    pub events: Vec<TrackMetricEventRequest>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CanaryDirection {
    HigherIsBetter,
    LowerIsBetter,
}

impl From<CanaryDirection> for LogicCanaryDirection {
    fn from(value: CanaryDirection) -> Self {
        match value {
            CanaryDirection::HigherIsBetter => LogicCanaryDirection::HigherIsBetter,
            CanaryDirection::LowerIsBetter => LogicCanaryDirection::LowerIsBetter,
        }
    }
}

impl From<LogicCanaryDirection> for CanaryDirection {
    fn from(value: LogicCanaryDirection) -> Self {
        match value {
            LogicCanaryDirection::HigherIsBetter => CanaryDirection::HigherIsBetter,
            LogicCanaryDirection::LowerIsBetter => CanaryDirection::LowerIsBetter,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CanaryGateConfigRequest {
    pub metric_key: String,
    pub baseline_variant: String,
    pub canary_variant: String,
    pub direction: CanaryDirection,
    pub threshold_pct: f64,
    pub min_sample_size: i64,
    pub window_minutes: i32,
    pub auto_rollback_on_fail: bool,
    pub rollback_in_minutes: Option<i32>,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SetCanaryGatesRequest {
    pub gates: Vec<CanaryGateConfigRequest>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CanaryGateResponse {
    pub id: String,
    pub stage_id: String,
    pub feature_id: String,
    pub environment_id: String,
    pub metric_key: String,
    pub baseline_variant: String,
    pub canary_variant: String,
    pub direction: CanaryDirection,
    pub threshold_pct: f64,
    pub min_sample_size: i64,
    pub window_minutes: i32,
    pub auto_rollback_on_fail: bool,
    pub rollback_in_minutes: Option<i32>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeCanaryGateRequest {
    pub force_rollback: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CanaryVariantSnapshotResponse {
    pub variant: String,
    pub sample_size: i64,
    pub value: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CanaryAnalysisResponse {
    pub gate_id: String,
    pub feature_id: String,
    pub metric_key: String,
    pub passed: bool,
    pub reason: String,
    pub baseline: CanaryVariantSnapshotResponse,
    pub canary: CanaryVariantSnapshotResponse,
    pub regression_pct: Option<f64>,
    pub threshold_pct: f64,
    pub rollback_triggered: bool,
    pub rollback_error: Option<String>,
    pub evaluated_at: DateTime<Utc>,
}

fn map_canary_gate_response(gate: crate::logic::canary::CanaryGate) -> CanaryGateResponse {
    CanaryGateResponse {
        id: gate.id.to_string(),
        stage_id: gate.stage_id.to_string(),
        feature_id: gate.feature_id.to_string(),
        environment_id: gate.environment_id.to_string(),
        metric_key: gate.metric_key,
        baseline_variant: gate.baseline_variant,
        canary_variant: gate.canary_variant,
        direction: gate.direction.into(),
        threshold_pct: gate.threshold_pct,
        min_sample_size: gate.min_sample_size,
        window_minutes: gate.window_minutes,
        auto_rollback_on_fail: gate.auto_rollback_on_fail,
        rollback_in_minutes: gate.rollback_in_minutes,
        enabled: gate.enabled,
        created_at: gate.created_at,
        updated_at: gate.updated_at,
    }
}

fn map_canary_analysis_response(
    result: crate::logic::canary::CanaryAnalysisResult,
) -> CanaryAnalysisResponse {
    CanaryAnalysisResponse {
        gate_id: result.gate_id.to_string(),
        feature_id: result.feature_id.to_string(),
        metric_key: result.metric_key,
        passed: result.passed,
        reason: result.reason,
        baseline: CanaryVariantSnapshotResponse {
            variant: result.baseline.variant,
            sample_size: result.baseline.sample_size,
            value: result.baseline.value,
        },
        canary: CanaryVariantSnapshotResponse {
            variant: result.canary.variant,
            sample_size: result.canary.sample_size,
            value: result.canary.value,
        },
        regression_pct: result.regression_pct,
        threshold_pct: result.threshold_pct,
        rollback_triggered: result.rollback_triggered,
        rollback_error: result.rollback_error,
        evaluated_at: result.evaluated_at,
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn parse_opt_uuid(value: Option<&String>, field: &str) -> Result<Option<Uuid>, RestError> {
    value.as_ref().map(|raw| parse_uuid(raw, field)).transpose()
}

fn parse_datetime(value: &str, field: &str) -> Result<DateTime<Utc>, RestError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn round_pct(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn parse_time_period(value: &str) -> Result<crate::streaming::TimePeriod, RestError> {
    match value.to_uppercase().as_str() {
        "PERIOD_24H" | "H24" | "24H" => Ok(crate::streaming::TimePeriod::H24),
        "PERIOD_7D" | "D7" | "7D" => Ok(crate::streaming::TimePeriod::D7),
        "PERIOD_30D" | "D30" | "30D" => Ok(crate::streaming::TimePeriod::D30),
        _ => Err(RestError::invalid_input(
            "period must be PERIOD_24H, PERIOD_7D, or PERIOD_30D",
        )),
    }
}

fn normalize_metric_key(value: &str) -> String {
    value.trim().to_string()
}

fn normalize_metric_name(value: &str) -> String {
    value.trim().to_string()
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn resolve_time_range_with_period(
    period: crate::streaming::TimePeriod,
) -> (DateTime<Utc>, DateTime<Utc>) {
    crate::streaming::calculate_time_range(period, Utc::now())
}

fn parse_metric_keys_from_query(query: &str) -> Result<Vec<String>, RestError> {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(query)
        .map_err(|_| RestError::invalid_input("invalid query parameters"))?;
    let mut metric_keys = Vec::new();
    for (key, value) in pairs {
        if key == "metricKeys" || key == "metricKeys[]" || key == "metric_keys" {
            for entry in value.split(',') {
                let trimmed = entry.trim();
                if !trimmed.is_empty() {
                    metric_keys.push(trimmed.to_string());
                }
            }
        }
    }
    if metric_keys.is_empty() {
        return Err(RestError::invalid_input(
            "metricKeys must include at least one entry",
        ));
    }
    Ok(metric_keys)
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
        environment_logic: &dyn crate::logic::environment::EnvironmentLogic,
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
    environment_logic: &dyn crate::logic::environment::EnvironmentLogic,
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

fn map_metric_result(row: crate::database::metrics::MetricAggregationRow) -> MetricResultResponse {
    let sample_size = std::cmp::min(row.sample_size, i32::MAX as i64) as i32;
    let conversion_rate = row.conversion_rate;
    let mean_value = row.mean_value;
    let p95_value = row.p95_value;

    let conversion_count = row.conversion_count;
    let sum_value = row.sum_value;

    let conversion_rate = if conversion_rate.is_none() {
        if let (Some(conversion_count), sample_size) = (conversion_count, sample_size) {
            if sample_size > 0 {
                Some(conversion_count as f64 / sample_size as f64)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        conversion_rate
    };

    let mean_value = if mean_value.is_none() {
        if let (Some(sum_value), sample_size) = (sum_value, sample_size) {
            if sample_size > 0 {
                Some(sum_value / sample_size as f64)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        mean_value
    };

    MetricResultResponse {
        metric_key: row.metric_key,
        variant: row.variant,
        sample_size,
        conversion_rate,
        lift: None,
        confidence: None,
        mean_value,
        p95_value,
        time_bucket: row.time_bucket,
        confidence_interval: None,
    }
}

fn variant_display_name(variant: &Option<String>) -> String {
    variant.clone().unwrap_or_else(|| "control".to_string())
}

fn variant_score(metric_type: DbMetricType, result: &MetricResultResponse) -> Option<f64> {
    match metric_type {
        DbMetricType::Conversion => result.conversion_rate,
        DbMetricType::Numeric => result.mean_value,
        DbMetricType::Duration => result.mean_value.map(|value| -value),
    }
}

fn lift_against_baseline(
    metric_type: DbMetricType,
    baseline: &MetricResultResponse,
    candidate: &MetricResultResponse,
) -> Option<f64> {
    let baseline_value = match metric_type {
        DbMetricType::Conversion => baseline.conversion_rate,
        DbMetricType::Numeric | DbMetricType::Duration => baseline.mean_value,
    }?;
    let candidate_value = match metric_type {
        DbMetricType::Conversion => candidate.conversion_rate,
        DbMetricType::Numeric | DbMetricType::Duration => candidate.mean_value,
    }?;

    if baseline_value.abs() < f64::EPSILON {
        return None;
    }

    if metric_type == DbMetricType::Duration {
        Some((baseline_value - candidate_value) / baseline_value)
    } else {
        Some((candidate_value - baseline_value) / baseline_value)
    }
}

fn normal_cdf(value: f64) -> f64 {
    0.5 * (1.0 + erf_approx(value / std::f64::consts::SQRT_2))
}

fn erf_approx(value: f64) -> f64 {
    let sign = if value < 0.0 { -1.0 } else { 1.0 };
    let x = value.abs();
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-x * x).exp();
    sign * y
}

fn two_proportion_p_value(
    a_success: i64,
    a_total: i64,
    b_success: i64,
    b_total: i64,
) -> Option<f64> {
    if a_total <= 0 || b_total <= 0 {
        return None;
    }

    let p1 = a_success as f64 / a_total as f64;
    let p2 = b_success as f64 / b_total as f64;
    let pooled = (a_success + b_success) as f64 / (a_total + b_total) as f64;
    let se = (pooled * (1.0 - pooled) * (1.0 / a_total as f64 + 1.0 / b_total as f64)).sqrt();
    if se <= f64::EPSILON {
        return None;
    }

    let z = (p1 - p2) / se;
    let p = 2.0 * (1.0 - normal_cdf(z.abs()));
    Some(p.clamp(0.0, 1.0))
}

fn wilson_interval(success: i64, total: i64) -> Option<Vec<f64>> {
    if total <= 0 {
        return None;
    }

    let z = 1.96;
    let n = total as f64;
    let p = success as f64 / n;
    let denom = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denom;
    let margin = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / denom;
    Some(vec![(center - margin).max(0.0), (center + margin).min(1.0)])
}

fn analyze_experiment_results(
    metric_type: DbMetricType,
    results: &mut [MetricResultResponse],
) -> ExperimentAnalysisDetails {
    let mut warnings = Vec::new();
    if results.is_empty() {
        warnings
            .push("Missing metric data for this feature, metric, or analysis window.".to_string());
        return ExperimentAnalysisDetails {
            winner: None,
            p_value: None,
            recommendation: "No winner: metric data is missing for the selected window."
                .to_string(),
            warnings,
        };
    }

    results.sort_by(|a, b| variant_display_name(&a.variant).cmp(&variant_display_name(&b.variant)));
    let total_samples: i64 = results.iter().map(|result| result.sample_size as i64).sum();
    let low_sample = total_samples < MIN_EXPERIMENT_TOTAL_SAMPLE
        || results
            .iter()
            .any(|result| (result.sample_size as i64) < MIN_EXPERIMENT_VARIANT_SAMPLE);
    if low_sample {
        warnings.push(format!(
            "Low sample size: need at least {MIN_EXPERIMENT_VARIANT_SAMPLE} samples per variant and {MIN_EXPERIMENT_TOTAL_SAMPLE} total before recommending a winner."
        ));
    }

    if results.len() < 2 {
        warnings.push(
            "Need at least two variants before experiment comparison is meaningful.".to_string(),
        );
    } else if total_samples >= MIN_EXPERIMENT_TOTAL_SAMPLE {
        let expected = total_samples as f64 / results.len() as f64;
        let mismatch = results.iter().any(|result| {
            ((result.sample_size as f64 - expected).abs() / expected)
                > SAMPLE_RATIO_MISMATCH_THRESHOLD
        });
        if mismatch {
            warnings.push(
                "Sample-ratio mismatch: observed samples differ materially from an even split."
                    .to_string(),
            );
        }
    }

    let baseline_index = results
        .iter()
        .position(|result| variant_display_name(&result.variant).eq_ignore_ascii_case("control"))
        .unwrap_or(0);
    let baseline = results[baseline_index].clone();
    let baseline_successes = baseline
        .conversion_rate
        .map(|rate| (rate * baseline.sample_size as f64).round() as i64);

    let mut best_index: Option<usize> = None;
    let mut best_score = f64::MIN;
    for (index, result) in results.iter().enumerate() {
        if let Some(score) = variant_score(metric_type, result)
            && score > best_score
        {
            best_score = score;
            best_index = Some(index);
        }
    }

    let mut best_p_value = None;
    for result in results.iter_mut() {
        result.lift = lift_against_baseline(metric_type, &baseline, result);
        if metric_type == DbMetricType::Conversion {
            if let Some(rate) = result.conversion_rate {
                let successes = (rate * result.sample_size as f64).round() as i64;
                result.confidence_interval = wilson_interval(successes, result.sample_size as i64);
                if let Some(baseline_successes) = baseline_successes {
                    let p_value = two_proportion_p_value(
                        successes,
                        result.sample_size as i64,
                        baseline_successes,
                        baseline.sample_size as i64,
                    );
                    result.confidence = p_value.map(|p| 1.0 - p);
                }
            }
        }
    }

    if metric_type != DbMetricType::Conversion {
        warnings.push(
            "Confidence unavailable: numeric and duration metrics need variance data before statistical winner recommendation."
                .to_string(),
        );
    }

    let candidate = best_index.map(|index| results[index].clone());
    if let Some(candidate) = candidate.as_ref()
        && metric_type == DbMetricType::Conversion
        && candidate.variant != baseline.variant
        && let (Some(candidate_successes), Some(baseline_successes)) = (
            candidate
                .conversion_rate
                .map(|rate| (rate * candidate.sample_size as f64).round() as i64),
            baseline_successes,
        )
    {
        best_p_value = two_proportion_p_value(
            candidate_successes,
            candidate.sample_size as i64,
            baseline_successes,
            baseline.sample_size as i64,
        );
    }

    let confidence = best_p_value.map(|p| 1.0 - p);
    let confidence_pass = confidence
        .map(|value| value >= MIN_EXPERIMENT_CONFIDENCE)
        .unwrap_or(false);
    if metric_type == DbMetricType::Conversion && !confidence_pass && results.len() >= 2 {
        warnings.push(format!(
            "Confidence below {:.0}%: treat current leader as directional only.",
            MIN_EXPERIMENT_CONFIDENCE * 100.0
        ));
    }

    let winner = if !low_sample && confidence_pass {
        candidate.as_ref().and_then(|result| result.variant.clone())
    } else {
        None
    };

    let recommendation = if let Some(winner) = winner.as_ref() {
        format!(
            "Recommend {winner}: minimum sample size passed and confidence is {:.1}%.",
            confidence.unwrap_or_default() * 100.0
        )
    } else if let Some(candidate) = candidate {
        format!(
            "No winner yet: {} is leading, but guardrails have not passed.",
            variant_display_name(&candidate.variant)
        )
    } else {
        "No winner: no comparable variant results found.".to_string()
    };

    ExperimentAnalysisDetails {
        winner,
        p_value: best_p_value,
        recommendation,
        warnings,
    }
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

fn validate_interval_minutes(interval_minutes: i32) -> Result<(), RestError> {
    if !(1..=60).contains(&interval_minutes) {
        return Err(RestError::invalid_input(
            "intervalMinutes must be between 1 and 60",
        ));
    }
    Ok(())
}

fn validate_time_range(from: DateTime<Utc>, to: DateTime<Utc>) -> Result<(), RestError> {
    if to < from {
        return Err(RestError::invalid_input("toTime must be >= fromTime"));
    }
    Ok(())
}

fn validate_metric_keys(metric_keys: &[String]) -> Result<(), RestError> {
    if metric_keys.is_empty() {
        return Err(RestError::invalid_input(
            "metricKeys must include at least one entry",
        ));
    }
    Ok(())
}

fn validate_feature_growth_interval(interval: &str) -> Result<(), RestError> {
    match interval {
        "day" | "week" | "month" => Ok(()),
        _ => Err(RestError::invalid_input(
            "interval must be 'day', 'week', or 'month'",
        )),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/metrics",
    request_body = CreateMetricRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 201, description = "Metric created", body = MetricResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[post("/teams/{team_id}/metrics")]
pub(crate) async fn create_metric(
    db_pool: web::Data<sqlx::PgPool>,
    team_id: web::Path<String>,
    payload: web::Json<CreateMetricRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let repo = metric_repository_tx(db_pool.get_ref().clone());

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let created = crate::logic::metrics_tx::create_metric_in_tx(
        &mut tx,
        &repo,
        team_uuid,
        normalize_metric_key(&payload.key),
        normalize_metric_name(&payload.name),
        payload.description.clone(),
        DbMetricType::from(payload.metric_type),
        normalize_optional_string(payload.unit.clone()),
        payload.success_criteria.clone(),
    )
    .await;

    match created {
        Ok(metric) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;

            Ok(HttpResponse::Created().json(MetricResponse {
                id: metric.id.to_string(),
                key: metric.key,
                name: metric.name,
                description: metric.description,
                metric_type: MetricType::from(metric.metric_type),
                unit: metric.unit,
            }))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/metrics",
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Metrics list", body = MetricsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/teams/{team_id}/metrics")]
pub(crate) async fn list_metrics(
    logic: web::Data<Box<dyn MetricLogic>>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let metrics = logic
        .list_metrics(team_uuid)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(MetricsResponse {
        items: metrics
            .into_iter()
            .map(|metric| MetricResponse {
                id: metric.id.to_string(),
                key: metric.key,
                name: metric.name,
                description: metric.description,
                metric_type: MetricType::from(metric.metric_type),
                unit: metric.unit,
            })
            .collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/by-feature",
    params(
        ("featureKey" = String, Query, description = "Feature key"),
        ("environmentId" = String, Query, description = "Environment ID"),
        ("timePeriod" = String, Query, description = "Time period")
    ),
    responses(
        (status = 200, description = "Metric results", body = [MetricResultResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/by-feature")]
pub(crate) async fn metrics_by_feature(
    logic: web::Data<Box<dyn MetricLogic>>,
    query: web::Query<MetricsByFeatureQuery>,
) -> Result<impl Responder, RestError> {
    let env_uuid = parse_uuid(&query.environment_id, "environment_id")?;
    let period = parse_time_period(&query.time_period)?;
    let (from, to) = resolve_time_range_with_period(period);

    let rows = logic
        .get_metric_results(&query.feature_key, None, Some(env_uuid), from, to)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(rows.into_iter().map(map_metric_result).collect::<Vec<_>>()))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/experiment-results",
    params(
        ("featureKey" = String, Query, description = "Feature key"),
        ("metricKeys" = [String], Query, description = "Metric keys"),
        ("teamId" = Option<String>, Query, description = "Team ID"),
        ("environmentId" = Option<String>, Query, description = "Environment ID"),
        ("timePeriod" = Option<String>, Query, description = "Time period")
    ),
    responses(
        (status = 200, description = "Experiment analysis", body = ExperimentAnalysisResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/experiment-results")]
pub(crate) async fn experiment_results(
    logic: web::Data<Box<dyn MetricLogic>>,
    req: HttpRequest,
    query: web::Query<ExperimentResultsQuery>,
) -> Result<impl Responder, RestError> {
    let metric_keys = if !query.metric_keys.is_empty() {
        query.metric_keys.clone()
    } else {
        parse_metric_keys_from_query(req.query_string())?
    };
    validate_metric_keys(&metric_keys)?;

    let team_uuid = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;
    let env_uuid = parse_opt_uuid(query.environment_id.as_ref(), "environment_id")?;

    let period = match query.time_period.as_ref() {
        Some(period) => parse_time_period(period)?,
        None => crate::streaming::TimePeriod::D7,
    };

    let (from, to) = resolve_time_range_with_period(period);

    let rows = logic
        .get_metric_results(&query.feature_key, team_uuid, env_uuid, from, to)
        .await
        .map_err(RestError::from)?;

    let requested: std::collections::HashSet<String> = metric_keys.iter().cloned().collect();

    let mut aggregated: std::collections::HashMap<
        String,
        std::collections::HashMap<Option<String>, ExperimentVariantAggregate>,
    > = std::collections::HashMap::new();

    for row in rows
        .into_iter()
        .filter(|r| requested.contains(&r.metric_key))
    {
        let metric_entry = aggregated.entry(row.metric_key.clone()).or_default();
        let entry =
            metric_entry
                .entry(row.variant.clone())
                .or_insert_with(|| ExperimentVariantAggregate {
                    variant: row.variant.clone(),
                    metric_type: row.metric_type,
                    sample_size: 0,
                    conversion_count: 0,
                    sum_value: 0.0,
                    p95_value: None,
                    latest_bucket: row.time_bucket,
                });

        entry.sample_size += row.sample_size;
        entry.conversion_count += row.conversion_count.unwrap_or(0);
        entry.sum_value += row.sum_value.unwrap_or(0.0);
        if row.p95_value.is_some() {
            entry.p95_value = row.p95_value;
        }
        if row.time_bucket > entry.latest_bucket {
            entry.latest_bucket = row.time_bucket;
        }
    }

    let mut analyses = Vec::new();
    for key in &metric_keys {
        let mut results = Vec::new();
        let mut metric_type = DbMetricType::Conversion;
        if let Some(variants) = aggregated.get(key) {
            for aggregate in variants.values() {
                metric_type = aggregate.metric_type;
                let sample_size_i32 = std::cmp::min(aggregate.sample_size, i32::MAX as i64) as i32;
                let conversion_rate = if aggregate.sample_size > 0 {
                    Some(aggregate.conversion_count as f64 / aggregate.sample_size as f64)
                } else {
                    None
                };
                let mean_value = if aggregate.sample_size > 0 {
                    Some(aggregate.sum_value / aggregate.sample_size as f64)
                } else {
                    None
                };

                results.push(MetricResultResponse {
                    metric_key: key.clone(),
                    variant: aggregate.variant.clone(),
                    sample_size: sample_size_i32,
                    conversion_rate,
                    lift: None,
                    confidence: None,
                    mean_value,
                    p95_value: aggregate.p95_value,
                    time_bucket: aggregate.latest_bucket,
                    confidence_interval: None,
                });
            }
        }

        let details = analyze_experiment_results(metric_type, &mut results);

        analyses.push(MetricAnalysisResponse {
            metric_key: key.clone(),
            results,
            winner: details.winner,
            statistical_significance: details.p_value,
            recommendation: details.recommendation,
            warnings: details.warnings,
        });
    }

    Ok(HttpResponse::Ok().json(ExperimentAnalysisResponse {
        feature_key: query.feature_key.clone(),
        metrics: analyses,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/evaluations/summary",
    params(
        ("period" = String, Query, description = "PERIOD_24H, PERIOD_7D, PERIOD_30D"),
        ("featureKey" = Option<String>, Query, description = "Feature key"),
        ("environmentId" = Option<String>, Query, description = "Environment ID"),
        ("clientId" = Option<String>, Query, description = "Client ID"),
        ("teamId" = Option<String>, Query, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Evaluation summary", body = EvaluationSummaryResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/evaluations/summary")]
pub(crate) async fn evaluation_summary(
    logic: web::Data<Box<dyn FeatureEvaluationLogic>>,
    query: web::Query<EvaluationSummaryQuery>,
) -> Result<impl Responder, RestError> {
    let period = parse_time_period(&query.period)?;
    let (from, to) = resolve_time_range_with_period(period);
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "client_id")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;

    let summary = logic
        .get_evaluation_summary(
            query.feature_key.clone(),
            query.environment_id.clone(),
            client_id,
            team_id,
            from,
            to,
        )
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(map_evaluation_summary(summary)))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/evaluations/rates",
    params(
        ("period" = String, Query, description = "PERIOD_24H, PERIOD_7D, PERIOD_30D"),
        ("intervalMinutes" = i32, Query, description = "Interval minutes"),
        ("featureKey" = Option<String>, Query, description = "Feature key"),
        ("environmentId" = Option<String>, Query, description = "Environment ID"),
        ("clientId" = Option<String>, Query, description = "Client ID"),
        ("teamId" = Option<String>, Query, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Evaluation rates", body = [EvaluationRateResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/evaluations/rates")]
pub(crate) async fn evaluation_rates(
    logic: web::Data<Box<dyn FeatureEvaluationLogic>>,
    query: web::Query<EvaluationRatesQuery>,
) -> Result<impl Responder, RestError> {
    validate_interval_minutes(query.interval_minutes)?;
    let period = parse_time_period(&query.period)?;
    let (from, to) = resolve_time_range_with_period(period);
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "client_id")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;

    let rates = logic
        .get_evaluation_rates(
            query.feature_key.clone(),
            query.environment_id.clone(),
            client_id,
            team_id,
            from,
            to,
            query.interval_minutes,
        )
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(
        rates
            .into_iter()
            .map(map_evaluation_rate)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/evaluations/by-feature",
    params(
        ("fromTime" = String, Query, description = "Start time"),
        ("toTime" = String, Query, description = "End time"),
        ("environmentId" = Option<String>, Query, description = "Environment ID"),
        ("clientId" = Option<String>, Query, description = "Client ID"),
        ("teamId" = Option<String>, Query, description = "Team ID"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Evaluations by feature", body = EvaluationsByFeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/evaluations/by-feature")]
pub(crate) async fn evaluations_by_feature(
    logic: web::Data<Box<dyn FeatureEvaluationLogic>>,
    query: web::Query<EvaluationsByFeatureQuery>,
) -> Result<impl Responder, RestError> {
    let from_time = parse_datetime(&query.from_time, "fromTime")?;
    let to_time = parse_datetime(&query.to_time, "toTime")?;
    validate_time_range(from_time, to_time)?;
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "client_id")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;

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
        .map_err(RestError::from)?;

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

    Ok(HttpResponse::Ok().json(EvaluationsByFeatureResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/evaluations/count",
    params(
        ("fromTime" = String, Query, description = "Start time"),
        ("toTime" = String, Query, description = "End time"),
        ("environmentId" = Option<String>, Query, description = "Environment ID"),
        ("clientId" = Option<String>, Query, description = "Client ID"),
        ("featureKey" = Option<String>, Query, description = "Feature key"),
        ("teamId" = Option<String>, Query, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Evaluation count", body = i64),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/evaluations/count")]
pub(crate) async fn evaluation_count(
    logic: web::Data<Box<dyn FeatureEvaluationLogic>>,
    query: web::Query<EvaluationCountQuery>,
) -> Result<impl Responder, RestError> {
    let from_time = parse_datetime(&query.from_time, "fromTime")?;
    let to_time = parse_datetime(&query.to_time, "toTime")?;
    validate_time_range(from_time, to_time)?;
    let client_id = parse_opt_uuid(query.client_id.as_ref(), "client_id")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;

    let count = logic
        .count_evaluations(
            from_time,
            to_time,
            query.environment_id.clone(),
            client_id,
            query.feature_key.clone(),
            team_id,
        )
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(count))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/feature-growth",
    params(
        ("fromTime" = String, Query, description = "Start time"),
        ("toTime" = String, Query, description = "End time"),
        ("interval" = String, Query, description = "day|week|month"),
        ("teamId" = Option<String>, Query, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Feature growth", body = [FeatureGrowthResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/feature-growth")]
pub(crate) async fn feature_growth(
    repo: web::Data<Box<dyn FeatureRepository>>,
    query: web::Query<FeatureGrowthQuery>,
) -> Result<impl Responder, RestError> {
    let from_time = parse_datetime(&query.from_time, "fromTime")?;
    let to_time = parse_datetime(&query.to_time, "toTime")?;
    validate_time_range(from_time, to_time)?;
    validate_feature_growth_interval(&query.interval)?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;

    let results = repo
        .get_feature_growth(from_time, to_time, query.interval.clone(), team_id)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(
        results
            .into_iter()
            .map(|row| FeatureGrowthResponse {
                time_bucket: row.time_bucket,
                team_id: row.team_id.map(|id| id.to_string()),
                team_name: row.team_name,
                feature_count: row.feature_count,
                cumulative_count: row.cumulative_count,
            })
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/activity/recent",
    params(
        ("activityTypes" = Option<[String]>, Query, description = "Activity types"),
        ("entityType" = Option<String>, Query, description = "Entity type"),
        ("entityId" = Option<String>, Query, description = "Entity ID"),
        ("actorId" = Option<String>, Query, description = "Actor ID"),
        ("fromDate" = Option<String>, Query, description = "From date"),
        ("toDate" = Option<String>, Query, description = "To date"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit"),
        ("teamId" = Option<String>, Query, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Activity logs", body = ActivityLogPageResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Activity"
)]
#[get("/activity/recent")]
pub(crate) async fn recent_activity(
    repo: web::Data<Box<dyn ActivityLogRepository>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    environment_logic: web::Data<Box<dyn EnvironmentLogic>>,
    client_logic: web::Data<Box<dyn ClientLogic>>,
    pipeline_logic: web::Data<Box<dyn PipelineLogic>>,
    query: web::Query<ActivityRecentQuery>,
) -> Result<impl Responder, RestError> {
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let actor_id = parse_opt_uuid(query.actor_id.as_ref(), "actor_id")?;
    let team_id = parse_opt_uuid(query.team_id.as_ref(), "team_id")?;
    let from_date = query
        .from_date
        .as_ref()
        .map(|value| parse_datetime(value, "fromDate"))
        .transpose()?;
    let to_date = query
        .to_date
        .as_ref()
        .map(|value| parse_datetime(value, "toDate"))
        .transpose()?;

    let filter = crate::database::activity_log::ActivityLogFilter {
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
        .map_err(|_| RestError::internal("Failed to load recent activity"))?;

    let mut items = Vec::new();
    let mut filtered_count: i64 = 0;
    let feature_repo_arc = std::sync::Arc::new(feature_repo.clone());
    let feature_repo_ref = feature_repo.as_ref().as_ref();
    let environment_logic_ref = environment_logic.as_ref().as_ref();
    let client_logic_ref = client_logic.as_ref().as_ref();
    let pipeline_logic_ref = pipeline_logic.as_ref().as_ref();
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

    Ok(HttpResponse::Ok().json(ActivityLogPageResponse {
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
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/metrics/evaluations/system",
    params(("teamId" = Option<String>, Query, description = "Team ID")),
    responses(
        (status = 200, description = "System metrics", body = SystemMetricsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/metrics/evaluations/system")]
pub(crate) async fn system_metrics(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    client_logic: web::Data<Box<dyn ClientLogic>>,
    evaluation_logic: web::Data<Box<dyn FeatureEvaluationLogic>>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> Result<impl Responder, RestError> {
    let team_id = query
        .get("teamId")
        .map(|value| parse_uuid(value, "team_id"))
        .transpose()?;

    let now = Utc::now();
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let today_end = now;
    let yesterday_start = (now - chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let yesterday_end = today_start;
    let (from_7d, to_7d) = resolve_time_range_with_period(crate::streaming::TimePeriod::D7);

    let team_id_arg = team_id.map(ID::from);

    let (
        total_features,
        active_clients,
        total_clients,
        evaluations_today,
        evaluations_yesterday,
        summary_7d,
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
        evaluation_logic.get_evaluation_summary(None, None, None, team_id, from_7d, to_7d),
    );

    let summary = summary_7d.map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(SystemMetricsResponse {
        total_features: total_features.map_err(RestError::from)?,
        active_clients: active_clients.map_err(RestError::from)?,
        total_clients: total_clients.map_err(RestError::from)?,
        evaluations_today: evaluations_today.map_err(RestError::from)?,
        evaluations_yesterday: evaluations_yesterday.map_err(RestError::from)?,
        success_rate: round_pct(summary.success_rate),
        total_evaluations_7d: summary.total_evaluations,
        successful_evaluations_7d: summary.successful_evaluations,
        generated_at: now.to_rfc3339(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/metrics/track",
    request_body = TrackMetricsRequest,
    responses(
        (status = 200, description = "Metrics tracked", body = TrackMetricsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    security(()),
    tag = "Metrics"
)]
#[post("/metrics/track")]
pub(crate) async fn track_metrics(
    metric_logic: web::Data<Box<dyn MetricLogic>>,
    payload: web::Json<TrackMetricsRequest>,
) -> Result<HttpResponse, RestError> {
    track_metrics_handler(metric_logic, payload).await
}

fn map_track_metric_events(
    event_requests: Vec<TrackMetricEventRequest>,
) -> Result<Vec<crate::logic::metrics::TrackMetricInput>, RestError> {
    let mut events = Vec::with_capacity(event_requests.len());
    for ev in event_requests {
        let environment_id = match ev.environment_id {
            Some(ref env) if !env.is_empty() => Some(
                Uuid::parse_str(env)
                    .map_err(|_| RestError::invalid_input("invalid environment_id"))?,
            ),
            _ => None,
        };

        let timestamp = match ev.timestamp_unix_ms {
            Some(ts) if ts > 0 => Some(
                DateTime::<Utc>::from_timestamp_millis(ts)
                    .ok_or_else(|| RestError::invalid_input("invalid timestamp_unix_ms"))?,
            ),
            _ => None,
        };

        events.push(crate::logic::metrics::TrackMetricInput {
            metric_key: ev.metric_key,
            feature_key: ev.feature_key,
            environment_id,
            user_context: ev.user_context,
            variant: ev.variant,
            value: ev.value,
            metadata: ev.metadata,
            timestamp,
        });
    }
    Ok(events)
}

pub(crate) async fn track_metrics_handler(
    metric_logic: web::Data<Box<dyn MetricLogic>>,
    payload: web::Json<TrackMetricsRequest>,
) -> Result<HttpResponse, RestError> {
    let body = payload.into_inner();
    let events = map_track_metric_events(body.events)?;

    let processed = metric_logic
        .track_metrics(&body.client_id, &body.client_secret, events)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(TrackMetricsResponse { processed }))
}

#[utoipa::path(
    post,
    path = "/api/v1/metrics/track/system",
    request_body = TrackMetricsWithTokenRequest,
    responses(
        (status = 200, description = "Metrics tracked", body = TrackMetricsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[post("/metrics/track/system")]
pub(crate) async fn track_metrics_with_system_token(
    metric_logic: web::Data<Box<dyn MetricLogic>>,
    req: HttpRequest,
    payload: web::Json<TrackMetricsWithTokenRequest>,
) -> Result<HttpResponse, RestError> {
    let jwt = req
        .extensions()
        .get::<crate::JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    let team_id = jwt
        .team_id
        .ok_or_else(|| RestError::forbidden("system client team scope is required"))?;
    let events = map_track_metric_events(payload.into_inner().events)?;
    let processed = metric_logic
        .track_metrics_for_team(team_id, events)
        .await
        .map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(TrackMetricsResponse { processed }))
}

#[utoipa::path(
    get,
    path = "/api/v1/stages/{stage_id}/canary-gates",
    params(
        ("stage_id" = String, Path, description = "Stage ID")
    ),
    responses(
        (status = 200, description = "Canary gates for stage", body = [CanaryGateResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[get("/stages/{stage_id}/canary-gates")]
pub(crate) async fn list_canary_gates(
    canary_logic: web::Data<Box<dyn CanaryLogic>>,
    stage_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let stage_uuid = parse_uuid(&stage_id, "stage_id")?;
    let gates = canary_logic
        .list_stage_gates(stage_uuid)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(
        gates
            .into_iter()
            .map(map_canary_gate_response)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/stages/{stage_id}/canary-gates",
    request_body = SetCanaryGatesRequest,
    params(
        ("stage_id" = String, Path, description = "Stage ID")
    ),
    responses(
        (status = 200, description = "Canary gates replaced for stage", body = [CanaryGateResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[put("/stages/{stage_id}/canary-gates")]
pub(crate) async fn replace_canary_gates(
    canary_logic: web::Data<Box<dyn CanaryLogic>>,
    stage_id: web::Path<String>,
    body: web::Json<SetCanaryGatesRequest>,
) -> Result<impl Responder, RestError> {
    let stage_uuid = parse_uuid(&stage_id, "stage_id")?;
    let gates = body
        .into_inner()
        .gates
        .into_iter()
        .map(|gate| CanaryGateInput {
            metric_key: gate.metric_key,
            baseline_variant: gate.baseline_variant,
            canary_variant: gate.canary_variant,
            direction: gate.direction.into(),
            threshold_pct: gate.threshold_pct,
            min_sample_size: gate.min_sample_size,
            window_minutes: gate.window_minutes,
            auto_rollback_on_fail: gate.auto_rollback_on_fail,
            rollback_in_minutes: gate.rollback_in_minutes,
            enabled: gate.enabled,
        })
        .collect::<Vec<_>>();

    let updated = canary_logic
        .replace_stage_gates(stage_uuid, gates)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(
        updated
            .into_iter()
            .map(map_canary_gate_response)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/canary-gates/{gate_id}/analyze",
    request_body = AnalyzeCanaryGateRequest,
    params(
        ("gate_id" = String, Path, description = "Canary gate ID")
    ),
    responses(
        (status = 200, description = "Canary analysis result", body = CanaryAnalysisResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Metrics"
)]
#[post("/canary-gates/{gate_id}/analyze")]
pub(crate) async fn analyze_canary_gate(
    canary_logic: web::Data<Box<dyn CanaryLogic>>,
    gate_id: web::Path<String>,
    body: web::Json<AnalyzeCanaryGateRequest>,
) -> Result<impl Responder, RestError> {
    let gate_uuid = parse_uuid(&gate_id, "gate_id")?;
    let analysis = canary_logic
        .analyze_gate(gate_uuid, body.force_rollback)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(map_canary_analysis_response(analysis)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_metrics)
        .service(create_metric)
        .service(metrics_by_feature)
        .service(experiment_results)
        .service(evaluation_summary)
        .service(evaluation_rates)
        .service(evaluations_by_feature)
        .service(evaluation_count)
        .service(feature_growth)
        .service(list_canary_gates)
        .service(replace_canary_gates)
        .service(analyze_canary_gate)
        .service(recent_activity)
        .service(system_metrics)
        .service(track_metrics)
        .service(track_metrics_with_system_token);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::{
        ActivityLogRepository, CreateActivityLog, PgActivityLogRepository,
    };
    use crate::database::client::client_repository;
    use crate::database::feature::{FeatureRepository, MockFeatureRepository};
    use crate::database::metrics::{
        CreateMetric, MetricAggregationRow, MetricRow, MetricType as DbMetricType,
        metric_repository,
    };
    use crate::logic::client::{ClientLogic, MockClientLogic};
    use crate::logic::environment::{EnvironmentLogic, MockEnvironmentLogic};
    use crate::logic::metrics::{MetricLogic, MetricLogicError, TrackMetricInput, metric_logic};
    use crate::logic::pipeline::{MockPipelineLogic, PipelineLogic};
    use actix_web::{App, guard, http::StatusCode, test};
    use sqlx::postgres::PgPoolOptions;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct RecordedTrackCall {
        client_id: String,
        client_secret: String,
        events: Vec<TrackMetricInput>,
    }

    #[derive(Clone)]
    struct RecordingMetricLogic {
        processed: usize,
        calls: Arc<Mutex<Vec<RecordedTrackCall>>>,
    }

    impl RecordingMetricLogic {
        fn new(processed: usize) -> Self {
            Self {
                processed,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn recorded_calls(&self) -> Vec<RecordedTrackCall> {
            self.calls
                .lock()
                .expect("track call mutex poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl MetricLogic for RecordingMetricLogic {
        async fn create_metric(
            &self,
            _team_id: Uuid,
            _key: String,
            _name: String,
            _description: Option<String>,
            _metric_type: crate::database::metrics::MetricType,
            _unit: Option<String>,
            _success_criteria: Option<serde_json::Value>,
        ) -> Result<MetricRow, MetricLogicError> {
            unreachable!("create_metric is not used in track_metrics route tests")
        }

        async fn track_metrics(
            &self,
            client_id: &str,
            client_secret: &str,
            events: Vec<TrackMetricInput>,
        ) -> Result<usize, MetricLogicError> {
            self.calls
                .lock()
                .expect("track call mutex poisoned")
                .push(RecordedTrackCall {
                    client_id: client_id.to_string(),
                    client_secret: client_secret.to_string(),
                    events,
                });
            Ok(self.processed)
        }

        async fn track_metrics_for_team(
            &self,
            _team_id: Uuid,
            events: Vec<TrackMetricInput>,
        ) -> Result<usize, MetricLogicError> {
            self.calls
                .lock()
                .expect("track call mutex poisoned")
                .push(RecordedTrackCall {
                    client_id: "system-token".to_string(),
                    client_secret: String::new(),
                    events,
                });
            Ok(self.processed)
        }

        async fn aggregate_metrics(
            &self,
            _from: DateTime<Utc>,
            _to: DateTime<Utc>,
            _bucket: &str,
        ) -> Result<u64, MetricLogicError> {
            unreachable!("aggregate_metrics is not used in track_metrics route tests")
        }

        async fn get_metric_results(
            &self,
            _feature_key: &str,
            _team_id: Option<Uuid>,
            _environment_id: Option<Uuid>,
            _from: DateTime<Utc>,
            _to: DateTime<Utc>,
        ) -> Result<Vec<MetricAggregationRow>, MetricLogicError> {
            unreachable!("get_metric_results is not used in track_metrics route tests")
        }

        async fn list_metrics(&self, _team_id: Uuid) -> Result<Vec<MetricRow>, MetricLogicError> {
            unreachable!("list_metrics is not used in track_metrics route tests")
        }

        fn clone_box(&self) -> Box<dyn MetricLogic> {
            Box::new(self.clone())
        }
    }

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    async fn insert_team(pool: &sqlx::PgPool) -> Uuid {
        let team_id = Uuid::new_v4();
        let name = format!("metrics-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "metrics test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_metric(pool: &sqlx::PgPool, team_id: Uuid, key: &str) {
        let repo = metric_repository(pool.clone());
        repo.create_metric(CreateMetric {
            team_id,
            key: key.to_string(),
            name: format!("metric-{}", key),
            description: None,
            metric_type: DbMetricType::Conversion,
            unit: None,
            success_criteria: None,
        })
        .await
        .expect("Failed to insert metric");
    }

    #[actix_web::test]
    async fn create_metric_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/metrics");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateMetricRequest {
                key: "signup_conversion".to_string(),
                name: "Signup Conversion".to_string(),
                description: Some("signup conversion metric".to_string()),
                metric_type: MetricType::Conversion,
                unit: Some("%".to_string()),
                success_criteria: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["key"], "signup_conversion");
        assert_eq!(json["metricType"], "CONVERSION");
    }

    #[actix_web::test]
    async fn create_metric_duplicate_returns_conflict() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        insert_metric(&pool, team_id, "dup_metric").await;

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/metrics");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateMetricRequest {
                key: "dup_metric".to_string(),
                name: "Duplicate Metric".to_string(),
                description: None,
                metric_type: MetricType::Conversion,
                unit: None,
                success_criteria: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
    }

    #[actix_web::test]
    async fn list_metrics_returns_items() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        insert_metric(&pool, team_id, "metric_a").await;

        let metric_logic = metric_logic(
            metric_repository(pool.clone()),
            client_repository(pool.clone()),
        );

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(metric_logic))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/metrics");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["key"], "metric_a");
    }

    #[actix_web::test]
    async fn recent_activity_returns_items() {
        let pool = test_pool().await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());
        let entity_id = Uuid::new_v4();
        let created = activity_repo
            .create_activity(CreateActivityLog {
                activity_type: "team_created".to_string(),
                entity_type: "team".to_string(),
                entity_id: entity_id.to_string(),
                actor_id: None,
                actor_name: Some("system".to_string()),
                description: "Team created".to_string(),
                metadata: Some(serde_json::json!({ "name": "Team Alpha" })),
            })
            .await
            .expect("Failed to create activity");

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(MockFeatureRepository::new()) as Box<dyn FeatureRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(MockEnvironmentLogic::new()) as Box<dyn EnvironmentLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(MockClientLogic::new()) as Box<dyn ClientLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(MockPipelineLogic::new()) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!(
                "/api/v1/activity/recent?offset=0&limit=5&entityType=team&entityId={entity_id}"
            ))
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], created.id.to_string());
        assert_eq!(json["items"][0]["activityType"], "team_created");
    }

    #[actix_web::test]
    async fn track_metrics_endpoints_share_invalid_input_mapping() {
        let metric_logic: Box<dyn MetricLogic> = Box::new(RecordingMetricLogic::new(1));
        let payload = serde_json::json!({
            "clientId": Uuid::new_v4().to_string(),
            "clientSecret": "secret",
            "events": [{
                "metricKey": "signup_conversion",
                "userContext": "user-123",
                "value": 1.0,
                "environmentId": "not-a-uuid"
            }]
        });

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(metric_logic))
                .service(
                    web::resource("/metrics/track")
                        .guard(guard::Post())
                        .to(super::track_metrics_handler),
                )
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let root_resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/metrics/track")
                .set_json(payload.clone())
                .to_request(),
        )
        .await;
        let root_status = root_resp.status();
        let root_json: serde_json::Value = test::read_body_json(root_resp).await;

        let scoped_resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/v1/metrics/track")
                .set_json(payload)
                .to_request(),
        )
        .await;
        let scoped_status = scoped_resp.status();
        let scoped_json: serde_json::Value = test::read_body_json(scoped_resp).await;

        assert_eq!(root_status, StatusCode::BAD_REQUEST);
        assert_eq!(root_status, scoped_status);
        assert_eq!(root_json, scoped_json);
        assert_eq!(root_json["error"], "invalid_input");
        assert_eq!(root_json["message"], "invalid environment_id");
    }

    #[actix_web::test]
    async fn track_metrics_endpoints_share_success_mapping_and_delegate_same_payload() {
        let metric_logic = RecordingMetricLogic::new(1);
        let payload = serde_json::json!({
            "clientId": Uuid::new_v4().to_string(),
            "clientSecret": "secret",
            "events": [{
                "metricKey": "signup_conversion",
                "featureKey": "feature-a",
                "environmentId": Uuid::new_v4().to_string(),
                "userContext": "user-123",
                "variant": "control",
                "value": 1.0,
                "metadata": { "source": "sdk" },
                "timestampUnixMs": 1_725_000_000_000_i64
            }]
        });

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(metric_logic.clone()) as Box<dyn MetricLogic>
                ))
                .service(
                    web::resource("/metrics/track")
                        .guard(guard::Post())
                        .to(super::track_metrics_handler),
                )
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let root_resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/metrics/track")
                .set_json(payload.clone())
                .to_request(),
        )
        .await;
        let root_status = root_resp.status();
        let root_json: serde_json::Value = test::read_body_json(root_resp).await;

        let scoped_resp = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/v1/metrics/track")
                .set_json(payload)
                .to_request(),
        )
        .await;
        let scoped_status = scoped_resp.status();
        let scoped_json: serde_json::Value = test::read_body_json(scoped_resp).await;

        assert_eq!(root_status, StatusCode::OK);
        assert_eq!(root_status, scoped_status);
        assert_eq!(root_json, scoped_json);
        assert_eq!(root_json["processed"], 1);

        let calls = metric_logic.recorded_calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|call| call.client_secret == "secret"));
        assert!(calls.iter().all(|call| call.events.len() == 1));
        assert_eq!(calls[0].client_id, calls[1].client_id);

        let first_event = &calls[0].events[0];
        assert_eq!(first_event.metric_key, "signup_conversion");
        assert_eq!(first_event.feature_key.as_deref(), Some("feature-a"));
        assert_eq!(first_event.variant.as_deref(), Some("control"));
        assert_eq!(first_event.user_context, "user-123");
        assert!(first_event.environment_id.is_some());
        assert!(first_event.timestamp.is_some());
    }

    #[::std::prelude::v1::test]
    fn experiment_analysis_recommends_winner_only_after_sample_and_confidence_pass() {
        let now = Utc::now();
        let mut results = vec![
            MetricResultResponse {
                metric_key: "signup".to_string(),
                variant: Some("control".to_string()),
                sample_size: 1_000,
                conversion_rate: Some(0.10),
                lift: None,
                confidence: None,
                mean_value: Some(0.10),
                p95_value: None,
                time_bucket: now,
                confidence_interval: None,
            },
            MetricResultResponse {
                metric_key: "signup".to_string(),
                variant: Some("treatment".to_string()),
                sample_size: 1_000,
                conversion_rate: Some(0.16),
                lift: None,
                confidence: None,
                mean_value: Some(0.16),
                p95_value: None,
                time_bucket: now,
                confidence_interval: None,
            },
        ];

        let details = analyze_experiment_results(DbMetricType::Conversion, &mut results);

        assert_eq!(details.winner.as_deref(), Some("treatment"));
        assert!(details.p_value.is_some_and(|p| p < 0.05));
        assert!(details.warnings.is_empty());
        let treatment = results
            .iter()
            .find(|result| result.variant.as_deref() == Some("treatment"))
            .expect("treatment result missing");
        assert!(treatment.lift.is_some_and(|lift| lift > 0.5));
        assert!(
            treatment
                .confidence
                .is_some_and(|confidence| confidence > 0.95)
        );
        assert!(treatment.confidence_interval.is_some());
    }

    #[::std::prelude::v1::test]
    fn experiment_analysis_warns_for_low_sample_and_sample_ratio_mismatch() {
        let now = Utc::now();
        let mut results = vec![
            MetricResultResponse {
                metric_key: "signup".to_string(),
                variant: Some("control".to_string()),
                sample_size: 180,
                conversion_rate: Some(0.20),
                lift: None,
                confidence: None,
                mean_value: Some(0.20),
                p95_value: None,
                time_bucket: now,
                confidence_interval: None,
            },
            MetricResultResponse {
                metric_key: "signup".to_string(),
                variant: Some("treatment".to_string()),
                sample_size: 20,
                conversion_rate: Some(0.25),
                lift: None,
                confidence: None,
                mean_value: Some(0.25),
                p95_value: None,
                time_bucket: now,
                confidence_interval: None,
            },
        ];

        let details = analyze_experiment_results(DbMetricType::Conversion, &mut results);

        assert!(details.winner.is_none());
        assert!(
            details
                .warnings
                .iter()
                .any(|warning| warning.contains("Low sample size"))
        );
        assert!(
            details
                .warnings
                .iter()
                .any(|warning| warning.contains("Sample-ratio mismatch"))
        );
        assert!(details.recommendation.contains("No winner yet"));
    }
}
