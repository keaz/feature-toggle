use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, sqlx::Type, ToSchema)]
#[sqlx(type_name = "varchar", rename_all = "lowercase")]
pub enum MetricType {
    Conversion,
    Numeric,
    Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMetric {
    pub team_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub metric_type: MetricType,
    pub unit: Option<String>,
    pub success_criteria: Option<serde_json::Value>,
}

#[derive(sqlx::FromRow, Debug, Clone, Serialize, Deserialize)]
pub struct MetricRow {
    pub id: Uuid,
    pub team_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub metric_type: MetricType,
    pub unit: Option<String>,
    pub success_criteria: Option<serde_json::Value>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMetricEvent {
    pub metric_id: Uuid,
    pub feature_key: Option<String>,
    pub environment_id: Option<Uuid>,
    pub user_context: String,
    pub variant: Option<String>,
    pub value: f64,
    pub metadata: Option<serde_json::Value>,
    pub occurred_at: DateTime<Utc>,
    /// True for conversion metrics with a positive conversion event so we can dedupe
    pub is_conversion: bool,
}

#[derive(sqlx::FromRow, Debug, Clone, Serialize, Deserialize)]
pub struct MetricEventRow {
    pub id: Uuid,
    pub metric_id: Uuid,
    pub feature_key: Option<String>,
    pub environment_id: Option<Uuid>,
    pub user_context: String,
    pub variant: Option<String>,
    pub value: f64,
    pub metadata: Option<serde_json::Value>,
    pub is_conversion: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow, Debug, Clone, Serialize, Deserialize)]
pub struct MetricAggregationRow {
    pub metric_id: Uuid,
    pub metric_key: String,
    pub metric_type: MetricType,
    pub feature_key: Option<String>,
    pub environment_id: Option<Uuid>,
    pub variant: Option<String>,
    pub time_bucket: DateTime<Utc>,
    pub sample_size: i64,
    pub sum_value: Option<f64>,
    pub mean_value: Option<f64>,
    pub min_value: Option<f64>,
    pub max_value: Option<f64>,
    pub p50_value: Option<f64>,
    pub p95_value: Option<f64>,
    pub p99_value: Option<f64>,
    pub conversion_count: Option<i64>,
    pub conversion_rate: Option<f64>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait MetricRepository: Send + Sync {
    async fn create_metric(&self, metric: CreateMetric) -> Result<MetricRow, sqlx::Error>;

    async fn get_metric_by_key(
        &self,
        team_id: Uuid,
        key: &str,
    ) -> Result<Option<MetricRow>, sqlx::Error>;

    async fn list_metrics(&self, team_id: Uuid) -> Result<Vec<MetricRow>, sqlx::Error>;

    async fn insert_metric_events(
        &self,
        events: Vec<CreateMetricEvent>,
    ) -> Result<Vec<MetricEventRow>, sqlx::Error>;

    /// Pre-compute aggregations into metric_aggregations between [from, to)
    async fn upsert_aggregations(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: &str,
    ) -> Result<u64, sqlx::Error>;

    async fn get_metric_results(
        &self,
        feature_key: &str,
        team_id: Option<Uuid>,
        environment_id: Option<Uuid>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<MetricAggregationRow>, sqlx::Error>;

    fn clone_box(&self) -> Box<dyn MetricRepository>;
}

impl Clone for Box<dyn MetricRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait MetricRepositoryTx: MetricRepository {
    async fn create_metric_tx(
        &self,
        conn: &mut PgConnection,
        metric: CreateMetric,
    ) -> Result<MetricRow, sqlx::Error>;
}

#[derive(Clone)]
pub struct PgMetricRepository {
    pool: PgPool,
}

pub fn metric_repository(pool: PgPool) -> Box<dyn MetricRepository> {
    Box::new(PgMetricRepository { pool })
}

pub fn metric_repository_tx(pool: PgPool) -> PgMetricRepository {
    PgMetricRepository { pool }
}

#[async_trait::async_trait]
impl MetricRepository for PgMetricRepository {
    async fn create_metric(&self, metric: CreateMetric) -> Result<MetricRow, sqlx::Error> {
        sqlx::query_as!(
            MetricRow,
            r#"
            INSERT INTO metrics (team_id, key, name, description, metric_type, unit, success_criteria)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING
                id,
                team_id,
                key,
                name,
                description,
                metric_type as "metric_type: MetricType",
                unit,
                success_criteria,
                created_at
            "#,
            metric.team_id,
            metric.key,
            metric.name,
            metric.description,
            metric.metric_type as MetricType,
            metric.unit,
            metric.success_criteria
        )
        .fetch_one(&self.pool)
        .await
    }

    async fn get_metric_by_key(
        &self,
        team_id: Uuid,
        key: &str,
    ) -> Result<Option<MetricRow>, sqlx::Error> {
        sqlx::query_as!(
            MetricRow,
            r#"
            SELECT
                id,
                team_id,
                key,
                name,
                description,
                metric_type as "metric_type: MetricType",
                unit,
                success_criteria,
                created_at
            FROM metrics
            WHERE team_id = $1 AND key = $2
            "#,
            team_id,
            key
        )
        .fetch_optional(&self.pool)
        .await
    }

    async fn list_metrics(&self, team_id: Uuid) -> Result<Vec<MetricRow>, sqlx::Error> {
        sqlx::query_as!(
            MetricRow,
            r#"
            SELECT
                id,
                team_id,
                key,
                name,
                description,
                metric_type as "metric_type: MetricType",
                unit,
                success_criteria,
                created_at
            FROM metrics
            WHERE team_id = $1
            ORDER BY created_at DESC
            "#,
            team_id
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn insert_metric_events(
        &self,
        events: Vec<CreateMetricEvent>,
    ) -> Result<Vec<MetricEventRow>, sqlx::Error> {
        if events.is_empty() {
            return Ok(vec![]);
        }

        let mut inserted = Vec::with_capacity(events.len());
        let mut tx = self.pool.begin().await?;

        for event in events {
            let row = if event.is_conversion {
                sqlx::query_as!(
                    MetricEventRow,
                    r#"
                    INSERT INTO metric_events (
                        metric_id,
                        feature_key,
                        environment_id,
                        user_context,
                        variant,
                        value,
                        metadata,
                        is_conversion,
                        occurred_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    ON CONFLICT (metric_id, feature_key, environment_id, user_context, variant)
                        WHERE is_conversion DO NOTHING
                    RETURNING
                        id,
                        metric_id,
                        feature_key,
                        environment_id,
                        user_context,
                        variant,
                        value,
                        metadata,
                        is_conversion,
                        created_at,
                        occurred_at
                    "#,
                    event.metric_id,
                    event.feature_key,
                    event.environment_id,
                    event.user_context,
                    event.variant,
                    event.value,
                    event.metadata,
                    event.is_conversion,
                    event.occurred_at
                )
                .fetch_optional(&mut *tx)
                .await?
            } else {
                Some(
                    sqlx::query_as!(
                        MetricEventRow,
                        r#"
                        INSERT INTO metric_events (
                            metric_id,
                            feature_key,
                            environment_id,
                            user_context,
                            variant,
                            value,
                            metadata,
                            is_conversion,
                            occurred_at
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                        RETURNING
                            id,
                            metric_id,
                            feature_key,
                            environment_id,
                            user_context,
                            variant,
                            value,
                            metadata,
                            is_conversion,
                            created_at,
                            occurred_at
                        "#,
                        event.metric_id,
                        event.feature_key,
                        event.environment_id,
                        event.user_context,
                        event.variant,
                        event.value,
                        event.metadata,
                        event.is_conversion,
                        event.occurred_at
                    )
                    .fetch_one(&mut *tx)
                    .await?,
                )
            };

            if let Some(r) = row {
                inserted.push(r);
            }
        }

        tx.commit().await?;
        Ok(inserted)
    }

    async fn upsert_aggregations(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query!(
            r#"
            INSERT INTO metric_aggregations (
                metric_id,
                feature_key,
                environment_id,
                variant,
                time_bucket,
                sample_size,
                sum_value,
                mean_value,
                min_value,
                max_value,
                p50_value,
                p95_value,
                p99_value,
                conversion_count,
                conversion_rate
            )
            SELECT
                me.metric_id,
                me.feature_key,
                me.environment_id,
                me.variant,
                date_trunc($3, me.occurred_at) AS time_bucket,
                COUNT(*) AS sample_size,
                SUM(me.value)::DOUBLE PRECISION AS sum_value,
                AVG(me.value)::DOUBLE PRECISION AS mean_value,
                MIN(me.value)::DOUBLE PRECISION AS min_value,
                MAX(me.value)::DOUBLE PRECISION AS max_value,
                percentile_disc(0.50) WITHIN GROUP (ORDER BY me.value) AS p50_value,
                percentile_disc(0.95) WITHIN GROUP (ORDER BY me.value) AS p95_value,
                percentile_disc(0.99) WITHIN GROUP (ORDER BY me.value) AS p99_value,
                SUM(
                    CASE
                        WHEN m.metric_type = 'conversion' AND me.value > 0 THEN 1
                        ELSE 0
                    END
                )::BIGINT AS conversion_count,
                CASE
                    WHEN COUNT(*) = 0 THEN 0::DOUBLE PRECISION
                    ELSE SUM(
                        CASE
                            WHEN m.metric_type = 'conversion' AND me.value > 0 THEN 1
                            ELSE 0
                        END
                    )::DOUBLE PRECISION / COUNT(*)::DOUBLE PRECISION
                END AS conversion_rate
            FROM metric_events me
            JOIN metrics m ON me.metric_id = m.id
            WHERE me.occurred_at >= $1 AND me.occurred_at < $2
            GROUP BY me.metric_id, me.feature_key, me.environment_id, me.variant, date_trunc($3, me.occurred_at)
            ON CONFLICT (metric_id, feature_key, environment_id, variant, time_bucket)
            DO UPDATE SET
                sample_size = EXCLUDED.sample_size,
                sum_value = EXCLUDED.sum_value,
                mean_value = EXCLUDED.mean_value,
                min_value = EXCLUDED.min_value,
                max_value = EXCLUDED.max_value,
                p50_value = EXCLUDED.p50_value,
                p95_value = EXCLUDED.p95_value,
                p99_value = EXCLUDED.p99_value,
                conversion_count = EXCLUDED.conversion_count,
                conversion_rate = EXCLUDED.conversion_rate
            "#,
            from,
            to,
            bucket
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    async fn get_metric_results(
        &self,
        feature_key: &str,
        team_id: Option<Uuid>,
        environment_id: Option<Uuid>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<MetricAggregationRow>, sqlx::Error> {
        sqlx::query_as!(
            MetricAggregationRow,
            r#"
            SELECT
                ma.metric_id,
                m.key as metric_key,
                m.metric_type as "metric_type: MetricType",
                ma.feature_key,
                ma.environment_id,
                ma.variant,
                ma.time_bucket,
                ma.sample_size,
                ma.sum_value,
                ma.mean_value,
                ma.min_value,
                ma.max_value,
                ma.p50_value,
                ma.p95_value,
                ma.p99_value,
                ma.conversion_count,
                ma.conversion_rate
            FROM metric_aggregations ma
            JOIN metrics m ON ma.metric_id = m.id
            WHERE ma.feature_key = $1
              AND ($2::UUID IS NULL OR m.team_id = $2)
              AND ($3::UUID IS NULL OR ma.environment_id = $3)
              AND ma.time_bucket >= date_trunc('hour', $4::timestamptz)
              AND ma.time_bucket < date_trunc('hour', $5::timestamptz) + INTERVAL '1 hour'
            ORDER BY ma.time_bucket DESC, ma.metric_id, COALESCE(ma.variant, '')
            "#,
            feature_key,
            team_id,
            environment_id,
            from,
            to
        )
        .fetch_all(&self.pool)
        .await
    }

    fn clone_box(&self) -> Box<dyn MetricRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl MetricRepositoryTx for PgMetricRepository {
    async fn create_metric_tx(
        &self,
        conn: &mut PgConnection,
        metric: CreateMetric,
    ) -> Result<MetricRow, sqlx::Error> {
        sqlx::query_as!(
            MetricRow,
            r#"
            INSERT INTO metrics (team_id, key, name, description, metric_type, unit, success_criteria)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING
                id,
                team_id,
                key,
                name,
                description,
                metric_type as "metric_type: MetricType",
                unit,
                success_criteria,
                created_at
            "#,
            metric.team_id,
            metric.key,
            metric.name,
            metric.description,
            metric.metric_type as MetricType,
            metric.unit,
            metric.success_criteria
        )
        .fetch_one(&mut *conn)
        .await
    }
}
