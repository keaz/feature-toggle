use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::canary::{
    CanaryGateConfigInput, CanaryGateResultRow, CanaryGateRow, CanaryRepository,
    CreateCanaryGateResult,
};
use crate::database::feature::FeatureRepository;
use crate::logic::feature::FeatureLogic;
use crate::logic::metrics::{MetricLogic, MetricLogicError};
use crate::model::ID;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CanaryDirection {
    HigherIsBetter,
    LowerIsBetter,
}

impl CanaryDirection {
    fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "HIGHER_IS_BETTER" => Some(Self::HigherIsBetter),
            "LOWER_IS_BETTER" => Some(Self::LowerIsBetter),
            _ => None,
        }
    }

    fn as_db_value(self) -> &'static str {
        match self {
            Self::HigherIsBetter => "HIGHER_IS_BETTER",
            Self::LowerIsBetter => "LOWER_IS_BETTER",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanaryGateInput {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanaryGate {
    pub id: Uuid,
    pub stage_id: Uuid,
    pub feature_id: Uuid,
    pub environment_id: Uuid,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanaryVariantSnapshot {
    pub variant: String,
    pub sample_size: i64,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanaryAnalysisResult {
    pub gate_id: Uuid,
    pub feature_id: Uuid,
    pub metric_key: String,
    pub passed: bool,
    pub reason: String,
    pub baseline: CanaryVariantSnapshot,
    pub canary: CanaryVariantSnapshot,
    pub regression_pct: Option<f64>,
    pub threshold_pct: f64,
    pub rollback_triggered: bool,
    pub rollback_error: Option<String>,
    pub evaluated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum CanaryLogicError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("feature operation failed: {0}")]
    Feature(#[from] crate::Error),
    #[error("metrics query failed: {0}")]
    Metrics(#[from] MetricLogicError),
}

#[async_trait::async_trait]
pub trait CanaryLogic: Send + Sync {
    async fn replace_stage_gates(
        &self,
        stage_id: Uuid,
        gates: Vec<CanaryGateInput>,
    ) -> Result<Vec<CanaryGate>, CanaryLogicError>;

    async fn list_stage_gates(&self, stage_id: Uuid) -> Result<Vec<CanaryGate>, CanaryLogicError>;

    async fn analyze_gate(
        &self,
        gate_id: Uuid,
        force_rollback: Option<bool>,
    ) -> Result<CanaryAnalysisResult, CanaryLogicError>;

    async fn analyze_enabled_gates(&self) -> Result<usize, CanaryLogicError>;

    fn clone_box(&self) -> Box<dyn CanaryLogic>;
}

impl Clone for Box<dyn CanaryLogic> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub fn canary_logic(
    canary_repository: Box<dyn CanaryRepository>,
    metric_logic: Box<dyn MetricLogic>,
    feature_repository: Box<dyn FeatureRepository>,
    feature_logic: Box<dyn FeatureLogic>,
    activity_log_repository: Box<dyn ActivityLogRepository>,
) -> Box<dyn CanaryLogic> {
    Box::new(CanaryLogicImpl {
        canary_repository,
        metric_logic,
        feature_repository,
        feature_logic,
        activity_log_repository,
    })
}

struct CanaryLogicImpl {
    canary_repository: Box<dyn CanaryRepository>,
    metric_logic: Box<dyn MetricLogic>,
    feature_repository: Box<dyn FeatureRepository>,
    feature_logic: Box<dyn FeatureLogic>,
    activity_log_repository: Box<dyn ActivityLogRepository>,
}

impl Clone for CanaryLogicImpl {
    fn clone(&self) -> Self {
        Self {
            canary_repository: self.canary_repository.clone_box(),
            metric_logic: self.metric_logic.clone_box(),
            feature_repository: self.feature_repository.clone_box(),
            feature_logic: self.feature_logic.clone_box(),
            activity_log_repository: self.activity_log_repository.clone_box(),
        }
    }
}

#[derive(Debug)]
struct VariantAggregate {
    sample_size: i64,
    value: Option<f64>,
}

impl CanaryLogicImpl {
    fn map_gate_row(row: CanaryGateRow) -> Result<CanaryGate, CanaryLogicError> {
        let direction = CanaryDirection::from_db_value(&row.direction).ok_or_else(|| {
            CanaryLogicError::InvalidInput(format!(
                "unsupported canary direction '{}' for gate {}",
                row.direction, row.id
            ))
        })?;

        Ok(CanaryGate {
            id: row.id,
            stage_id: row.stage_id,
            feature_id: row.feature_id,
            environment_id: row.environment_id,
            metric_key: row.metric_key,
            baseline_variant: row.baseline_variant,
            canary_variant: row.canary_variant,
            direction,
            threshold_pct: row.threshold_pct,
            min_sample_size: row.min_sample_size,
            window_minutes: row.window_minutes,
            auto_rollback_on_fail: row.auto_rollback_on_fail,
            rollback_in_minutes: row.rollback_in_minutes,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    fn validate_gate_input(gate: &CanaryGateInput) -> Result<(), CanaryLogicError> {
        if gate.metric_key.trim().is_empty() {
            return Err(CanaryLogicError::InvalidInput(
                "metricKey must not be empty".to_string(),
            ));
        }

        if gate.baseline_variant.trim().is_empty() {
            return Err(CanaryLogicError::InvalidInput(
                "baselineVariant must not be empty".to_string(),
            ));
        }

        if gate.canary_variant.trim().is_empty() {
            return Err(CanaryLogicError::InvalidInput(
                "canaryVariant must not be empty".to_string(),
            ));
        }

        if gate.baseline_variant.trim() == gate.canary_variant.trim() {
            return Err(CanaryLogicError::InvalidInput(
                "baselineVariant and canaryVariant must be different".to_string(),
            ));
        }

        if !gate.threshold_pct.is_finite() || gate.threshold_pct < 0.0 {
            return Err(CanaryLogicError::InvalidInput(
                "thresholdPct must be >= 0".to_string(),
            ));
        }

        if gate.min_sample_size <= 0 {
            return Err(CanaryLogicError::InvalidInput(
                "minSampleSize must be greater than 0".to_string(),
            ));
        }

        if gate.window_minutes <= 0 {
            return Err(CanaryLogicError::InvalidInput(
                "windowMinutes must be greater than 0".to_string(),
            ));
        }

        if let Some(minutes) = gate.rollback_in_minutes
            && minutes <= 0
        {
            return Err(CanaryLogicError::InvalidInput(
                "rollbackInMinutes must be greater than 0 when provided".to_string(),
            ));
        }

        Ok(())
    }

    fn aggregate_variant(
        rows: &[crate::database::metrics::MetricAggregationRow],
        metric_key: &str,
        variant: &str,
    ) -> VariantAggregate {
        let matching = rows
            .iter()
            .filter(|row| row.metric_key == metric_key && row.variant.as_deref() == Some(variant))
            .collect::<Vec<_>>();

        if matching.is_empty() {
            return VariantAggregate {
                sample_size: 0,
                value: None,
            };
        }

        let sample_size = matching.iter().map(|row| row.sample_size).sum::<i64>();
        if sample_size <= 0 {
            return VariantAggregate {
                sample_size,
                value: None,
            };
        }

        let metric_type = matching[0].metric_type;
        let value = match metric_type {
            crate::database::metrics::MetricType::Conversion => {
                let mut conversion_total = 0.0;
                let mut has_conversion_data = false;

                for row in &matching {
                    if let Some(conversion_count) = row.conversion_count {
                        conversion_total += conversion_count as f64;
                        has_conversion_data = true;
                    } else if let Some(conversion_rate) = row.conversion_rate {
                        conversion_total += conversion_rate * row.sample_size as f64;
                        has_conversion_data = true;
                    }
                }

                if has_conversion_data {
                    Some(conversion_total / sample_size as f64)
                } else {
                    None
                }
            }
            _ => {
                let mut weighted_total = 0.0;
                let mut has_data = false;

                for row in &matching {
                    if let Some(sum_value) = row.sum_value {
                        weighted_total += sum_value;
                        has_data = true;
                    } else if let Some(mean_value) = row.mean_value {
                        weighted_total += mean_value * row.sample_size as f64;
                        has_data = true;
                    }
                }

                if has_data {
                    Some(weighted_total / sample_size as f64)
                } else {
                    None
                }
            }
        };

        VariantAggregate { sample_size, value }
    }

    fn compute_regression_pct(
        direction: CanaryDirection,
        baseline_value: f64,
        canary_value: f64,
    ) -> Option<f64> {
        if !baseline_value.is_finite() || !canary_value.is_finite() {
            return None;
        }

        let raw_regression = match direction {
            CanaryDirection::HigherIsBetter => {
                if baseline_value <= 0.0 {
                    if canary_value < baseline_value {
                        100.0
                    } else {
                        0.0
                    }
                } else {
                    ((baseline_value - canary_value) / baseline_value) * 100.0
                }
            }
            CanaryDirection::LowerIsBetter => {
                if baseline_value <= 0.0 {
                    if canary_value > baseline_value {
                        100.0
                    } else {
                        0.0
                    }
                } else {
                    ((canary_value - baseline_value) / baseline_value) * 100.0
                }
            }
        };

        Some(raw_regression.max(0.0))
    }

    async fn evaluate_gate_row(
        &self,
        gate_row: CanaryGateRow,
        force_rollback: Option<bool>,
    ) -> Result<CanaryAnalysisResult, CanaryLogicError> {
        let gate = Self::map_gate_row(gate_row.clone())?;
        let feature = self
            .feature_repository
            .get_feature_by_id(gate.feature_id)
            .await
            .map_err(CanaryLogicError::Feature)?;

        let now = Utc::now();
        let from = now - ChronoDuration::minutes(gate.window_minutes as i64);

        let rows = self
            .metric_logic
            .get_metric_results(&feature.key, None, Some(gate.environment_id), from, now)
            .await?;

        let baseline = Self::aggregate_variant(&rows, &gate.metric_key, &gate.baseline_variant);
        let canary = Self::aggregate_variant(&rows, &gate.metric_key, &gate.canary_variant);

        let (passed, reason, regression_pct, rollback_eligible) = if baseline.sample_size
            < gate.min_sample_size
            || canary.sample_size < gate.min_sample_size
        {
            (
                false,
                format!(
                    "Insufficient sample size for canary analysis (baseline={}, canary={}, minRequired={})",
                    baseline.sample_size, canary.sample_size, gate.min_sample_size
                ),
                None,
                false,
            )
        } else if baseline.value.is_none() || canary.value.is_none() {
            (
                false,
                format!(
                    "Metric data for '{}' could not be calculated for one or both variants",
                    gate.metric_key
                ),
                None,
                false,
            )
        } else {
            let baseline_value = baseline.value.unwrap_or_default();
            let canary_value = canary.value.unwrap_or_default();
            let regression_pct =
                Self::compute_regression_pct(gate.direction, baseline_value, canary_value);
            let regression = regression_pct.unwrap_or(0.0);

            if regression > gate.threshold_pct {
                (
                    false,
                    format!(
                        "Canary gate failed: regression {:.4}% exceeded threshold {:.4}%",
                        regression, gate.threshold_pct
                    ),
                    regression_pct,
                    true,
                )
            } else {
                (
                    true,
                    format!(
                        "Canary gate passed: regression {:.4}% is within threshold {:.4}%",
                        regression, gate.threshold_pct
                    ),
                    regression_pct,
                    false,
                )
            }
        };

        let should_rollback = force_rollback.unwrap_or(gate.auto_rollback_on_fail);
        let mut rollback_triggered = false;
        let mut rollback_error = None;

        if !passed && rollback_eligible && should_rollback {
            match self
                .feature_logic
                .emergency_disable_feature(
                    ID::from(gate.feature_id),
                    gate.rollback_in_minutes,
                    "Canary gate failed; automatic rollback triggered".to_string(),
                    None,
                    None,
                )
                .await
            {
                Ok(_) => {
                    rollback_triggered = true;
                }
                Err(err) => {
                    rollback_error = Some(err.to_string());
                }
            }
        }

        let result_row = self
            .canary_repository
            .insert_gate_result(CreateCanaryGateResult {
                gate_id: gate.id,
                feature_id: gate.feature_id,
                passed,
                reason: reason.clone(),
                baseline_sample_size: baseline.sample_size,
                canary_sample_size: canary.sample_size,
                baseline_value: baseline.value,
                canary_value: canary.value,
                regression_pct,
                threshold_pct: gate.threshold_pct,
                rollback_triggered,
                rollback_error: rollback_error.clone(),
            })
            .await?;

        let activity_type = if passed {
            "canary_gate_passed"
        } else {
            "canary_gate_failed"
        };

        self.activity_log_repository
            .create_activity(CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: "feature".to_string(),
                entity_id: gate.feature_id.to_string(),
                actor_id: None,
                actor_name: Some("canary-governance".to_string()),
                description: format!(
                    "Canary analysis {} for feature '{}' using metric '{}': {}",
                    if passed { "passed" } else { "failed" },
                    feature.key,
                    gate.metric_key,
                    reason
                ),
                metadata: Some(serde_json::json!({
                    "gate_id": gate.id.to_string(),
                    "stage_id": gate.stage_id.to_string(),
                    "feature_id": gate.feature_id.to_string(),
                    "environment_id": gate.environment_id.to_string(),
                    "metric_key": gate.metric_key,
                    "baseline_variant": gate.baseline_variant,
                    "canary_variant": gate.canary_variant,
                    "threshold_pct": gate.threshold_pct,
                    "regression_pct": regression_pct,
                    "baseline_sample_size": baseline.sample_size,
                    "canary_sample_size": canary.sample_size,
                    "rollback_triggered": rollback_triggered,
                    "rollback_error": rollback_error,
                })),
            })
            .await
            .map_err(CanaryLogicError::Database)?;

        Ok(Self::map_analysis_result(
            result_row, gate, baseline, canary,
        ))
    }

    fn map_analysis_result(
        row: CanaryGateResultRow,
        gate: CanaryGate,
        baseline: VariantAggregate,
        canary: VariantAggregate,
    ) -> CanaryAnalysisResult {
        CanaryAnalysisResult {
            gate_id: gate.id,
            feature_id: gate.feature_id,
            metric_key: gate.metric_key,
            passed: row.passed,
            reason: row.reason,
            baseline: CanaryVariantSnapshot {
                variant: gate.baseline_variant,
                sample_size: baseline.sample_size,
                value: baseline.value,
            },
            canary: CanaryVariantSnapshot {
                variant: gate.canary_variant,
                sample_size: canary.sample_size,
                value: canary.value,
            },
            regression_pct: row.regression_pct,
            threshold_pct: row.threshold_pct,
            rollback_triggered: row.rollback_triggered,
            rollback_error: row.rollback_error,
            evaluated_at: row.evaluated_at,
        }
    }
}

#[async_trait::async_trait]
impl CanaryLogic for CanaryLogicImpl {
    async fn replace_stage_gates(
        &self,
        stage_id: Uuid,
        gates: Vec<CanaryGateInput>,
    ) -> Result<Vec<CanaryGate>, CanaryLogicError> {
        let stage = self
            .feature_repository
            .get_stage_by_id(stage_id)
            .await
            .map_err(CanaryLogicError::Feature)?
            .ok_or_else(|| CanaryLogicError::NotFound(format!("stage {} not found", stage_id)))?;
        let feature = self
            .feature_repository
            .get_feature_by_id(stage.feature_id)
            .await
            .map_err(CanaryLogicError::Feature)?;
        let known_metric_keys: std::collections::HashSet<String> = self
            .metric_logic
            .list_metrics(feature.team_id)
            .await?
            .into_iter()
            .map(|metric| metric.key)
            .collect();

        if gates.is_empty() {
            self.canary_repository
                .replace_stage_gates(stage_id, stage.feature_id, stage.environment_id, Vec::new())
                .await?;
            return Ok(Vec::new());
        }

        let mut payload = Vec::with_capacity(gates.len());
        for gate in gates {
            Self::validate_gate_input(&gate)?;
            let metric_key = gate.metric_key.trim().to_string();
            if !known_metric_keys.contains(&metric_key) {
                return Err(CanaryLogicError::NotFound(format!(
                    "metric '{}' not found for feature team",
                    metric_key
                )));
            }
            payload.push(CanaryGateConfigInput {
                metric_key,
                baseline_variant: gate.baseline_variant.trim().to_string(),
                canary_variant: gate.canary_variant.trim().to_string(),
                direction: gate.direction.as_db_value().to_string(),
                threshold_pct: gate.threshold_pct,
                min_sample_size: gate.min_sample_size,
                window_minutes: gate.window_minutes,
                auto_rollback_on_fail: gate.auto_rollback_on_fail,
                rollback_in_minutes: gate.rollback_in_minutes,
                enabled: gate.enabled,
            });
        }

        let rows = self
            .canary_repository
            .replace_stage_gates(stage_id, stage.feature_id, stage.environment_id, payload)
            .await?;

        rows.into_iter().map(Self::map_gate_row).collect()
    }

    async fn list_stage_gates(&self, stage_id: Uuid) -> Result<Vec<CanaryGate>, CanaryLogicError> {
        let rows = self.canary_repository.list_gates_by_stage(stage_id).await?;
        rows.into_iter().map(Self::map_gate_row).collect()
    }

    async fn analyze_gate(
        &self,
        gate_id: Uuid,
        force_rollback: Option<bool>,
    ) -> Result<CanaryAnalysisResult, CanaryLogicError> {
        let gate = self
            .canary_repository
            .get_gate_by_id(gate_id)
            .await?
            .ok_or_else(|| {
                CanaryLogicError::NotFound(format!("canary gate {} not found", gate_id))
            })?;

        self.evaluate_gate_row(gate, force_rollback).await
    }

    async fn analyze_enabled_gates(&self) -> Result<usize, CanaryLogicError> {
        let gates = self.canary_repository.list_enabled_gates().await?;
        let mut processed = 0usize;

        for gate in gates {
            match self.evaluate_gate_row(gate, None).await {
                Ok(_) => {
                    processed += 1;
                }
                Err(err) => {
                    log::warn!("canary analysis failed for one gate: {}", err);
                }
            }
        }

        Ok(processed)
    }

    fn clone_box(&self) -> Box<dyn CanaryLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::{ActivityLogRow, MockActivityLogRepository};
    use crate::database::canary::{CanaryGateResultRow, MockCanaryRepository};
    use crate::database::entity::{Feature as DbFeature, FeatureType as DbFeatureType};
    use crate::database::feature::MockFeatureRepository;
    use crate::logic::feature::MockFeatureLogic;

    #[derive(Clone)]
    struct StubMetricLogic {
        rows: Vec<crate::database::metrics::MetricAggregationRow>,
    }

    #[async_trait::async_trait]
    impl MetricLogic for StubMetricLogic {
        async fn create_metric(
            &self,
            _team_id: Uuid,
            _key: String,
            _name: String,
            _description: Option<String>,
            _metric_type: crate::database::metrics::MetricType,
            _unit: Option<String>,
            _success_criteria: Option<serde_json::Value>,
        ) -> Result<crate::database::metrics::MetricRow, MetricLogicError> {
            Err(MetricLogicError::InvalidInput(
                "not implemented in test".to_string(),
            ))
        }

        async fn track_metrics(
            &self,
            _client_id: &str,
            _client_secret: &str,
            _events: Vec<crate::logic::metrics::TrackMetricInput>,
        ) -> Result<usize, MetricLogicError> {
            Err(MetricLogicError::InvalidInput(
                "not implemented in test".to_string(),
            ))
        }

        async fn aggregate_metrics(
            &self,
            _from: DateTime<Utc>,
            _to: DateTime<Utc>,
            _bucket: &str,
        ) -> Result<u64, MetricLogicError> {
            Err(MetricLogicError::InvalidInput(
                "not implemented in test".to_string(),
            ))
        }

        async fn get_metric_results(
            &self,
            _feature_key: &str,
            _team_id: Option<Uuid>,
            _environment_id: Option<Uuid>,
            _from: DateTime<Utc>,
            _to: DateTime<Utc>,
        ) -> Result<Vec<crate::database::metrics::MetricAggregationRow>, MetricLogicError> {
            Ok(self.rows.clone())
        }

        async fn list_metrics(
            &self,
            _team_id: Uuid,
        ) -> Result<Vec<crate::database::metrics::MetricRow>, MetricLogicError> {
            Err(MetricLogicError::InvalidInput(
                "not implemented in test".to_string(),
            ))
        }

        fn clone_box(&self) -> Box<dyn MetricLogic> {
            Box::new(self.clone())
        }
    }

    fn sample_gate(auto_rollback: bool) -> CanaryGateRow {
        CanaryGateRow {
            id: Uuid::new_v4(),
            stage_id: Uuid::new_v4(),
            feature_id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            metric_key: "conversion_signup".to_string(),
            baseline_variant: "baseline".to_string(),
            canary_variant: "canary".to_string(),
            direction: "HIGHER_IS_BETTER".to_string(),
            threshold_pct: 5.0,
            min_sample_size: 50,
            window_minutes: 60,
            auto_rollback_on_fail: auto_rollback,
            rollback_in_minutes: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_feature(feature_id: Uuid) -> DbFeature {
        DbFeature {
            id: feature_id,
            key: "checkout_redesign".to_string(),
            description: None,
            feature_type: DbFeatureType::Simple,
            team_id: Uuid::new_v4(),
            active: true,
            created_at: Utc::now(),
            kill_switch_enabled: true,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            lifecycle_stage: "active".to_string(),
            owner: None,
            expires_at: None,
            cleanup_reason: None,
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
        }
    }

    fn sample_result_row(
        gate: &CanaryGateRow,
        passed: bool,
        rollback: bool,
    ) -> CanaryGateResultRow {
        CanaryGateResultRow {
            id: Uuid::new_v4(),
            gate_id: gate.id,
            feature_id: gate.feature_id,
            passed,
            reason: if passed {
                "pass".to_string()
            } else {
                "fail".to_string()
            },
            baseline_sample_size: 100,
            canary_sample_size: 100,
            baseline_value: Some(0.6),
            canary_value: Some(0.58),
            regression_pct: Some(if passed { 3.33 } else { 33.33 }),
            threshold_pct: gate.threshold_pct,
            rollback_triggered: rollback,
            rollback_error: None,
            evaluated_at: Utc::now(),
        }
    }

    fn sample_metric_rows(
        canary_conversion_count: i64,
    ) -> Vec<crate::database::metrics::MetricAggregationRow> {
        vec![
            crate::database::metrics::MetricAggregationRow {
                metric_id: Uuid::new_v4(),
                metric_key: "conversion_signup".to_string(),
                metric_type: crate::database::metrics::MetricType::Conversion,
                feature_key: Some("checkout_redesign".to_string()),
                environment_id: Some(Uuid::new_v4()),
                variant: Some("baseline".to_string()),
                time_bucket: Utc::now(),
                sample_size: 100,
                sum_value: None,
                mean_value: None,
                min_value: None,
                max_value: None,
                p50_value: None,
                p95_value: None,
                p99_value: None,
                conversion_count: Some(60),
                conversion_rate: Some(0.6),
            },
            crate::database::metrics::MetricAggregationRow {
                metric_id: Uuid::new_v4(),
                metric_key: "conversion_signup".to_string(),
                metric_type: crate::database::metrics::MetricType::Conversion,
                feature_key: Some("checkout_redesign".to_string()),
                environment_id: Some(Uuid::new_v4()),
                variant: Some("canary".to_string()),
                time_bucket: Utc::now(),
                sample_size: 100,
                sum_value: None,
                mean_value: None,
                min_value: None,
                max_value: None,
                p50_value: None,
                p95_value: None,
                p99_value: None,
                conversion_count: Some(canary_conversion_count),
                conversion_rate: Some(canary_conversion_count as f64 / 100.0),
            },
        ]
    }

    fn sample_activity_row() -> ActivityLogRow {
        ActivityLogRow {
            id: Uuid::new_v4(),
            activity_type: "canary_gate_passed".to_string(),
            entity_type: "feature".to_string(),
            entity_id: Uuid::new_v4().to_string(),
            actor_id: None,
            actor_name: Some("canary-governance".to_string()),
            description: "ok".to_string(),
            metadata: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn analyze_gate_passes_without_rollback() {
        let gate = sample_gate(false);
        let feature = sample_feature(gate.feature_id);

        let mut canary_repo = MockCanaryRepository::new();
        let gate_for_lookup = gate.clone();
        canary_repo
            .expect_get_gate_by_id()
            .times(1)
            .returning(move |_| Ok(Some(gate_for_lookup.clone())));

        let gate_for_result = gate.clone();
        canary_repo
            .expect_insert_gate_result()
            .times(1)
            .withf(|input| input.passed && !input.rollback_triggered)
            .returning(move |_| Ok(sample_result_row(&gate_for_result, true, false)));

        let mut feature_repo = MockFeatureRepository::new();
        feature_repo
            .expect_get_feature_by_id()
            .times(1)
            .returning(move |_| Ok(feature.clone()));

        let mut feature_logic = MockFeatureLogic::new();
        feature_logic.expect_emergency_disable_feature().times(0);

        let metric_logic = StubMetricLogic {
            rows: sample_metric_rows(58),
        };

        let mut activity_repo = MockActivityLogRepository::new();
        activity_repo
            .expect_create_activity()
            .times(1)
            .returning(|_| Ok(sample_activity_row()));

        let logic = canary_logic(
            Box::new(canary_repo),
            Box::new(metric_logic),
            Box::new(feature_repo),
            Box::new(feature_logic),
            Box::new(activity_repo),
        );

        let result = logic
            .analyze_gate(gate.id, None)
            .await
            .expect("analysis should pass");
        assert!(result.passed);
        assert!(!result.rollback_triggered);
    }

    #[tokio::test]
    async fn analyze_gate_failure_triggers_rollback_when_enabled() {
        let gate = sample_gate(true);
        let feature = sample_feature(gate.feature_id);

        let mut canary_repo = MockCanaryRepository::new();
        let gate_for_lookup = gate.clone();
        canary_repo
            .expect_get_gate_by_id()
            .times(1)
            .returning(move |_| Ok(Some(gate_for_lookup.clone())));

        let gate_for_result = gate.clone();
        canary_repo
            .expect_insert_gate_result()
            .times(1)
            .withf(|input| !input.passed && input.rollback_triggered)
            .returning(move |_| Ok(sample_result_row(&gate_for_result, false, true)));

        let mut feature_repo = MockFeatureRepository::new();
        feature_repo
            .expect_get_feature_by_id()
            .times(1)
            .returning(move |_| Ok(feature.clone()));

        let mut feature_logic = MockFeatureLogic::new();
        feature_logic
            .expect_emergency_disable_feature()
            .times(1)
            .returning(|_, _, _, _, _| {
                Ok(crate::model::Feature {
                    id: ID::from(Uuid::new_v4()),
                    key: "checkout_redesign".to_string(),
                    description: None,
                    feature_type: crate::model::FeatureType::Simple,
                    enabled: false,
                    created_at: Utc::now(),
                    kill_switch_enabled: false,
                    kill_switch_activated_at: Some(Utc::now()),
                    rollback_scheduled_at: None,
                    emergency_override_reason: Some(
                        "Canary gate failed; automatic rollback triggered".to_string(),
                    ),
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: Some(Utc::now()),
                    lifecycle_stage: crate::model::LifecycleStage::Active,
                    owner: None,
                    expires_at: None,
                    cleanup_reason: None,
                    archived_at: None,
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    is_stale: false,
                    stale_reasons: vec![],
                    dependencies: vec![],
                    team_id: ID::from(Uuid::new_v4()),
                    pending_approval_request_id: None,
                })
            });

        let metric_logic = StubMetricLogic {
            rows: sample_metric_rows(40),
        };

        let mut activity_repo = MockActivityLogRepository::new();
        activity_repo
            .expect_create_activity()
            .times(1)
            .returning(|_| Ok(sample_activity_row()));

        let logic = canary_logic(
            Box::new(canary_repo),
            Box::new(metric_logic),
            Box::new(feature_repo),
            Box::new(feature_logic),
            Box::new(activity_repo),
        );

        let result = logic
            .analyze_gate(gate.id, None)
            .await
            .expect("analysis should fail with rollback");
        assert!(!result.passed);
        assert!(result.rollback_triggered);
    }
}
