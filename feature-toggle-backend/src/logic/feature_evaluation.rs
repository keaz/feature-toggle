use crate::database::feature_evaluation::{
    CreateFeatureEvaluation, FeatureEvaluationFilter, FeatureEvaluationRepository,
    FeatureEvaluationRow,
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

    async fn record_evaluations_bulk(
        &self,
        evaluations: Vec<CreateFeatureEvaluation>,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError>;

    async fn get_evaluations(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError>;

    async fn get_evaluation_count(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<i64, FeatureEvaluationLogicError>;

    fn clone_box(&self) -> Box<dyn FeatureEvaluationLogic>;
}

// Blanket implementation of Clone for Box<dyn FeatureEvaluationLogic>
impl Clone for Box<dyn FeatureEvaluationLogic> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

pub struct FeatureEvaluationLogicImpl {
    repository: Box<dyn FeatureEvaluationRepository>,
}

impl Clone for FeatureEvaluationLogicImpl {
    fn clone(&self) -> Self {
        Self {
            repository: self.repository.clone_box(),
        }
    }
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

        // Validate all evaluations
        for eval in &evaluations {
            if eval.feature_key.is_empty() {
                return Err(FeatureEvaluationLogicError::InvalidInput(
                    "Feature key cannot be empty".to_string(),
                ));
            }
            if eval.environment_id.is_empty() {
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
        let result = self.repository.get_evaluations(filter).await?;
        Ok(result)
    }

    async fn get_evaluation_count(
        &self,
        filter: FeatureEvaluationFilter,
    ) -> Result<i64, FeatureEvaluationLogicError> {
        let result = self.repository.get_evaluation_count(filter).await?;
        Ok(result)
    }

    fn clone_box(&self) -> Box<dyn FeatureEvaluationLogic> {
        Box::new(FeatureEvaluationLogicImpl {
            repository: self.repository.clone_box(),
        })
    }
}

pub fn feature_evaluation_logic(
    repository: Box<dyn FeatureEvaluationRepository>,
) -> Box<dyn FeatureEvaluationLogic> {
    Box::new(FeatureEvaluationLogicImpl::new(repository))
}
