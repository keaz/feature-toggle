use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Clone, Serialize, Deserialize)]
pub struct FeatureEvaluationRow {
    pub id: Uuid,
    pub feature_key: String,
    pub environment_id: String,
    pub client_id: Uuid,
    pub evaluated_at: DateTime<Utc>,
    pub evaluation_result: bool,
    pub evaluation_context: Option<serde_json::Value>,
    pub user_context: Option<String>,
    pub prior_assignment: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeatureEvaluation {
    pub feature_key: String,
    pub environment_id: String,
    pub client_id: Uuid,
    pub evaluated_at: DateTime<Utc>,
    pub evaluation_result: bool,
    pub evaluation_context: Option<serde_json::Value>,
    pub user_context: Option<String>,
    pub prior_assignment: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureEvaluationFilter {
    pub feature_key: Option<String>,
    pub environment_id: Option<String>,
    pub client_id: Option<Uuid>,
    pub user_context: Option<String>,
    pub prior_assignment: Option<bool>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait FeatureEvaluationRepository: Send + Sync {
    async fn create_evaluation(
        &self,
        evaluation: CreateFeatureEvaluation,
    ) -> Result<FeatureEvaluationRow, sqlx::Error>;

    async fn bulk_create_evaluations(
        &self,
        evaluations: Vec<CreateFeatureEvaluation>,
    ) -> Result<Vec<FeatureEvaluationRow>, sqlx::Error>;

    async fn get_evaluations(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<Vec<FeatureEvaluationRow>, sqlx::Error>;

    async fn get_evaluation_count(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<i64, sqlx::Error>;

    /// Get feature evaluation rates aggregated by time intervals
    /// Returns evaluation counts grouped by time buckets for dashboard visualization
    async fn get_evaluation_rates(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<uuid::Uuid>,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
        interval_minutes: i32,
    ) -> Result<Vec<EvaluationRatePoint>, sqlx::Error>;

    /// Get feature evaluation summary statistics for the dashboard
    /// Returns aggregated metrics like total evaluations, success rate, etc.
    async fn get_evaluation_summary(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<uuid::Uuid>,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<EvaluationSummary, sqlx::Error>;

    /// Get aggregated evaluation data grouped by feature key
    /// Returns evaluation statistics for the most evaluated features
    async fn get_evaluations_by_feature(
        &self,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
        environment_id: Option<String>,
        client_id: Option<uuid::Uuid>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<EvaluationByFeature>, sqlx::Error>;

    fn clone_box(&self) -> Box<dyn FeatureEvaluationRepository>;
}

/// Represents a single point in the evaluation rate time series
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EvaluationRatePoint {
    /// The timestamp bucket (rounded to interval)
    pub time_bucket: chrono::DateTime<chrono::Utc>,
    /// Number of evaluations in this time bucket
    pub evaluation_count: i64,
    /// Number of evaluations that resulted in true
    pub success_count: i64,
    /// Number of evaluations that were from prior assignments (cached)
    pub prior_assignment_count: i64,
}

/// Intermediate struct for the summary query result (without top_feature_key)
#[derive(sqlx::FromRow)]
struct EvaluationSummaryPartial {
    total_evaluations: i64,
    successful_evaluations: i64,
    cached_evaluations: i64,
    unique_users: i64,
    success_rate: f64,
    cache_hit_rate: f64,
}

/// Summary statistics for feature evaluations over a time period
#[derive(Debug, Clone)]
pub struct EvaluationSummary {
    /// Total number of evaluations
    pub total_evaluations: i64,
    /// Number of evaluations that resulted in true
    pub successful_evaluations: i64,
    /// Number of evaluations from prior assignments (cached)
    pub cached_evaluations: i64,
    /// Number of unique users who had evaluations
    pub unique_users: i64,
    /// Most frequently evaluated feature key
    pub top_feature_key: Option<String>,
    /// Success rate as percentage (0-100)
    pub success_rate: f64,
    /// Cache hit rate as percentage (0-100)  
    pub cache_hit_rate: f64,
}

/// Aggregated evaluation data grouped by feature key
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EvaluationByFeature {
    /// Feature key
    pub feature_key: String,
    /// Total number of evaluations for this feature
    pub total_evaluations: i64,
    /// Number of evaluations that resulted in true
    pub successful_evaluations: i64,
    /// Number of evaluations from prior assignments (cached)
    pub cached_evaluations: i64,
    /// Number of unique users who had evaluations for this feature
    pub unique_users: i64,
    /// Timestamp of the last evaluation for this feature
    pub last_evaluated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
pub struct PgFeatureEvaluationRepository {
    pool: sqlx::PgPool,
}

impl PgFeatureEvaluationRepository {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl FeatureEvaluationRepository for PgFeatureEvaluationRepository {
    async fn create_evaluation(
        &self,
        evaluation: CreateFeatureEvaluation,
    ) -> Result<FeatureEvaluationRow, sqlx::Error> {
        let row = sqlx::query_as::<_, FeatureEvaluationRow>(
            r#"
            INSERT INTO feature_evaluations (
                feature_key, environment_id, client_id, evaluated_at, 
                evaluation_result, evaluation_context, user_context, prior_assignment
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(evaluation.feature_key)
        .bind(evaluation.environment_id)
        .bind(evaluation.client_id)
        .bind(evaluation.evaluated_at)
        .bind(evaluation.evaluation_result)
        .bind(evaluation.evaluation_context)
        .bind(evaluation.user_context)
        .bind(evaluation.prior_assignment)
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }

    async fn bulk_create_evaluations(
        &self,
        evaluations: Vec<CreateFeatureEvaluation>,
    ) -> Result<Vec<FeatureEvaluationRow>, sqlx::Error> {
        if evaluations.is_empty() {
            return Ok(vec![]);
        }

        let mut query = String::from(
            r#"
            INSERT INTO feature_evaluations (
                feature_key, environment_id, client_id, evaluated_at, 
                evaluation_result, evaluation_context, user_context, prior_assignment
            )
            VALUES
            "#,
        );

        // Build the VALUES clause dynamically
        for (i, _) in evaluations.iter().enumerate() {
            if i > 0 {
                query.push_str(", ");
            }
            let base = i * 8;
            query.push_str(&format!(
                "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
                base + 1,
                base + 2,
                base + 3,
                base + 4,
                base + 5,
                base + 6,
                base + 7,
                base + 8
            ));
        }
        query.push_str(" RETURNING *");

        let mut sql_query = sqlx::query_as::<_, FeatureEvaluationRow>(&query);

        // Bind all parameters
        for evaluation in evaluations {
            sql_query = sql_query
                .bind(evaluation.feature_key)
                .bind(evaluation.environment_id)
                .bind(evaluation.client_id)
                .bind(evaluation.evaluated_at)
                .bind(evaluation.evaluation_result)
                .bind(evaluation.evaluation_context)
                .bind(evaluation.user_context)
                .bind(evaluation.prior_assignment);
        }

        let rows = sql_query.fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn get_evaluations(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<Vec<FeatureEvaluationRow>, sqlx::Error> {
        let mut query = String::from("SELECT * FROM feature_evaluations WHERE 1=1");
        let mut param_count = 0;

        // Build dynamic WHERE clause
        if filter.feature_key.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND feature_key = ${}", param_count));
        }
        if filter.environment_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND environment_id = ${}", param_count));
        }
        if filter.client_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND client_id = ${}", param_count));
        }
        if filter.user_context.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND user_context = ${}", param_count));
        }
        if filter.prior_assignment.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND prior_assignment = ${}", param_count));
        }
        if filter.from_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND evaluated_at >= ${}", param_count));
        }
        if filter.to_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND evaluated_at <= ${}", param_count));
        }

        query.push_str(" ORDER BY evaluated_at DESC");

        if let Some(_limit) = filter.limit {
            param_count += 1;
            query.push_str(&format!(" LIMIT ${}", param_count));
        }
        if let Some(_offset) = filter.offset {
            param_count += 1;
            query.push_str(&format!(" OFFSET ${}", param_count));
        }

        let mut sql_query = sqlx::query_as::<_, FeatureEvaluationRow>(&query);

        // Bind parameters in the same order
        if let Some(feature_key) = filter.feature_key {
            sql_query = sql_query.bind(feature_key);
        }
        if let Some(environment_id) = filter.environment_id {
            sql_query = sql_query.bind(environment_id);
        }
        if let Some(client_id) = filter.client_id {
            sql_query = sql_query.bind(client_id);
        }
        if let Some(user_context) = filter.user_context {
            sql_query = sql_query.bind(user_context);
        }
        if let Some(prior_assignment) = filter.prior_assignment {
            sql_query = sql_query.bind(prior_assignment);
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

    async fn get_evaluation_count(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<i64, sqlx::Error> {
        let mut query = String::from("SELECT COUNT(*) as count FROM feature_evaluations WHERE 1=1");
        let mut param_count = 0;

        // Build dynamic WHERE clause (same as get_evaluations)
        if filter.feature_key.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND feature_key = ${}", param_count));
        }
        if filter.environment_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND environment_id = ${}", param_count));
        }
        if filter.client_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND client_id = ${}", param_count));
        }
        if filter.user_context.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND user_context = ${}", param_count));
        }
        if filter.prior_assignment.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND prior_assignment = ${}", param_count));
        }
        if filter.from_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND evaluated_at >= ${}", param_count));
        }
        if filter.to_date.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND evaluated_at <= ${}", param_count));
        }

        let mut sql_query = sqlx::query_scalar::<_, i64>(&query);

        // Bind parameters in the same order
        if let Some(feature_key) = filter.feature_key {
            sql_query = sql_query.bind(feature_key);
        }
        if let Some(environment_id) = filter.environment_id {
            sql_query = sql_query.bind(environment_id);
        }
        if let Some(client_id) = filter.client_id {
            sql_query = sql_query.bind(client_id);
        }
        if let Some(user_context) = filter.user_context {
            sql_query = sql_query.bind(user_context);
        }
        if let Some(prior_assignment) = filter.prior_assignment {
            sql_query = sql_query.bind(prior_assignment);
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

    /// Get feature evaluation rates aggregated by time intervals
    /// Uses PostgreSQL's date_trunc function to group evaluations into time buckets
    async fn get_evaluation_rates(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<uuid::Uuid>,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
        interval_minutes: i32,
    ) -> Result<Vec<EvaluationRatePoint>, sqlx::Error> {
        let mut query = format!(
            r#"
            SELECT 
                date_trunc('minute', evaluated_at) + 
                INTERVAL '{} minutes' * floor(extract(minute from evaluated_at) / {}) as time_bucket,
                COUNT(*) as evaluation_count,
                COUNT(*) FILTER (WHERE evaluation_result = true) as success_count,
                COUNT(*) FILTER (WHERE prior_assignment = true) as prior_assignment_count
            FROM feature_evaluations 
            WHERE evaluated_at >= $1 AND evaluated_at <= $2
            "#,
            interval_minutes, interval_minutes
        );

        let mut param_count = 2;

        // Add optional filters
        if feature_key.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND feature_key = ${}", param_count));
        }
        if environment_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND environment_id = ${}", param_count));
        }
        if client_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND client_id = ${}", param_count));
        }

        query.push_str(" GROUP BY time_bucket ORDER BY time_bucket");

        let mut sql_query = sqlx::query_as::<_, EvaluationRatePoint>(&query)
            .bind(from_time)
            .bind(to_time);

        // Bind optional parameters
        if let Some(key) = feature_key {
            sql_query = sql_query.bind(key);
        }
        if let Some(env_id) = environment_id {
            sql_query = sql_query.bind(env_id);
        }
        if let Some(c_id) = client_id {
            sql_query = sql_query.bind(c_id);
        }

        let rates = sql_query.fetch_all(&self.pool).await?;
        Ok(rates)
    }

    /// Get comprehensive evaluation summary statistics
    /// Provides aggregated metrics useful for dashboard overview
    async fn get_evaluation_summary(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<uuid::Uuid>,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<EvaluationSummary, sqlx::Error> {
        let mut query = String::from(
            r#"
            SELECT 
                COUNT(*) as total_evaluations,
                COUNT(*) FILTER (WHERE evaluation_result = true) as successful_evaluations,
                COUNT(*) FILTER (WHERE prior_assignment = true) as cached_evaluations,
                COUNT(DISTINCT user_context) FILTER (WHERE user_context IS NOT NULL) as unique_users,
                CASE 
                    WHEN COUNT(*) > 0 THEN (COUNT(*) FILTER (WHERE evaluation_result = true) * 100.0 / COUNT(*))::FLOAT8
                    ELSE 0.0::FLOAT8
                END as success_rate,
                CASE 
                    WHEN COUNT(*) > 0 THEN (COUNT(*) FILTER (WHERE prior_assignment = true) * 100.0 / COUNT(*))::FLOAT8
                    ELSE 0.0::FLOAT8
                END as cache_hit_rate
            FROM feature_evaluations 
            WHERE evaluated_at >= $1 AND evaluated_at <= $2
            "#,
        );

        let mut param_count = 2;

        // Add optional filters
        if feature_key.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND feature_key = ${}", param_count));
        }
        if environment_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND environment_id = ${}", param_count));
        }
        if client_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND client_id = ${}", param_count));
        }

        let mut sql_query = sqlx::query_as::<_, EvaluationSummaryPartial>(&query)
            .bind(from_time)
            .bind(to_time);

        // Bind optional parameters
        if let Some(ref key) = feature_key {
            sql_query = sql_query.bind(key);
        }
        if let Some(ref env_id) = environment_id {
            sql_query = sql_query.bind(env_id);
        }
        if let Some(c_id) = client_id {
            sql_query = sql_query.bind(c_id);
        }

        let partial_summary = sql_query.fetch_one(&self.pool).await?;

        // Get the most frequently evaluated feature key in a separate query
        let mut top_feature_query = String::from(
            r#"
            SELECT feature_key, COUNT(*) as count
            FROM feature_evaluations 
            WHERE evaluated_at >= $1 AND evaluated_at <= $2
            "#,
        );

        let mut param_count = 2;
        if feature_key.is_some() {
            param_count += 1;
            top_feature_query.push_str(&format!(" AND feature_key = ${}", param_count));
        }
        if environment_id.is_some() {
            param_count += 1;
            top_feature_query.push_str(&format!(" AND environment_id = ${}", param_count));
        }
        if client_id.is_some() {
            param_count += 1;
            top_feature_query.push_str(&format!(" AND client_id = ${}", param_count));
        }

        top_feature_query.push_str(" GROUP BY feature_key ORDER BY count DESC LIMIT 1");

        let mut top_feature_sql = sqlx::query_scalar::<_, String>(&top_feature_query)
            .bind(from_time)
            .bind(to_time);

        if let Some(ref key) = feature_key {
            top_feature_sql = top_feature_sql.bind(key);
        }
        if let Some(ref env_id) = environment_id {
            top_feature_sql = top_feature_sql.bind(env_id);
        }
        if let Some(c_id) = client_id {
            top_feature_sql = top_feature_sql.bind(c_id);
        }

        let top_feature_key = top_feature_sql.fetch_optional(&self.pool).await?;

        // Construct the complete EvaluationSummary from the partial result and top_feature_key
        let summary = EvaluationSummary {
            total_evaluations: partial_summary.total_evaluations,
            successful_evaluations: partial_summary.successful_evaluations,
            cached_evaluations: partial_summary.cached_evaluations,
            unique_users: partial_summary.unique_users,
            top_feature_key,
            success_rate: partial_summary.success_rate,
            cache_hit_rate: partial_summary.cache_hit_rate,
        };

        Ok(summary)
    }

    /// Get aggregated evaluation data grouped by feature key
    /// Provides evaluation statistics for each feature in descending order by total evaluations
    async fn get_evaluations_by_feature(
        &self,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
        environment_id: Option<String>,
        client_id: Option<uuid::Uuid>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<EvaluationByFeature>, sqlx::Error> {
        let mut query = String::from(
            r#"
            SELECT 
                feature_key,
                COUNT(*) as total_evaluations,
                COUNT(*) FILTER (WHERE evaluation_result = true) as successful_evaluations,
                COUNT(*) FILTER (WHERE prior_assignment = true) as cached_evaluations,
                COUNT(DISTINCT user_context) FILTER (WHERE user_context IS NOT NULL) as unique_users,
                MAX(evaluated_at) as last_evaluated_at
            FROM feature_evaluations 
            WHERE evaluated_at >= $1 AND evaluated_at <= $2
            "#,
        );

        let mut param_count = 2;

        // Add optional filters
        if environment_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND environment_id = ${}", param_count));
        }
        if client_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND client_id = ${}", param_count));
        }

        query.push_str(" GROUP BY feature_key ORDER BY total_evaluations DESC");

        // Add pagination
        if limit.is_some() {
            param_count += 1;
            query.push_str(&format!(" LIMIT ${}", param_count));
        }
        if offset.is_some() {
            param_count += 1;
            query.push_str(&format!(" OFFSET ${}", param_count));
        }

        let mut sql_query = sqlx::query_as::<_, EvaluationByFeature>(&query)
            .bind(from_time)
            .bind(to_time);

        // Bind optional parameters
        if let Some(env_id) = environment_id {
            sql_query = sql_query.bind(env_id);
        }
        if let Some(c_id) = client_id {
            sql_query = sql_query.bind(c_id);
        }
        if let Some(lim) = limit {
            sql_query = sql_query.bind(lim);
        }
        if let Some(off) = offset {
            sql_query = sql_query.bind(off);
        }

        let results = sql_query.fetch_all(&self.pool).await?;
        Ok(results)
    }

    fn clone_box(&self) -> Box<dyn FeatureEvaluationRepository> {
        Box::new(self.clone())
    }
}

pub fn feature_evaluation_repository(pool: sqlx::PgPool) -> Box<dyn FeatureEvaluationRepository> {
    Box::new(PgFeatureEvaluationRepository::new(pool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn sample_create_evaluation() -> CreateFeatureEvaluation {
        CreateFeatureEvaluation {
            feature_key: "test-feature".to_string(),
            environment_id: "env-123".to_string(),
            client_id: Uuid::new_v4(),
            evaluated_at: Utc::now(),
            evaluation_result: true,
            evaluation_context: Some(json!({"user": "test-user"})),
            user_context: Some("user123".to_string()),
            prior_assignment: false,
        }
    }

    #[test]
    fn test_feature_evaluation_filter_creation() {
        let filter = FeatureEvaluationFilter {
            feature_key: Some("test-feature".to_string()),
            environment_id: Some("env-123".to_string()),
            client_id: Some(Uuid::new_v4()),
            user_context: Some("user123".to_string()),
            prior_assignment: Some(false),
            from_date: Some(Utc::now() - chrono::Duration::hours(1)),
            to_date: Some(Utc::now()),
            limit: Some(10),
            offset: Some(0),
        };

        assert_eq!(filter.feature_key, Some("test-feature".to_string()));
        assert_eq!(filter.environment_id, Some("env-123".to_string()));
        assert!(filter.client_id.is_some());
        assert_eq!(filter.limit, Some(10));
        assert_eq!(filter.offset, Some(0));
    }

    #[test]
    fn test_create_evaluation_struct() {
        let evaluation = sample_create_evaluation();

        assert_eq!(evaluation.feature_key, "test-feature");
        assert_eq!(evaluation.environment_id, "env-123");
        assert_eq!(evaluation.evaluation_result, true);
        assert_eq!(evaluation.prior_assignment, false);
        assert!(evaluation.evaluation_context.is_some());
        assert!(evaluation.user_context.is_some());
    }

    #[test]
    fn test_evaluation_summary_calculation() {
        let summary = EvaluationSummary {
            total_evaluations: 100,
            successful_evaluations: 80,
            cached_evaluations: 30,
            unique_users: 25,
            top_feature_key: Some("popular-feature".to_string()),
            success_rate: 80.0,
            cache_hit_rate: 30.0,
        };

        assert_eq!(summary.total_evaluations, 100);
        assert_eq!(summary.successful_evaluations, 80);
        assert_eq!(summary.success_rate, 80.0);
        assert_eq!(summary.cache_hit_rate, 30.0);
        assert_eq!(summary.top_feature_key, Some("popular-feature".to_string()));
    }

    #[test]
    fn test_evaluation_rate_point() {
        let rate_point = EvaluationRatePoint {
            time_bucket: Utc::now(),
            evaluation_count: 50,
            success_count: 40,
            prior_assignment_count: 15,
        };

        assert_eq!(rate_point.evaluation_count, 50);
        assert_eq!(rate_point.success_count, 40);
        assert_eq!(rate_point.prior_assignment_count, 15);
    }

    #[test]
    fn test_repository_creation() {
        // This test only verifies the factory function signature
        // In a real test environment, this would use a test database connection
        use sqlx::PgPool;

        // Just verify this compiles and is the correct function signature
        fn _verify_signature(_pool: PgPool) -> Box<dyn FeatureEvaluationRepository> {
            feature_evaluation_repository(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[test]
    fn test_pg_repository_creation() {
        // This test only verifies the constructor signature
        use sqlx::PgPool;

        // Just verify this compiles and is the correct function signature
        fn _verify_signature(_pool: PgPool) -> PgFeatureEvaluationRepository {
            PgFeatureEvaluationRepository::new(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }
}
