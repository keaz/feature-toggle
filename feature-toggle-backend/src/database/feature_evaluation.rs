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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureEvaluationFilter {
    pub feature_key: Option<String>,
    pub environment_id: Option<String>,
    pub client_id: Option<Uuid>,
    pub user_context: Option<String>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

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

    fn clone_box(&self) -> Box<dyn FeatureEvaluationRepository>;
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
                evaluation_result, evaluation_context, user_context
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
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
                evaluation_result, evaluation_context, user_context
            )
            VALUES
            "#,
        );

        // Build the VALUES clause dynamically
        for (i, _) in evaluations.iter().enumerate() {
            if i > 0 {
                query.push_str(", ");
            }
            let base = i * 7;
            query.push_str(&format!(
                "(${}, ${}, ${}, ${}, ${}, ${}, ${})",
                base + 1,
                base + 2,
                base + 3,
                base + 4,
                base + 5,
                base + 6,
                base + 7
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
                .bind(evaluation.user_context);
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
        if let Some(from_date) = filter.from_date {
            sql_query = sql_query.bind(from_date);
        }
        if let Some(to_date) = filter.to_date {
            sql_query = sql_query.bind(to_date);
        }

        let count = sql_query.fetch_one(&self.pool).await?;
        Ok(count)
    }

    fn clone_box(&self) -> Box<dyn FeatureEvaluationRepository> {
        Box::new(self.clone())
    }
}

pub fn feature_evaluation_repository(pool: sqlx::PgPool) -> Box<dyn FeatureEvaluationRepository> {
    Box::new(PgFeatureEvaluationRepository::new(pool))
}
