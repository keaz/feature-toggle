//! Transactional logic for metric management.

use crate::Error;
use crate::database::metrics::{CreateMetric, MetricRepositoryTx, MetricRow, MetricType};
use sqlx::PgConnection;
use uuid::Uuid;

pub async fn create_metric_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    team_id: Uuid,
    key: String,
    name: String,
    description: Option<String>,
    metric_type: MetricType,
    unit: Option<String>,
    success_criteria: Option<serde_json::Value>,
) -> Result<MetricRow, Error>
where
    R: MetricRepositoryTx,
{
    let key_trimmed = key.trim();
    if key_trimmed.is_empty() {
        return Err(Error::InvalidInput(
            "metric key cannot be empty".to_string(),
        ));
    }

    let name_trimmed = name.trim();
    if name_trimmed.is_empty() {
        return Err(Error::InvalidInput(
            "metric name cannot be empty".to_string(),
        ));
    }

    if let Some(existing) = repo
        .get_metric_by_key(team_id, key_trimmed)
        .await
        .map_err(Error::DatabaseError)?
    {
        return Err(Error::RecordAlreadyExists(existing.key));
    }

    let metric = CreateMetric {
        team_id,
        key: key_trimmed.to_string(),
        name: name_trimmed.to_string(),
        description,
        metric_type,
        unit: unit
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
        success_criteria,
    };

    repo.create_metric_tx(conn, metric)
        .await
        .map_err(Error::DatabaseError)
}
