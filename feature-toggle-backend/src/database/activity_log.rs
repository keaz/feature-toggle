use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

/// Represents an activity log entry in the database
#[derive(sqlx::FromRow, Debug, Clone, Serialize, Deserialize)]
pub struct ActivityLogRow {
    pub id: Uuid,
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Input for creating a new activity log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateActivityLog {
    pub activity_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
}

/// Filter options for querying activity logs
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityLogFilter {
    pub activity_types: Option<Vec<String>>,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub actor_id: Option<Uuid>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
    /// Optional team filter (best-effort; enforced at the API/logic layer where entity lookups are available)
    pub team_id: Option<Uuid>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait ActivityLogRepository: Send + Sync {
    /// Create a new activity log entry
    async fn create_activity(
        &self,
        activity: CreateActivityLog,
    ) -> Result<ActivityLogRow, sqlx::Error>;

    /// Get activity logs with optional filtering
    async fn get_activities(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<Vec<ActivityLogRow>, sqlx::Error>;

    /// Get paginated activity logs with filtering
    async fn get_activities_paginated(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<(Vec<ActivityLogRow>, i64), sqlx::Error>;

    /// Get count of activities matching the filter
    async fn get_activity_count(&self, filter: ActivityLogFilter) -> Result<i64, sqlx::Error>;

    /// Create a new activity log entry within an existing transaction
    async fn create_activity_tx(
        &self,
        conn: &mut PgConnection,
        activity: CreateActivityLog,
    ) -> Result<ActivityLogRow, sqlx::Error>;

    fn clone_box(&self) -> Box<dyn ActivityLogRepository>;
}

impl Clone for Box<dyn ActivityLogRepository> {
    fn clone(&self) -> Box<dyn ActivityLogRepository> {
        self.clone_box()
    }
}

#[derive(Clone)]
pub struct PgActivityLogRepository {
    pool: sqlx::PgPool,
}

impl PgActivityLogRepository {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    async fn create_activity_internal(
        conn: &mut PgConnection,
        activity: CreateActivityLog,
    ) -> Result<ActivityLogRow, sqlx::Error> {
        let row = sqlx::query_as::<_, ActivityLogRow>(
            r#"
            INSERT INTO activity_log (
                activity_type, entity_type, entity_id, 
                actor_id, actor_name, description, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(&activity.activity_type)
        .bind(&activity.entity_type)
        .bind(&activity.entity_id)
        .bind(activity.actor_id)
        .bind(&activity.actor_name)
        .bind(&activity.description)
        .bind(&activity.metadata)
        .fetch_one(&mut *conn)
        .await?;

        Ok(row)
    }
}

#[async_trait::async_trait]
impl ActivityLogRepository for PgActivityLogRepository {
    async fn create_activity(
        &self,
        activity: CreateActivityLog,
    ) -> Result<ActivityLogRow, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        Self::create_activity_internal(&mut conn, activity).await
    }

    async fn create_activity_tx(
        &self,
        conn: &mut PgConnection,
        activity: CreateActivityLog,
    ) -> Result<ActivityLogRow, sqlx::Error> {
        Self::create_activity_internal(conn, activity).await
    }

    async fn get_activities(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<Vec<ActivityLogRow>, sqlx::Error> {
        let mut query = String::from("SELECT * FROM activity_log WHERE 1=1");
        let mut param_count = 0;

        // Build dynamic WHERE clause
        if let Some(ref activity_types) = filter.activity_types
            && !activity_types.is_empty()
        {
            param_count += 1;
            query.push_str(&format!(" AND activity_type = ANY(${})", param_count));
        }
        if filter.entity_type.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND entity_type = ${}", param_count));
        }
        if filter.entity_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND entity_id = ${}", param_count));
        }
        if filter.actor_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND actor_id = ${}", param_count));
        }
        if filter.from_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND created_at >= ${}", param_count));
        }
        if filter.to_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND created_at <= ${}", param_count));
        }

        query.push_str(" ORDER BY created_at DESC");

        if let Some(_limit) = filter.limit {
            param_count += 1;
            query.push_str(&format!(" LIMIT ${}", param_count));
        }
        if let Some(_offset) = filter.offset {
            param_count += 1;
            query.push_str(&format!(" OFFSET ${}", param_count));
        }

        let mut sql_query = sqlx::query_as::<_, ActivityLogRow>(&query);

        // Bind parameters in the same order
        if let Some(ref activity_types) = filter.activity_types
            && !activity_types.is_empty()
        {
            sql_query = sql_query.bind(activity_types);
        }
        if let Some(entity_type) = filter.entity_type {
            sql_query = sql_query.bind(entity_type);
        }
        if let Some(entity_id) = filter.entity_id {
            sql_query = sql_query.bind(entity_id);
        }
        if let Some(actor_id) = filter.actor_id {
            sql_query = sql_query.bind(actor_id);
        }
        if let Some(from_date) = filter.from_date {
            sql_query = sql_query.bind(from_date);
        }
        if let Some(to_date) = filter.to_date {
            sql_query = sql_query.bind(to_date);
        }
        if let Some(limit) = filter.limit {
            sql_query = sql_query.bind(limit);
        }
        if let Some(offset) = filter.offset {
            sql_query = sql_query.bind(offset);
        }

        let rows = sql_query.fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn get_activities_paginated(
        &self,
        filter: ActivityLogFilter,
    ) -> Result<(Vec<ActivityLogRow>, i64), sqlx::Error> {
        // Get total count
        let total = self.get_activity_count(filter.clone()).await?;

        // Get paginated results
        let activities = self.get_activities(filter).await?;

        Ok((activities, total))
    }

    async fn get_activity_count(&self, filter: ActivityLogFilter) -> Result<i64, sqlx::Error> {
        let mut query = String::from("SELECT COUNT(*) as count FROM activity_log WHERE 1=1");
        let mut param_count = 0;

        // Build dynamic WHERE clause (same as get_activities but without pagination)
        if let Some(ref activity_types) = filter.activity_types
            && !activity_types.is_empty()
        {
            param_count += 1;
            query.push_str(&format!(" AND activity_type = ANY(${})", param_count));
        }
        if filter.entity_type.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND entity_type = ${}", param_count));
        }
        if filter.entity_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND entity_id = ${}", param_count));
        }
        if filter.actor_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND actor_id = ${}", param_count));
        }
        if filter.from_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND created_at >= ${}", param_count));
        }
        if filter.to_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND created_at <= ${}", param_count));
        }

        let mut sql_query = sqlx::query_scalar::<_, i64>(&query);

        // Bind parameters in the same order
        if let Some(ref activity_types) = filter.activity_types
            && !activity_types.is_empty()
        {
            sql_query = sql_query.bind(activity_types);
        }
        if let Some(entity_type) = filter.entity_type {
            sql_query = sql_query.bind(entity_type);
        }
        if let Some(entity_id) = filter.entity_id {
            sql_query = sql_query.bind(entity_id);
        }
        if let Some(actor_id) = filter.actor_id {
            sql_query = sql_query.bind(actor_id);
        }
        if let Some(from_date) = filter.from_date {
            sql_query = sql_query.bind(from_date);
        }
        if let Some(to_date) = filter.to_date {
            sql_query = sql_query.bind(to_date);
        }

        let count = sql_query.fetch_one(&self.pool).await?;
        Ok(count)
    }

    fn clone_box(&self) -> Box<dyn ActivityLogRepository> {
        Box::new(self.clone())
    }
}

pub fn activity_log_repository(pool: sqlx::PgPool) -> Box<dyn ActivityLogRepository> {
    Box::new(PgActivityLogRepository::new(pool))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_create_activity() -> CreateActivityLog {
        CreateActivityLog {
            activity_type: "feature_created".to_string(),
            entity_type: "feature".to_string(),
            entity_id: "test-feature-id".to_string(),
            actor_id: Some(Uuid::new_v4()),
            actor_name: Some("John Doe".to_string()),
            description: "Created feature 'new-checkout'".to_string(),
            metadata: Some(serde_json::json!({"feature_key": "new-checkout"})),
        }
    }

    #[test]
    fn test_create_activity_struct() {
        let activity = sample_create_activity();

        assert_eq!(activity.activity_type, "feature_created");
        assert_eq!(activity.entity_type, "feature");
        assert_eq!(activity.entity_id, "test-feature-id");
        assert!(activity.actor_id.is_some());
        assert_eq!(activity.actor_name, Some("John Doe".to_string()));
        assert!(activity.metadata.is_some());
    }

    #[test]
    fn test_activity_log_filter_default() {
        let filter = ActivityLogFilter::default();

        assert!(filter.activity_types.is_none());
        assert!(filter.entity_type.is_none());
        assert!(filter.entity_id.is_none());
        assert!(filter.actor_id.is_none());
        assert!(filter.from_date.is_none());
        assert!(filter.to_date.is_none());
        assert!(filter.team_id.is_none());
    }

    #[test]
    fn test_activity_log_filter_with_values() {
        let filter = ActivityLogFilter {
            activity_types: Some(vec![
                "feature_created".to_string(),
                "feature_updated".to_string(),
            ]),
            entity_type: Some("feature".to_string()),
            entity_id: Some("123".to_string()),
            actor_id: Some(Uuid::new_v4()),
            from_date: Some(Utc::now() - chrono::Duration::days(7)),
            to_date: Some(Utc::now()),
            limit: Some(20),
            offset: Some(0),
            team_id: None,
        };

        assert_eq!(
            filter.activity_types,
            Some(vec![
                "feature_created".to_string(),
                "feature_updated".to_string()
            ])
        );
        assert_eq!(filter.limit, Some(20));
        assert_eq!(filter.offset, Some(0));
    }

    #[test]
    fn test_repository_creation() {
        use sqlx::PgPool;

        fn _verify_signature(_pool: PgPool) -> Box<dyn ActivityLogRepository> {
            activity_log_repository(_pool)
        }

        assert!(true);
    }
}
