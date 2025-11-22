use crate::database::client::ClientRepository;
use crate::database::metrics::{
    CreateMetric, CreateMetricEvent, MetricAggregationRow, MetricRepository, MetricRow, MetricType,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum MetricLogicError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("record already exists: {0}")]
    RecordAlreadyExists(String),
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct TrackMetricInput {
    pub metric_key: String,
    pub feature_key: Option<String>,
    pub environment_id: Option<Uuid>,
    pub user_context: String,
    pub variant: Option<String>,
    pub value: f64,
    pub metadata: Option<Value>,
    pub timestamp: Option<DateTime<Utc>>,
}

#[async_trait::async_trait]
pub trait MetricLogic: Send + Sync {
    async fn create_metric(
        &self,
        team_id: Uuid,
        key: String,
        name: String,
        description: Option<String>,
        metric_type: MetricType,
        unit: Option<String>,
        success_criteria: Option<Value>,
    ) -> Result<MetricRow, MetricLogicError>;

    async fn track_metrics(
        &self,
        client_id: &str,
        client_secret: &str,
        events: Vec<TrackMetricInput>,
    ) -> Result<usize, MetricLogicError>;

    async fn aggregate_metrics(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: &str,
    ) -> Result<u64, MetricLogicError>;

    async fn get_metric_results(
        &self,
        feature_key: &str,
        environment_id: Option<Uuid>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<MetricAggregationRow>, MetricLogicError>;

    async fn list_metrics(&self, team_id: Uuid) -> Result<Vec<MetricRow>, MetricLogicError>;

    fn clone_box(&self) -> Box<dyn MetricLogic>;
}

impl Clone for Box<dyn MetricLogic> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub fn metric_logic(
    metric_repo: Box<dyn MetricRepository>,
    client_repo: Box<dyn ClientRepository>,
) -> Box<dyn MetricLogic> {
    Box::new(MetricLogicImpl {
        metric_repo,
        client_repo,
    })
}

struct MetricLogicImpl {
    metric_repo: Box<dyn MetricRepository>,
    client_repo: Box<dyn ClientRepository>,
}

impl Clone for MetricLogicImpl {
    fn clone(&self) -> Self {
        Self {
            metric_repo: self.metric_repo.clone_box(),
            client_repo: self.client_repo.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl MetricLogic for MetricLogicImpl {
    fn clone_box(&self) -> Box<dyn MetricLogic> {
        Box::new(self.clone())
    }

    async fn create_metric(
        &self,
        team_id: Uuid,
        key: String,
        name: String,
        description: Option<String>,
        metric_type: MetricType,
        unit: Option<String>,
        success_criteria: Option<Value>,
    ) -> Result<MetricRow, MetricLogicError> {
        if key.trim().is_empty() {
            return Err(MetricLogicError::InvalidInput(
                "metric key cannot be empty".into(),
            ));
        }
        if name.trim().is_empty() {
            return Err(MetricLogicError::InvalidInput(
                "metric name cannot be empty".into(),
            ));
        }

        if let Some(existing) = self
            .metric_repo
            .get_metric_by_key(team_id, key.trim())
            .await
            .map_err(MetricLogicError::Database)?
        {
            return Err(MetricLogicError::RecordAlreadyExists(existing.key));
        }

        self.metric_repo
            .create_metric(CreateMetric {
                team_id,
                key: key.trim().to_string(),
                name: name.trim().to_string(),
                description,
                metric_type,
                unit: unit.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
                success_criteria,
            })
            .await
            .map_err(MetricLogicError::Database)
    }

    async fn track_metrics(
        &self,
        client_id: &str,
        client_secret: &str,
        events: Vec<TrackMetricInput>,
    ) -> Result<usize, MetricLogicError> {
        if client_id.is_empty() {
            return Err(MetricLogicError::InvalidInput(
                "client_id is required".into(),
            ));
        }
        if client_secret.is_empty() {
            return Err(MetricLogicError::InvalidInput(
                "client_secret is required".into(),
            ));
        }
        if events.is_empty() {
            return Ok(0);
        }

        let client_uuid = Uuid::parse_str(client_id)
            .map_err(|_| MetricLogicError::InvalidInput("invalid client_id".into()))?;
        let client = self
            .client_repo
            .get_client_by_id(client_uuid)
            .await
            .map_err(|e| MetricLogicError::InvalidInput(e.to_string()))?;

        if !client.enabled {
            return Err(MetricLogicError::PermissionDenied(
                "client is disabled".into(),
            ));
        }
        if client.api_key != client_secret {
            return Err(MetricLogicError::Unauthenticated(
                "client_secret mismatch".into(),
            ));
        }

        let team_id = client.team_id;
        let mut metric_cache: HashMap<String, MetricRow> = HashMap::new();
        let mut to_store = Vec::with_capacity(events.len());

        for event in events {
            let metric_key = event.metric_key.trim();
            if metric_key.is_empty() {
                return Err(MetricLogicError::InvalidInput(
                    "metric_key cannot be empty".into(),
                ));
            }
            if event.user_context.trim().is_empty() {
                return Err(MetricLogicError::InvalidInput(
                    "user_context is required".into(),
                ));
            }

            let metric = if let Some(m) = metric_cache.get(metric_key) {
                m.clone()
            } else {
                let fetched = self
                    .metric_repo
                    .get_metric_by_key(team_id, metric_key)
                    .await
                    .map_err(MetricLogicError::Database)?;
                let metric = fetched.ok_or_else(|| {
                    MetricLogicError::NotFound(format!("metric {} not found", metric_key))
                })?;
                metric_cache.insert(metric_key.to_string(), metric.clone());
                metric
            };

            let mut value = event.value;
            if metric.metric_type == MetricType::Conversion {
                if value < 0.0 {
                    return Err(MetricLogicError::InvalidInput(
                        "conversion metric value cannot be negative".into(),
                    ));
                }
                if value > 1.0 {
                    // Treat any positive conversion payload as a single conversion
                    value = 1.0;
                }
            }

            let occurred_at = event.timestamp.unwrap_or_else(Utc::now);
            let feature_key = event
                .feature_key
                .filter(|f| !f.trim().is_empty())
                .map(|f| f.trim().to_string());
            let variant = event
                .variant
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().to_string());

            let is_conversion = metric.metric_type == MetricType::Conversion && value > 0.0;

            to_store.push(CreateMetricEvent {
                metric_id: metric.id,
                feature_key,
                environment_id: event.environment_id,
                user_context: event.user_context,
                variant,
                value,
                metadata: event.metadata,
                occurred_at,
                is_conversion,
            });
        }

        let stored = self
            .metric_repo
            .insert_metric_events(to_store)
            .await
            .map_err(MetricLogicError::Database)?;
        Ok(stored.len())
    }

    async fn aggregate_metrics(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        bucket: &str,
    ) -> Result<u64, MetricLogicError> {
        if bucket != "hour" && bucket != "day" {
            return Err(MetricLogicError::InvalidInput(
                "bucket must be 'hour' or 'day'".into(),
            ));
        }

        self.metric_repo
            .upsert_aggregations(from, to, bucket)
            .await
            .map_err(MetricLogicError::Database)
    }

    async fn get_metric_results(
        &self,
        feature_key: &str,
        environment_id: Option<Uuid>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<MetricAggregationRow>, MetricLogicError> {
        if feature_key.trim().is_empty() {
            return Err(MetricLogicError::InvalidInput(
                "feature_key is required".into(),
            ));
        }

        self.metric_repo
            .get_metric_results(feature_key, environment_id, from, to)
            .await
            .map_err(MetricLogicError::Database)
    }

    async fn list_metrics(&self, team_id: Uuid) -> Result<Vec<MetricRow>, MetricLogicError> {
        self.metric_repo
            .list_metrics(team_id)
            .await
            .map_err(MetricLogicError::Database)
    }
}
