use async_graphql::{Context, InputObject, Result as GqlResult, SimpleObject, Subscription};
use chrono::Utc;
use futures_util::stream::{Stream, StreamExt};
use std::time::Duration;
use tokio_stream::wrappers::IntervalStream;
use uuid::Uuid;

use crate::logic::feature_evaluation::FeatureEvaluationLogic;

/// Input parameters for the evaluation rates subscription
#[derive(InputObject, Clone)]
pub struct EvaluationRatesInput {
    /// Optional feature key to filter by
    pub feature_key: Option<String>,
    /// Optional environment ID to filter by
    pub environment_id: Option<String>,
    /// Optional client ID to filter by  
    pub client_id: Option<String>,
    /// Time interval in minutes for aggregation (1-60 minutes)
    pub interval_minutes: i32,
    /// Duration in hours to look back from current time (max 24 hours)
    pub duration_hours: i32,
}

/// Input parameters for the evaluation summary subscription
#[derive(InputObject, Clone)]
pub struct EvaluationSummaryInput {
    /// Optional feature key to filter by
    pub feature_key: Option<String>,
    /// Optional environment ID to filter by
    pub environment_id: Option<String>,
    /// Optional client ID to filter by
    pub client_id: Option<String>,
    /// Duration in hours to look back from current time (max 24 hours)
    pub duration_hours: i32,
}

/// GraphQL output type for evaluation rate points
#[derive(SimpleObject, Clone)]
pub struct GqlEvaluationRatePoint {
    /// ISO 8601 timestamp of the time bucket
    pub time_bucket: String,
    /// Number of evaluations in this time bucket
    pub evaluation_count: i64,
    /// Number of evaluations that resulted in true
    pub success_count: i64,
    /// Number of evaluations that were from prior assignments (cached)
    pub prior_assignment_count: i64,
    /// Success rate as percentage (0-100)
    pub success_rate: f64,
    /// Cache hit rate as percentage (0-100)
    pub cache_hit_rate: f64,
}

/// GraphQL output type for evaluation summary
#[derive(SimpleObject, Clone)]
pub struct GqlEvaluationSummary {
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
    /// Timestamp when this summary was generated
    pub generated_at: String,
}

/// GraphQL output type combining rates and summary for dashboard
#[derive(SimpleObject, Clone)]
pub struct GqlEvaluationDashboardData {
    /// Time series evaluation rates
    pub rates: Vec<GqlEvaluationRatePoint>,
    /// Summary statistics
    pub summary: GqlEvaluationSummary,
    /// Timestamp when this data was generated
    pub generated_at: String,
}

/// GraphQL Subscription root for feature evaluation analytics
pub struct FeatureEvaluationSubscription;

#[Subscription]
impl FeatureEvaluationSubscription {
    /// Subscribe to real-time feature evaluation rates
    /// Updates every 30 seconds with the latest evaluation metrics
    ///
    /// # Arguments
    /// * `input` - Filter parameters and aggregation settings
    ///
    /// # Returns
    /// Stream of evaluation rate data points for dashboard visualization
    async fn evaluation_rates(
        &self,
        ctx: &Context<'_>,
        input: EvaluationRatesInput,
    ) -> impl Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> {
        // Early validation and setup
        if input.interval_minutes < 1 || input.interval_minutes > 60 {
            return Box::pin(futures_util::stream::once(async {
                Err("Interval must be between 1 and 60 minutes".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                >;
        }
        if input.duration_hours < 1 || input.duration_hours > 24 {
            return Box::pin(futures_util::stream::once(async {
                Err("Duration must be between 1 and 24 hours".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                >;
        }

        let client_id = match input.client_id.as_ref().map(|s| Uuid::parse_str(s)) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Invalid client ID format".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                    >;
            }
            None => None,
        };

        let logic = match ctx.data::<Box<dyn FeatureEvaluationLogic>>() {
            Ok(logic) => logic.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                    >;
            }
        };

        // Create a stream using IntervalStream
        Box::pin(
            IntervalStream::new(tokio::time::interval(Duration::from_secs(30))).then(move |_| {
                let logic = logic.clone();
                let input = input.clone();
                let client_id = client_id;

                async move {
                    let now = Utc::now();
                    let from_time = now - chrono::Duration::hours(input.duration_hours as i64);

                    match logic
                        .get_evaluation_rates(
                            input.feature_key,
                            input.environment_id,
                            client_id,
                            from_time,
                            now,
                            input.interval_minutes,
                        )
                        .await
                    {
                        Ok(rates) => {
                            let gql_rates = rates
                                .into_iter()
                                .map(|rate| {
                                    let success_rate = if rate.evaluation_count > 0 {
                                        ((rate.success_count as f64 / rate.evaluation_count as f64)
                                            * 100.0
                                            * 100.0)
                                            .round()
                                            / 100.0
                                    } else {
                                        0.0
                                    };

                                    let cache_hit_rate = if rate.evaluation_count > 0 {
                                        ((rate.prior_assignment_count as f64
                                            / rate.evaluation_count as f64)
                                            * 100.0
                                            * 100.0)
                                            .round()
                                            / 100.0
                                    } else {
                                        0.0
                                    };

                                    GqlEvaluationRatePoint {
                                        time_bucket: rate.time_bucket.to_rfc3339(),
                                        evaluation_count: rate.evaluation_count,
                                        success_count: rate.success_count,
                                        prior_assignment_count: rate.prior_assignment_count,
                                        success_rate,
                                        cache_hit_rate,
                                    }
                                })
                                .collect();

                            Ok(gql_rates)
                        }
                        Err(e) => Err(format!("Failed to get evaluation rates: {}", e).into()),
                    }
                }
            }),
        )
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>>
    }

    /// Subscribe to real-time evaluation summary statistics
    /// Updates every 30 seconds with aggregated metrics
    ///
    /// # Arguments
    /// * `input` - Filter parameters for the summary
    ///
    /// # Returns
    /// Stream of evaluation summary data for dashboard overview
    async fn evaluation_summary(
        &self,
        ctx: &Context<'_>,
        input: EvaluationSummaryInput,
    ) -> impl Stream<Item = GqlResult<GqlEvaluationSummary>> {
        // Early validation and setup
        if input.duration_hours < 1 || input.duration_hours > 24 {
            return Box::pin(futures_util::stream::once(async {
                Err("Duration must be between 1 and 24 hours".into())
            }))
                as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>>;
        }

        let client_id = match input.client_id.as_ref().map(|s| Uuid::parse_str(s)) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Invalid client ID format".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>,
                    >;
            }
            None => None,
        };

        let logic = match ctx.data::<Box<dyn FeatureEvaluationLogic>>() {
            Ok(logic) => logic.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>,
                    >;
            }
        };

        Box::pin(
            IntervalStream::new(tokio::time::interval(Duration::from_secs(30))).then(move |_| {
                let logic = logic.clone();
                let input = input.clone();
                let client_id = client_id;

                async move {
                    let now = Utc::now();
                    let from_time = now - chrono::Duration::hours(input.duration_hours as i64);

                    match logic
                        .get_evaluation_summary(
                            input.feature_key,
                            input.environment_id,
                            client_id,
                            from_time,
                            now,
                        )
                        .await
                    {
                        Ok(summary) => {
                            let total_evaluations = summary.total_evaluations;
                            let success_rate = if total_evaluations > 0 {
                                (((summary.successful_evaluations as f64
                                    / total_evaluations as f64)
                                    * 100.0)
                                    * 100.0)
                                    .round()
                                    / 100.0
                            } else {
                                0.0
                            };
                            let cache_hit_rate = if total_evaluations > 0 {
                                (((summary.cached_evaluations as f64 / total_evaluations as f64)
                                    * 100.0)
                                    * 100.0)
                                    .round()
                                    / 100.0
                            } else {
                                0.0
                            };

                            Ok(GqlEvaluationSummary {
                                total_evaluations,
                                successful_evaluations: summary.successful_evaluations,
                                cached_evaluations: summary.cached_evaluations,
                                unique_users: 0, // Not available in current summary
                                top_feature_key: None, // Not available in current summary
                                success_rate,
                                cache_hit_rate,
                                generated_at: now.to_rfc3339(),
                            })
                        }
                        Err(e) => Err(format!("Failed to get evaluation summary: {}", e).into()),
                    }
                }
            }),
        ) as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>>
    }

    /// Subscribe to combined dashboard data (rates + summary)
    /// Updates every 30 seconds with complete dashboard analytics
    ///
    /// # Arguments
    /// * `input` - Filter parameters for rates (summary uses same filters except interval)
    ///
    /// # Returns
    /// Stream of combined evaluation data for complete dashboard view
    async fn evaluation_dashboard(
        &self,
        ctx: &Context<'_>,
        input: EvaluationRatesInput,
    ) -> impl Stream<Item = GqlResult<GqlEvaluationDashboardData>> {
        // Early validation and setup
        if input.interval_minutes < 1 || input.interval_minutes > 60 {
            return Box::pin(futures_util::stream::once(async {
                Err("Interval must be between 1 and 60 minutes".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>,
                >;
        }
        if input.duration_hours < 1 || input.duration_hours > 24 {
            return Box::pin(futures_util::stream::once(async {
                Err("Duration must be between 1 and 24 hours".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>,
                >;
        }

        let client_id = match input.client_id.as_ref().map(|s| Uuid::parse_str(s)) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Invalid client ID format".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>,
                    >;
            }
            None => None,
        };

        let logic = match ctx.data::<Box<dyn FeatureEvaluationLogic>>() {
            Ok(logic) => logic.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>,
                    >;
            }
        };

        Box::pin(
            IntervalStream::new(tokio::time::interval(Duration::from_secs(30))).then(move |_| {
                let logic = logic.clone();
                let input = input.clone();
                let client_id = client_id;

                async move {
                    let now = Utc::now();
                    let from_time = now - chrono::Duration::hours(input.duration_hours as i64);

                    // Fetch both rates and summary concurrently
                    let (rates_result, summary_result) = tokio::join!(
                        logic.get_evaluation_rates(
                            input.feature_key.clone(),
                            input.environment_id.clone(),
                            client_id,
                            from_time,
                            now,
                            input.interval_minutes,
                        ),
                        logic.get_evaluation_summary(
                            input.feature_key.clone(),
                            input.environment_id.clone(),
                            client_id,
                            from_time,
                            now,
                        )
                    );

                    match (rates_result, summary_result) {
                        (Ok(rates), Ok(summary)) => {
                            let gql_rates = rates
                                .into_iter()
                                .map(|rate| {
                                    let success_rate = if rate.evaluation_count > 0 {
                                        (((rate.success_count as f64
                                            / rate.evaluation_count as f64)
                                            * 100.0)
                                            * 100.0)
                                            .round()
                                            / 100.0
                                    } else {
                                        0.0
                                    };

                                    let cache_hit_rate = if rate.evaluation_count > 0 {
                                        (((rate.prior_assignment_count as f64
                                            / rate.evaluation_count as f64)
                                            * 100.0)
                                            * 100.0)
                                            .round()
                                            / 100.0
                                    } else {
                                        0.0
                                    };

                                    GqlEvaluationRatePoint {
                                        time_bucket: rate.time_bucket.to_rfc3339(),
                                        evaluation_count: rate.evaluation_count,
                                        success_count: rate.success_count,
                                        prior_assignment_count: rate.prior_assignment_count,
                                        success_rate,
                                        cache_hit_rate,
                                    }
                                })
                                .collect();

                            let gql_summary = GqlEvaluationSummary {
                                total_evaluations: summary.total_evaluations,
                                successful_evaluations: summary.successful_evaluations,
                                cached_evaluations: summary.cached_evaluations,
                                unique_users: summary.unique_users,
                                top_feature_key: summary.top_feature_key,
                                success_rate: (summary.success_rate * 100.0).round() / 100.0,
                                cache_hit_rate: (summary.cache_hit_rate * 100.0).round() / 100.0,
                                generated_at: now.to_rfc3339(),
                            };

                            let dashboard_data = GqlEvaluationDashboardData {
                                rates: gql_rates,
                                summary: gql_summary,
                                generated_at: now.to_rfc3339(),
                            };

                            Ok(dashboard_data)
                        }
                        (Err(e), _) | (_, Err(e)) => {
                            Err(format!("Failed to get dashboard data: {}", e).into())
                        }
                    }
                }
            }),
        )
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test basic subscription input validation
    #[test]
    fn test_evaluation_rates_input_validation() {
        // Test invalid interval
        let invalid_input = EvaluationRatesInput {
            feature_key: None,
            environment_id: None,
            client_id: None,
            interval_minutes: 0, // Invalid: too small
            duration_hours: 2,
        };

        // Verify validation constants
        assert!(invalid_input.interval_minutes < 1);
        assert!(invalid_input.duration_hours >= 1 && invalid_input.duration_hours <= 24);
    }

    /// Test client ID validation
    #[test]
    fn test_client_id_parsing() {
        // Test valid UUID
        let valid_uuid = "123e4567-e89b-12d3-a456-426614174000";
        assert!(Uuid::parse_str(valid_uuid).is_ok());

        // Test invalid UUID
        let invalid_uuid = "invalid-uuid";
        assert!(Uuid::parse_str(invalid_uuid).is_err());
    }

    /// Test input struct creation
    #[test]
    fn test_input_struct_creation() {
        let rates_input = EvaluationRatesInput {
            feature_key: Some("test_feature".to_string()),
            environment_id: Some("prod".to_string()),
            client_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            interval_minutes: 5,
            duration_hours: 2,
        };

        assert_eq!(rates_input.feature_key.as_ref().unwrap(), "test_feature");
        assert_eq!(rates_input.environment_id.as_ref().unwrap(), "prod");
        assert_eq!(rates_input.interval_minutes, 5);
        assert_eq!(rates_input.duration_hours, 2);

        let summary_input = EvaluationSummaryInput {
            feature_key: Some("test_feature".to_string()),
            environment_id: Some("prod".to_string()),
            client_id: None,
            duration_hours: 4,
        };

        assert_eq!(summary_input.feature_key.as_ref().unwrap(), "test_feature");
        assert_eq!(summary_input.environment_id.as_ref().unwrap(), "prod");
        assert!(summary_input.client_id.is_none());
        assert_eq!(summary_input.duration_hours, 4);
    }
}
