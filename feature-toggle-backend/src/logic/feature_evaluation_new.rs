use crate::database::feature_evaluation::{
    CreateFeatureEvaluation, FeatureEvaluationFilter, FeatureEvaluationRepository,
    FeatureEvaluationRow, EvaluationRatePoint, EvaluationSummary,
};
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum FeatureEvaluationLogicError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Not found")]
    NotFound,
}

#[async_trait::async_trait]
pub trait FeatureEvaluationLogic: Send + Sync {
    /// Record a single feature evaluation event
    async fn record_evaluation(
        &self,
        feature_key: String,
        environment_id: String,
        client_id: Uuid,
        evaluated_at: DateTime<Utc>,
        evaluation_result: bool,
        evaluation_context: Option<serde_json::Value>,
        user_context: Option<String>,
        prior_assignment: bool,
    ) -> Result<FeatureEvaluationRow, FeatureEvaluationLogicError>;

    /// Record multiple feature evaluation events in bulk
    async fn record_evaluations_bulk(
        &self,
        evaluations: Vec<CreateFeatureEvaluation>,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError>;

    /// Get feature evaluations with filtering
    async fn get_evaluations(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError>;

    /// Get count of feature evaluations with filtering
    async fn get_evaluation_count(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<i64, FeatureEvaluationLogicError>;

    /// Get feature evaluation rates for dashboard visualization
    /// Returns time-series data showing evaluation counts over time intervals
    async fn get_evaluation_rates(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<Uuid>,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
        interval_minutes: i32,
    ) -> Result<Vec<EvaluationRatePoint>, FeatureEvaluationLogicError>;

    /// Get evaluation summary statistics for dashboard overview
    /// Provides aggregated metrics like success rate, cache hit rate, etc.
    async fn get_evaluation_summary(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<Uuid>,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
    ) -> Result<EvaluationSummary, FeatureEvaluationLogicError>;

    fn clone_box(&self) -> Box<dyn FeatureEvaluationLogic>;
}

// Blanket implementation of Clone for Box<dyn FeatureEvaluationLogic>
impl Clone for Box<dyn FeatureEvaluationLogic> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Implementation of feature evaluation logic using the repository pattern
pub struct FeatureEvaluationLogicImpl {
    repository: Box<dyn FeatureEvaluationRepository>,
}

impl FeatureEvaluationLogicImpl {
    pub fn new(repository: Box<dyn FeatureEvaluationRepository>) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl FeatureEvaluationLogic for FeatureEvaluationLogicImpl {
    async fn record_evaluation(
        &self,
        feature_key: String,
        environment_id: String,
        client_id: Uuid,
        evaluated_at: DateTime<Utc>,
        evaluation_result: bool,
        evaluation_context: Option<serde_json::Value>,
        user_context: Option<String>,
        prior_assignment: bool,
    ) -> Result<FeatureEvaluationRow, FeatureEvaluationLogicError> {
        if feature_key.is_empty() {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "Feature key cannot be empty".to_string(),
            ));
        }

        if environment_id.is_empty() {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "Environment ID cannot be empty".to_string(),
            ));
        }

        let evaluation = CreateFeatureEvaluation {
            feature_key,
            environment_id,
            client_id,
            evaluated_at,
            evaluation_result,
            evaluation_context,
            user_context,
            prior_assignment,
        };

        let result = self.repository.create_evaluation(evaluation).await?;
        Ok(result)
    }

    async fn record_evaluations_bulk(
        &self,
        evaluations: Vec<CreateFeatureEvaluation>,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError> {
        if evaluations.is_empty() {
            return Ok(vec![]);
        }

        // Validate each evaluation
        for evaluation in &evaluations {
            if evaluation.feature_key.is_empty() {
                return Err(FeatureEvaluationLogicError::InvalidInput(
                    "Feature key cannot be empty".to_string(),
                ));
            }
            if evaluation.environment_id.is_empty() {
                return Err(FeatureEvaluationLogicError::InvalidInput(
                    "Environment ID cannot be empty".to_string(),
                ));
            }
        }

        let result = self.repository.bulk_create_evaluations(evaluations).await?;
        Ok(result)
    }

    async fn get_evaluations(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError> {
        let evaluations = self.repository.get_evaluations(filter).await?;
        Ok(evaluations)
    }

    async fn get_evaluation_count(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<i64, FeatureEvaluationLogicError> {
        let count = self.repository.get_evaluation_count(filter).await?;
        Ok(count)
    }

    /// Get evaluation rates with validation and business logic
    async fn get_evaluation_rates(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<Uuid>,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
        interval_minutes: i32,
    ) -> Result<Vec<EvaluationRatePoint>, FeatureEvaluationLogicError> {
        // Validate time range (max 24 hours)
        let duration = to_time - from_time;
        if duration.num_hours() > 24 {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "Time range cannot exceed 24 hours".to_string(),
            ));
        }

        // Validate from_time is not in the future
        if from_time > Utc::now() {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "From time cannot be in the future".to_string(),
            ));
        }

        // Validate interval (must be between 1 minute and 1 hour)
        if interval_minutes < 1 || interval_minutes > 60 {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "Interval must be between 1 and 60 minutes".to_string(),
            ));
        }

        let rates = self.repository.get_evaluation_rates(
            feature_key,
            environment_id,
            client_id,
            from_time,
            to_time,
            interval_minutes,
        ).await?;

        Ok(rates)
    }

    /// Get evaluation summary with business logic validation
    async fn get_evaluation_summary(
        &self,
        feature_key: Option<String>,
        environment_id: Option<String>,
        client_id: Option<Uuid>,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
    ) -> Result<EvaluationSummary, FeatureEvaluationLogicError> {
        // Validate time range (max 24 hours)
        let duration = to_time - from_time;
        if duration.num_hours() > 24 {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "Time range cannot exceed 24 hours".to_string(),
            ));
        }

        // Validate from_time is not in the future
        if from_time > Utc::now() {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "From time cannot be in the future".to_string(),
            ));
        }

        let summary = self.repository.get_evaluation_summary(
            feature_key,
            environment_id,
            client_id,
            from_time,
            to_time,
        ).await?;

        Ok(summary)
    }

    fn clone_box(&self) -> Box<dyn FeatureEvaluationLogic> {
        Box::new(FeatureEvaluationLogicImpl {
            repository: self.repository.clone_box(),
        })
    }
}

/// Factory function to create feature evaluation logic implementation
pub fn feature_evaluation_logic(
    repository: Box<dyn FeatureEvaluationRepository>,
) -> Box<dyn FeatureEvaluationLogic> {
    Box::new(FeatureEvaluationLogicImpl::new(repository))
}
