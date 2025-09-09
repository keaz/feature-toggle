use crate::database::feature_evaluation::{
    CreateFeatureEvaluation, EvaluationRatePoint, EvaluationSummary, FeatureEvaluationFilter,
    FeatureEvaluationRepository, FeatureEvaluationRow,
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
        if !(1..=60).contains(&interval_minutes) {
            return Err(FeatureEvaluationLogicError::InvalidInput(
                "Interval must be between 1 and 60 minutes".to_string(),
            ));
        }

        let rates = self
            .repository
            .get_evaluation_rates(
                feature_key,
                environment_id,
                client_id,
                from_time,
                to_time,
                interval_minutes,
            )
            .await?;

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

        let summary = self
            .repository
            .get_evaluation_summary(feature_key, environment_id, client_id, from_time, to_time)
            .await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::feature_evaluation::MockFeatureEvaluationRepository;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn sample_evaluation() -> CreateFeatureEvaluation {
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

    fn sample_evaluation_row() -> FeatureEvaluationRow {
        let eval = sample_evaluation();
        FeatureEvaluationRow {
            id: Uuid::new_v4(),
            feature_key: eval.feature_key,
            environment_id: eval.environment_id,
            client_id: eval.client_id,
            evaluated_at: eval.evaluated_at,
            evaluation_result: eval.evaluation_result,
            evaluation_context: eval.evaluation_context,
            user_context: eval.user_context,
            prior_assignment: eval.prior_assignment,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_record_evaluation_success() {
        let mut mock_repo = MockFeatureEvaluationRepository::new();
        let evaluation = sample_evaluation();
        let expected_row = sample_evaluation_row();

        mock_repo
            .expect_create_evaluation()
            .withf(|eval| eval.feature_key == "test-feature" && eval.environment_id == "env-123")
            .times(1)
            .return_once(move |_| Ok(expected_row));

        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic
            .record_evaluation(
                evaluation.feature_key,
                evaluation.environment_id,
                evaluation.client_id,
                evaluation.evaluated_at,
                evaluation.evaluation_result,
                evaluation.evaluation_context,
                evaluation.user_context,
                evaluation.prior_assignment,
            )
            .await;

        assert!(result.is_ok());
        let row = result.unwrap();
        assert_eq!(row.feature_key, "test-feature");
        assert_eq!(row.environment_id, "env-123");
    }

    #[tokio::test]
    async fn test_record_evaluation_empty_feature_key() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic
            .record_evaluation(
                "".to_string(), // Empty feature key
                "env-123".to_string(),
                Uuid::new_v4(),
                Utc::now(),
                true,
                None,
                None,
                false,
            )
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "Feature key cannot be empty");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_record_evaluation_empty_environment_id() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic
            .record_evaluation(
                "test-feature".to_string(),
                "".to_string(), // Empty environment ID
                Uuid::new_v4(),
                Utc::now(),
                true,
                None,
                None,
                false,
            )
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "Environment ID cannot be empty");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_record_evaluations_bulk_success() {
        let mut mock_repo = MockFeatureEvaluationRepository::new();
        let evaluations = vec![sample_evaluation(), sample_evaluation()];
        let expected_rows = vec![sample_evaluation_row(), sample_evaluation_row()];

        mock_repo
            .expect_bulk_create_evaluations()
            .withf(|evals| evals.len() == 2)
            .times(1)
            .return_once(move |_| Ok(expected_rows));

        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic.record_evaluations_bulk(evaluations).await;

        assert!(result.is_ok());
        let rows = result.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn test_record_evaluations_bulk_empty() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic.record_evaluations_bulk(vec![]).await;

        assert!(result.is_ok());
        let rows = result.unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn test_record_evaluations_bulk_invalid_feature_key() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let mut evaluation = sample_evaluation();
        evaluation.feature_key = "".to_string(); // Empty feature key
        let evaluations = vec![evaluation];

        let result = logic.record_evaluations_bulk(evaluations).await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "Feature key cannot be empty");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_get_evaluation_rates_success() {
        let mut mock_repo = MockFeatureEvaluationRepository::new();
        let from_time = Utc::now() - chrono::Duration::hours(1);
        let to_time = Utc::now();

        let expected_rates = vec![EvaluationRatePoint {
            time_bucket: from_time,
            evaluation_count: 10,
            success_count: 8,
            prior_assignment_count: 3,
        }];

        mock_repo
            .expect_get_evaluation_rates()
            .withf(|_, _, _, _, _, interval| *interval == 15)
            .times(1)
            .return_once(move |_, _, _, _, _, _| Ok(expected_rates));

        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic
            .get_evaluation_rates(
                Some("test-feature".to_string()),
                Some("env-123".to_string()),
                None,
                from_time,
                to_time,
                15, // 15 minutes interval
            )
            .await;

        assert!(result.is_ok());
        let rates = result.unwrap();
        assert_eq!(rates.len(), 1);
        assert_eq!(rates[0].evaluation_count, 10);
    }

    #[tokio::test]
    async fn test_get_evaluation_rates_invalid_time_range() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let from_time = Utc::now() - chrono::Duration::hours(25); // > 24 hours
        let to_time = Utc::now();

        let result = logic
            .get_evaluation_rates(None, None, None, from_time, to_time, 15)
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "Time range cannot exceed 24 hours");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_get_evaluation_rates_future_time() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let from_time = Utc::now() + chrono::Duration::hours(1); // Future time
        let to_time = Utc::now() + chrono::Duration::hours(2);

        let result = logic
            .get_evaluation_rates(None, None, None, from_time, to_time, 15)
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "From time cannot be in the future");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_get_evaluation_rates_invalid_interval() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let from_time = Utc::now() - chrono::Duration::hours(1);
        let to_time = Utc::now();

        let result = logic
            .get_evaluation_rates(
                None, None, None, from_time, to_time, 0, // Invalid interval
            )
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "Interval must be between 1 and 60 minutes");
            }
            _ => panic!("Expected InvalidInput error"),
        }

        let result = logic
            .get_evaluation_rates(
                None, None, None, from_time, to_time, 61, // Invalid interval (> 60)
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_evaluation_summary_success() {
        let mut mock_repo = MockFeatureEvaluationRepository::new();
        let from_time = Utc::now() - chrono::Duration::hours(1);
        let to_time = Utc::now();

        let expected_summary = EvaluationSummary {
            total_evaluations: 100,
            successful_evaluations: 80,
            cached_evaluations: 30,
            unique_users: 25,
            top_feature_key: Some("test-feature".to_string()),
            success_rate: 80.0,
            cache_hit_rate: 30.0,
        };

        mock_repo
            .expect_get_evaluation_summary()
            .times(1)
            .return_once(move |_, _, _, _, _| Ok(expected_summary));

        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let result = logic
            .get_evaluation_summary(
                Some("test-feature".to_string()),
                Some("env-123".to_string()),
                None,
                from_time,
                to_time,
            )
            .await;

        assert!(result.is_ok());
        let summary = result.unwrap();
        assert_eq!(summary.total_evaluations, 100);
        assert_eq!(summary.successful_evaluations, 80);
        assert_eq!(summary.success_rate, 80.0);
    }

    #[tokio::test]
    async fn test_get_evaluation_summary_invalid_time_range() {
        let mock_repo = MockFeatureEvaluationRepository::new();
        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let from_time = Utc::now() - chrono::Duration::hours(25); // > 24 hours
        let to_time = Utc::now();

        let result = logic
            .get_evaluation_summary(None, None, None, from_time, to_time)
            .await;

        assert!(result.is_err());
        match result.err().unwrap() {
            FeatureEvaluationLogicError::InvalidInput(msg) => {
                assert_eq!(msg, "Time range cannot exceed 24 hours");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_get_evaluations() {
        let mut mock_repo = MockFeatureEvaluationRepository::new();
        let expected_rows = vec![sample_evaluation_row()];

        mock_repo
            .expect_get_evaluations()
            .times(1)
            .return_once(move |_| Ok(expected_rows));

        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let filter = FeatureEvaluationFilter {
            feature_key: Some("test-feature".to_string()),
            environment_id: None,
            client_id: None,
            user_context: None,
            prior_assignment: None,
            from_date: None,
            to_date: None,
            limit: None,
            offset: None,
        };

        let result = logic.get_evaluations(filter).await;
        assert!(result.is_ok());
        let rows = result.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn test_get_evaluation_count() {
        let mut mock_repo = MockFeatureEvaluationRepository::new();

        mock_repo
            .expect_get_evaluation_count()
            .times(1)
            .return_once(move |_| Ok(42));

        let logic = feature_evaluation_logic(Box::new(mock_repo));

        let filter = FeatureEvaluationFilter {
            feature_key: Some("test-feature".to_string()),
            environment_id: None,
            client_id: None,
            user_context: None,
            prior_assignment: None,
            from_date: None,
            to_date: None,
            limit: None,
            offset: None,
        };

        let result = logic.get_evaluation_count(filter).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }
}
