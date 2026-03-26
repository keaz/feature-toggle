use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, types::Uuid};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryGateConfigInput {
    pub metric_key: String,
    pub baseline_variant: String,
    pub canary_variant: String,
    pub direction: String,
    pub threshold_pct: f64,
    pub min_sample_size: i64,
    pub window_minutes: i32,
    pub auto_rollback_on_fail: bool,
    pub rollback_in_minutes: Option<i32>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CanaryGateRow {
    pub id: Uuid,
    pub stage_id: Uuid,
    pub feature_id: Uuid,
    pub environment_id: Uuid,
    pub metric_key: String,
    pub baseline_variant: String,
    pub canary_variant: String,
    pub direction: String,
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
pub struct CreateCanaryGateResult {
    pub gate_id: Uuid,
    pub feature_id: Uuid,
    pub passed: bool,
    pub reason: String,
    pub baseline_sample_size: i64,
    pub canary_sample_size: i64,
    pub baseline_value: Option<f64>,
    pub canary_value: Option<f64>,
    pub regression_pct: Option<f64>,
    pub threshold_pct: f64,
    pub rollback_triggered: bool,
    pub rollback_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CanaryGateResultRow {
    pub id: Uuid,
    pub gate_id: Uuid,
    pub feature_id: Uuid,
    pub passed: bool,
    pub reason: String,
    pub baseline_sample_size: i64,
    pub canary_sample_size: i64,
    pub baseline_value: Option<f64>,
    pub canary_value: Option<f64>,
    pub regression_pct: Option<f64>,
    pub threshold_pct: f64,
    pub rollback_triggered: bool,
    pub rollback_error: Option<String>,
    pub evaluated_at: DateTime<Utc>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait CanaryRepository: Send + Sync {
    async fn replace_stage_gates(
        &self,
        stage_id: Uuid,
        feature_id: Uuid,
        environment_id: Uuid,
        gates: Vec<CanaryGateConfigInput>,
    ) -> Result<Vec<CanaryGateRow>, sqlx::Error>;

    async fn list_gates_by_stage(&self, stage_id: Uuid) -> Result<Vec<CanaryGateRow>, sqlx::Error>;

    async fn get_gate_by_id(&self, gate_id: Uuid) -> Result<Option<CanaryGateRow>, sqlx::Error>;

    async fn list_enabled_gates(&self) -> Result<Vec<CanaryGateRow>, sqlx::Error>;

    async fn insert_gate_result(
        &self,
        input: CreateCanaryGateResult,
    ) -> Result<CanaryGateResultRow, sqlx::Error>;

    fn clone_box(&self) -> Box<dyn CanaryRepository>;
}

impl Clone for Box<dyn CanaryRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Clone)]
pub struct PgCanaryRepository {
    pool: PgPool,
}

pub fn canary_repository(pool: PgPool) -> Box<dyn CanaryRepository> {
    Box::new(PgCanaryRepository { pool })
}

#[async_trait::async_trait]
impl CanaryRepository for PgCanaryRepository {
    async fn replace_stage_gates(
        &self,
        stage_id: Uuid,
        feature_id: Uuid,
        environment_id: Uuid,
        gates: Vec<CanaryGateConfigInput>,
    ) -> Result<Vec<CanaryGateRow>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM rollout_canary_gates WHERE stage_id = $1")
            .bind(stage_id)
            .execute(&mut *tx)
            .await?;

        let mut created = Vec::with_capacity(gates.len());
        for gate in gates {
            let row = sqlx::query_as::<_, CanaryGateRow>(
                r#"
                INSERT INTO rollout_canary_gates (
                    stage_id,
                    feature_id,
                    environment_id,
                    metric_key,
                    baseline_variant,
                    canary_variant,
                    direction,
                    threshold_pct,
                    min_sample_size,
                    window_minutes,
                    auto_rollback_on_fail,
                    rollback_in_minutes,
                    enabled
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                RETURNING
                    id,
                    stage_id,
                    feature_id,
                    environment_id,
                    metric_key,
                    baseline_variant,
                    canary_variant,
                    direction,
                    threshold_pct,
                    min_sample_size,
                    window_minutes,
                    auto_rollback_on_fail,
                    rollback_in_minutes,
                    enabled,
                    created_at,
                    updated_at
                "#,
            )
            .bind(stage_id)
            .bind(feature_id)
            .bind(environment_id)
            .bind(gate.metric_key)
            .bind(gate.baseline_variant)
            .bind(gate.canary_variant)
            .bind(gate.direction)
            .bind(gate.threshold_pct)
            .bind(gate.min_sample_size)
            .bind(gate.window_minutes)
            .bind(gate.auto_rollback_on_fail)
            .bind(gate.rollback_in_minutes)
            .bind(gate.enabled)
            .fetch_one(&mut *tx)
            .await?;

            created.push(row);
        }

        tx.commit().await?;
        Ok(created)
    }

    async fn list_gates_by_stage(&self, stage_id: Uuid) -> Result<Vec<CanaryGateRow>, sqlx::Error> {
        sqlx::query_as::<_, CanaryGateRow>(
            r#"
            SELECT
                id,
                stage_id,
                feature_id,
                environment_id,
                metric_key,
                baseline_variant,
                canary_variant,
                direction,
                threshold_pct,
                min_sample_size,
                window_minutes,
                auto_rollback_on_fail,
                rollback_in_minutes,
                enabled,
                created_at,
                updated_at
            FROM rollout_canary_gates
            WHERE stage_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(stage_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn get_gate_by_id(&self, gate_id: Uuid) -> Result<Option<CanaryGateRow>, sqlx::Error> {
        sqlx::query_as::<_, CanaryGateRow>(
            r#"
            SELECT
                id,
                stage_id,
                feature_id,
                environment_id,
                metric_key,
                baseline_variant,
                canary_variant,
                direction,
                threshold_pct,
                min_sample_size,
                window_minutes,
                auto_rollback_on_fail,
                rollback_in_minutes,
                enabled,
                created_at,
                updated_at
            FROM rollout_canary_gates
            WHERE id = $1
            "#,
        )
        .bind(gate_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn list_enabled_gates(&self) -> Result<Vec<CanaryGateRow>, sqlx::Error> {
        sqlx::query_as::<_, CanaryGateRow>(
            r#"
            SELECT
                id,
                stage_id,
                feature_id,
                environment_id,
                metric_key,
                baseline_variant,
                canary_variant,
                direction,
                threshold_pct,
                min_sample_size,
                window_minutes,
                auto_rollback_on_fail,
                rollback_in_minutes,
                enabled,
                created_at,
                updated_at
            FROM rollout_canary_gates
            WHERE enabled = TRUE
            ORDER BY updated_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn insert_gate_result(
        &self,
        input: CreateCanaryGateResult,
    ) -> Result<CanaryGateResultRow, sqlx::Error> {
        sqlx::query_as::<_, CanaryGateResultRow>(
            r#"
            INSERT INTO rollout_canary_gate_results (
                gate_id,
                feature_id,
                passed,
                reason,
                baseline_sample_size,
                canary_sample_size,
                baseline_value,
                canary_value,
                regression_pct,
                threshold_pct,
                rollback_triggered,
                rollback_error
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING
                id,
                gate_id,
                feature_id,
                passed,
                reason,
                baseline_sample_size,
                canary_sample_size,
                baseline_value,
                canary_value,
                regression_pct,
                threshold_pct,
                rollback_triggered,
                rollback_error,
                evaluated_at
            "#,
        )
        .bind(input.gate_id)
        .bind(input.feature_id)
        .bind(input.passed)
        .bind(input.reason)
        .bind(input.baseline_sample_size)
        .bind(input.canary_sample_size)
        .bind(input.baseline_value)
        .bind(input.canary_value)
        .bind(input.regression_pct)
        .bind(input.threshold_pct)
        .bind(input.rollback_triggered)
        .bind(input.rollback_error)
        .fetch_one(&self.pool)
        .await
    }

    fn clone_box(&self) -> Box<dyn CanaryRepository> {
        Box::new(self.clone())
    }
}
