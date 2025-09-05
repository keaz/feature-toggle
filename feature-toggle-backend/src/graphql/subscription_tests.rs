#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::feature_evaluation::{MockFeatureEvaluationLogic, EvaluationRatePoint, EvaluationSummary};
    use async_graphql::{Schema, Context};
    use futures_util::stream::StreamExt;
    use chrono::{DateTime, Utc};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    /// Test basic subscription input validation
    #[tokio::test]
    async fn test_evaluation_rates_input_validation() {
        let subscription = FeatureEvaluationSubscription;
        
        // Test invalid interval
        let invalid_input = EvaluationRatesInput {
            feature_key: None,
            environment_id: None,
            client_id: None,
            interval_minutes: 0, // Invalid: too small
            duration_hours: 2,
        };

        let mock_logic = MockFeatureEvaluationLogic::new();
        let schema = Schema::build(
            async_graphql::EmptyQuery, 
            async_graphql::EmptyMutation, 
            subscription
        )
        .data(Box::new(mock_logic) as Box<dyn FeatureEvaluationLogic>)
        .finish();

        let ctx = Context::new();
        let mut stream = FeatureEvaluationSubscription.evaluation_rates(&ctx, invalid_input).await;
        
        // Should return error for invalid interval
        if let Some(result) = stream.next().await {
            assert!(result.is_err());
            assert!(result.unwrap_err().message.contains("Interval must be between 1 and 60 minutes"));
        }
    }

    /// Test evaluation summary subscription with valid data
    #[tokio::test]
    async fn test_evaluation_summary_subscription() {
        let subscription = FeatureEvaluationSubscription;
        
        let valid_input = EvaluationSummaryInput {
            feature_key: Some("test_feature".to_string()),
            environment_id: Some("test_env".to_string()),
            client_id: None,
            duration_hours: 2,
        };

        let mut mock_logic = MockFeatureEvaluationLogic::new();
        
        // Mock the get_evaluation_summary method
        let expected_summary = EvaluationSummary {
            total_evaluations: 100,
            successful_evaluations: 80,
            cached_evaluations: 30,
            unique_users: 25,
            top_feature_key: Some("test_feature".to_string()),
            success_rate: 80.0,
            cache_hit_rate: 30.0,
        };
        
        mock_logic.expect_get_evaluation_summary()
            .returning(move |_, _, _, _, _| Ok(expected_summary.clone()));

        let schema = Schema::build(
            async_graphql::EmptyQuery, 
            async_graphql::EmptyMutation, 
            subscription
        )
        .data(Box::new(mock_logic) as Box<dyn FeatureEvaluationLogic>)
        .finish();

        let ctx = Context::new();
        let mut stream = FeatureEvaluationSubscription.evaluation_summary(&ctx, valid_input).await;
        
        // Get first emission from stream
        if let Some(result) = stream.next().await {
            assert!(result.is_ok());
            let summary = result.unwrap();
            
            assert_eq!(summary.total_evaluations, 100);
            assert_eq!(summary.successful_evaluations, 80);
            assert_eq!(summary.cached_evaluations, 30);
            assert_eq!(summary.unique_users, 25);
            assert_eq!(summary.top_feature_key.as_ref().unwrap(), "test_feature");
            assert!((summary.success_rate - 80.0).abs() < f64::EPSILON);
            assert!((summary.cache_hit_rate - 30.0).abs() < f64::EPSILON);
        }
    }

    /// Test that the subscription handles client ID parsing correctly
    #[tokio::test]
    async fn test_client_id_validation() {
        let subscription = FeatureEvaluationSubscription;
        
        let invalid_input = EvaluationRatesInput {
            feature_key: None,
            environment_id: None,
            client_id: Some("invalid-uuid".to_string()), // Invalid UUID
            interval_minutes: 5,
            duration_hours: 2,
        };

        let mock_logic = MockFeatureEvaluationLogic::new();
        let schema = Schema::build(
            async_graphql::EmptyQuery, 
            async_graphql::EmptyMutation, 
            subscription
        )
        .data(Box::new(mock_logic) as Box<dyn FeatureEvaluationLogic>)
        .finish();

        let ctx = Context::new();
        let mut stream = FeatureEvaluationSubscription.evaluation_rates(&ctx, invalid_input).await;
        
        // Should return error for invalid UUID
        if let Some(result) = stream.next().await {
            assert!(result.is_err());
            assert!(result.unwrap_err().message.contains("Invalid client ID format"));
        }
    }

    /// Test the rate calculation logic in subscription
    #[tokio::test]
    async fn test_rate_calculation() {
        let subscription = FeatureEvaluationSubscription;
        
        let valid_input = EvaluationRatesInput {
            feature_key: Some("test_feature".to_string()),
            environment_id: Some("prod".to_string()),
            client_id: None,
            interval_minutes: 5,
            duration_hours: 1,
        };

        let mut mock_logic = MockFeatureEvaluationLogic::new();
        
        // Mock rate data with specific values for testing calculations
        let mock_rates = vec![
            EvaluationRatePoint {
                time_bucket: Utc::now(),
                evaluation_count: 100,
                success_count: 80,
                prior_assignment_count: 25,
            }
        ];
        
        mock_logic.expect_get_evaluation_rates()
            .returning(move |_, _, _, _, _, _| Ok(mock_rates.clone()));

        let schema = Schema::build(
            async_graphql::EmptyQuery, 
            async_graphql::EmptyMutation, 
            subscription
        )
        .data(Box::new(mock_logic) as Box<dyn FeatureEvaluationLogic>)
        .finish();

        let ctx = Context::new();
        let mut stream = FeatureEvaluationSubscription.evaluation_rates(&ctx, valid_input).await;
        
        if let Some(result) = stream.next().await {
            assert!(result.is_ok());
            let rates = result.unwrap();
            
            assert_eq!(rates.len(), 1);
            let rate_point = &rates[0];
            
            assert_eq!(rate_point.evaluation_count, 100);
            assert_eq!(rate_point.success_count, 80);
            assert_eq!(rate_point.prior_assignment_count, 25);
            
            // Verify calculated success rate: 80/100 * 100 = 80%
            assert!((rate_point.success_rate - 80.0).abs() < f64::EPSILON);
            
            // Verify calculated cache hit rate: 25/100 * 100 = 25%
            assert!((rate_point.cache_hit_rate - 25.0).abs() < f64::EPSILON);
        }
    }
}
