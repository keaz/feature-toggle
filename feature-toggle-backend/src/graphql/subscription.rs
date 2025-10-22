use async_graphql::{Context, Enum, InputObject, Result as GqlResult, SimpleObject, Subscription};
use async_stream::stream;
use chrono::{DateTime, Utc};
use futures_util::stream::Stream;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::database::activity_log::{ActivityLogFilter, ActivityLogRepository};
use crate::logic::client::ClientLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::feature_evaluation::FeatureEvaluationLogic;
use std::sync::Arc;

// Helper to round a percentage value (already in 0-100 range) to 2 decimal places
fn round_pct(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Calculate from_time and to_time based on the given period
/// Returns (from_time, to_time) where to_time is now
pub fn calculate_time_range(
    period: TimePeriod,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    match period {
        TimePeriod::H24 => {
            let from = now - chrono::Duration::hours(24);
            (from, now)
        }
        TimePeriod::D7 => {
            let from = now - chrono::Duration::days(7);
            (from, now)
        }
        TimePeriod::D30 => {
            let from = now - chrono::Duration::days(30);
            (from, now)
        }
    }
}

/// Time period enum for evaluation summary queries
#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimePeriod {
    /// 24 hours
    #[graphql(name = "PERIOD_24H")]
    H24,
    /// 7 days
    #[graphql(name = "PERIOD_7D")]
    D7,
    /// 30 days
    #[graphql(name = "PERIOD_30D")]
    D30,
}

/// Input parameters for the evaluation rates subscription
/// Matches UI shape: fromTime, toTime, intervalMinutes, optional filters.
#[derive(InputObject, Clone)]
pub struct EvaluationRatesInput {
    #[graphql(name = "featureKey")]
    pub feature_key: Option<String>,
    #[graphql(name = "environmentId")]
    pub environment_id: Option<String>,
    #[graphql(name = "clientId")]
    pub client_id: Option<String>,
    #[graphql(name = "fromTime")]
    pub from_time: DateTime<Utc>,
    #[graphql(name = "toTime")]
    pub to_time: DateTime<Utc>,
    #[graphql(name = "intervalMinutes")]
    pub interval_minutes: i32,
}

/// Input parameters for evaluation rates using period (24H, 7D, 30D)
#[derive(InputObject, Clone)]
pub struct EvaluationRatesInputWithPeriod {
    #[graphql(name = "featureKey")]
    pub feature_key: Option<String>,
    #[graphql(name = "environmentId")]
    pub environment_id: Option<String>,
    #[graphql(name = "clientId")]
    pub client_id: Option<String>,
    pub period: TimePeriod,
    #[graphql(name = "intervalMinutes")]
    pub interval_minutes: i32,
}

/// Input parameters for the evaluation summary subscription
#[derive(InputObject, Clone)]
pub struct EvaluationSummaryInput {
    #[graphql(name = "featureKey")]
    pub feature_key: Option<String>,
    #[graphql(name = "environmentId")]
    pub environment_id: Option<String>,
    #[graphql(name = "clientId")]
    pub client_id: Option<String>,
    pub period: TimePeriod,
}

/// GraphQL output type for evaluation rate points
#[derive(SimpleObject, Clone)]
pub struct GqlEvaluationRatePoint {
    #[graphql(name = "timeBucket")]
    pub time_bucket: String,
    #[graphql(name = "evaluationCount")]
    pub evaluation_count: i64,
    #[graphql(name = "successCount")]
    pub success_count: i64,
    #[graphql(name = "priorAssignmentCount")]
    pub prior_assignment_count: i64,
    #[graphql(name = "successRate")]
    pub success_rate: f64,
    #[graphql(name = "cacheHitRate")]
    pub cache_hit_rate: f64,
}

/// GraphQL output type for evaluation summary
#[derive(SimpleObject, Clone)]
pub struct GqlEvaluationSummary {
    #[graphql(name = "totalEvaluations")]
    pub total_evaluations: i64,
    #[graphql(name = "successfulEvaluations")]
    pub successful_evaluations: i64,
    #[graphql(name = "cachedEvaluations")]
    pub cached_evaluations: i64,
    #[graphql(name = "uniqueUsers")]
    pub unique_users: i64,
    #[graphql(name = "topFeatureKey")]
    pub top_feature_key: Option<String>,
    #[graphql(name = "successRate")]
    pub success_rate: f64,
    #[graphql(name = "cacheHitRate")]
    pub cache_hit_rate: f64,
    #[graphql(name = "generatedAt")]
    pub generated_at: String,
}

/// Output row for live evaluations grouped by feature
#[derive(SimpleObject, Clone)]
pub struct GqlEvaluationByFeatureRow {
    #[graphql(name = "featureKey")]
    pub feature_key: String,
    #[graphql(name = "totalEvaluations")]
    pub total_evaluations: i64,
    #[graphql(name = "successfulEvaluations")]
    pub successful_evaluations: i64,
    #[graphql(name = "cachedEvaluations")]
    pub cached_evaluations: i64,
    #[graphql(name = "uniqueUsers")]
    pub unique_users: i64,
    #[graphql(name = "lastEvaluatedAt")]
    pub last_evaluated_at: String,
    #[graphql(name = "sequence")]
    pub sequence: i64,
    #[graphql(name = "emittedAt")]
    pub emitted_at: String,
}

/// Input for live top-features subscription (using period-based time range)
#[derive(InputObject, Clone)]
pub struct EvaluationsByFeatureLiveInput {
    pub period: TimePeriod,
    #[graphql(name = "environmentId")]
    pub environment_id: Option<String>,
    #[graphql(name = "clientId")]
    pub client_id: Option<String>,
    #[graphql(name = "limit")]
    pub limit: Option<i32>,
    #[graphql(name = "offset")]
    pub offset: Option<i32>,
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
    // DESIGN NOTE:
    // The evaluation subscriptions are event-driven: instead of polling at fixed intervals,
    // they listen to a broadcast channel populated whenever a feature evaluation is recorded.
    // Each incoming event triggers recomputation of the aggregated window (rates or summary)
    // to keep logic centralized and avoid pushing large incremental state diffs.
    // This preserves correctness (recomputing with authoritative DB query), while eliminating
    // unnecessary periodic queries when there are no new evaluations.
    /// Subscribe to real-time feature evaluation rates with period
    /// Updates every 30 seconds with the latest evaluation metrics
    ///
    /// # Arguments
    /// * `input` - Filter parameters, period, and aggregation settings
    ///
    /// # Returns
    /// Stream of evaluation rate data points for dashboard visualization
    async fn evaluation_rates_with_period(
        &self,
        ctx: &Context<'_>,
        input: EvaluationRatesInputWithPeriod,
    ) -> impl Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> {
        // Validation: interval bounds
        if !(1..=60).contains(&input.interval_minutes) {
            return Box::pin(futures_util::stream::once(async {
                Err("Interval must be between 1 and 60 minutes".into())
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
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                    >;
            }
        };

        // Obtain evaluation event broadcast sender
        let mut events_rx =
            match ctx.data::<tokio::sync::broadcast::Sender<
                crate::logic::feature_evaluation::FeatureEvaluationEvent,
            >>() {
                Ok(tx) => tx.subscribe(),
                Err(_) => {
                    return Box::pin(futures_util::stream::once(async {
                        Err("Evaluation events channel not found".into())
                    }))
                        as std::pin::Pin<
                            Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                        >;
                }
            };

        // Helper closure to fetch and yield evaluation rates
        let fetch_rates = |logic: &Box<dyn FeatureEvaluationLogic>,
                           input: &EvaluationRatesInputWithPeriod,
                           client_id: Option<Uuid>| {
            let logic_clone = logic.clone();
            let input_clone = input.clone();
            async move {
                let now = Utc::now();
                let (from_time, to_time) = calculate_time_range(input_clone.period, now);
                match logic_clone
                    .get_evaluation_rates(
                        input_clone.feature_key.clone(),
                        input_clone.environment_id.clone(),
                        client_id,
                        from_time,
                        to_time,
                        input_clone.interval_minutes,
                    )
                    .await
                {
                    Ok(rates) => {
                        let mapped = rates
                            .into_iter()
                            .map(|rate| {
                                let success_rate = if rate.evaluation_count > 0 {
                                    (rate.success_count as f64 / rate.evaluation_count as f64)
                                        * 100.0
                                } else {
                                    0.0
                                };
                                let cache_hit_rate = if rate.evaluation_count > 0 {
                                    (rate.prior_assignment_count as f64
                                        / rate.evaluation_count as f64)
                                        * 100.0
                                } else {
                                    0.0
                                };

                                GqlEvaluationRatePoint {
                                    time_bucket: rate.time_bucket.to_rfc3339(),
                                    evaluation_count: rate.evaluation_count,
                                    success_count: rate.success_count,
                                    prior_assignment_count: rate.prior_assignment_count,
                                    success_rate: round_pct(success_rate),
                                    cache_hit_rate: round_pct(cache_hit_rate),
                                }
                            })
                            .collect();
                        Ok(mapped)
                    }
                    Err(e) => Err(format!("Failed to get evaluation rates: {}", e)),
                }
            }
        };

        let stream = stream! {
            // Send initial data immediately on subscription connect
            match fetch_rates(&logic, &input, client_id).await {
                Ok(rates) => {
                    yield Ok(rates);
                }
                Err(e) => {
                    yield Err(e.into());
                }
            }

            // Continue listening for events and send updated data
            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        log::debug!("[subscriptions] evaluation event received; recomputing aggregation");
                        match fetch_rates(&logic, &input, client_id).await {
                            Ok(rates) => {
                                yield Ok(rates);
                            }
                            Err(e) => {
                                yield Err(e.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>>
    }

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
        // Validation: interval bounds
        if !(1..=60).contains(&input.interval_minutes) {
            return Box::pin(futures_util::stream::once(async {
                Err("Interval must be between 1 and 60 minutes".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                >;
        }
        // Validate time range (max 24h for subscription window)
        if input.to_time < input.from_time {
            return Box::pin(futures_util::stream::once(async {
                Err("toTime must be >= fromTime".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                >;
        }
        let duration_hours = (input.to_time - input.from_time).num_hours();
        if duration_hours > 24 {
            return Box::pin(futures_util::stream::once(async {
                Err("Time range cannot exceed 24 hours".into())
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
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                    >;
            }
        };

        // Obtain evaluation event broadcast sender injected during schema build
        // We expect a Sender<FeatureEvaluationEvent> to be inserted under a known type.
        let mut events_rx =
            match ctx.data::<tokio::sync::broadcast::Sender<
                crate::logic::feature_evaluation::FeatureEvaluationEvent,
            >>() {
                Ok(tx) => tx.subscribe(),
                Err(_) => {
                    return Box::pin(futures_util::stream::once(async {
                        Err("Evaluation events channel not found".into())
                    }))
                        as std::pin::Pin<
                            Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>,
                        >;
                }
            };

        // Helper closure to fetch and yield evaluation rates
        let fetch_rates = |logic: &Box<dyn FeatureEvaluationLogic>,
                           input: &EvaluationRatesInput,
                           client_id: Option<Uuid>| {
            let logic_clone = logic.clone();
            let input_clone = input.clone();
            async move {
                let now = Utc::now();
                let upper = if input_clone.to_time > now {
                    now
                } else {
                    input_clone.to_time
                };
                let from_time = input_clone.from_time;
                match logic_clone
                    .get_evaluation_rates(
                        input_clone.feature_key.clone(),
                        input_clone.environment_id.clone(),
                        client_id,
                        from_time,
                        upper,
                        input_clone.interval_minutes,
                    )
                    .await
                {
                    Ok(rates) => {
                        let mapped: Vec<GqlEvaluationRatePoint> = rates
                            .into_iter()
                            .map(|rate| {
                                let success_rate = if rate.evaluation_count > 0 {
                                    (rate.success_count as f64 / rate.evaluation_count as f64)
                                        * 100.0
                                } else {
                                    0.0
                                };
                                let cache_hit_rate = if rate.evaluation_count > 0 {
                                    (rate.prior_assignment_count as f64
                                        / rate.evaluation_count as f64)
                                        * 100.0
                                } else {
                                    0.0
                                };
                                GqlEvaluationRatePoint {
                                    time_bucket: rate.time_bucket.to_rfc3339(),
                                    evaluation_count: rate.evaluation_count,
                                    success_count: rate.success_count,
                                    prior_assignment_count: rate.prior_assignment_count,
                                    success_rate: round_pct(success_rate),
                                    cache_hit_rate: round_pct(cache_hit_rate),
                                }
                            })
                            .collect();
                        Ok(mapped)
                    }
                    Err(e) => Err(format!("Failed to get evaluation rates: {}", e)),
                }
            }
        };

        // Wrap broadcast receiver into a stream. Send initial data on connect, then updates on events.
        let stream = stream! {
            // Send initial data immediately on subscription connect
            match fetch_rates(&logic, &input, client_id).await {
                Ok(rates) => {
                    yield Ok(rates);
                }
                Err(e) => {
                    yield Err(e.into());
                }
            }

            // Continue listening for events and send updated data
            loop {
                match events_rx.recv().await {
                    Ok(_evt) => {
                        match fetch_rates(&logic, &input, client_id).await {
                            Ok(rates) => {
                                yield Ok(rates);
                            }
                            Err(e) => {
                                yield Err(e.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        Box::pin(stream)
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationRatePoint>>> + Send>>
    }

    /// Subscribe to real-time evaluation summary statistics
    /// Updates every 30 seconds with aggregated metrics
    ///
    /// # Arguments
    /// * `input` - Filter parameters for the summary including period (24H, 7D, 30D)
    ///
    /// # Returns
    /// Stream of evaluation summary data for dashboard overview
    async fn evaluation_summary(
        &self,
        ctx: &Context<'_>,
        input: EvaluationSummaryInput,
    ) -> impl Stream<Item = GqlResult<GqlEvaluationSummary>> {
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
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>,
                    >;
            }
        };
        let mut events_rx =
            match ctx.data::<tokio::sync::broadcast::Sender<
                crate::logic::feature_evaluation::FeatureEvaluationEvent,
            >>() {
                Ok(tx) => tx.subscribe(),
                Err(_) => {
                    return Box::pin(futures_util::stream::once(async {
                        Err("Evaluation events channel not found".into())
                    }))
                        as std::pin::Pin<
                            Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>,
                        >;
                }
            };

        // Helper closure to fetch and yield evaluation summary
        let fetch_summary = |logic: &Box<dyn FeatureEvaluationLogic>,
                             input: &EvaluationSummaryInput,
                             client_id: Option<Uuid>| {
            let logic_clone = logic.clone();
            let input_clone = input.clone();
            async move {
                let now = Utc::now();
                let (from_time, to_time) = calculate_time_range(input_clone.period, now);
                match logic_clone
                    .get_evaluation_summary(
                        input_clone.feature_key.clone(),
                        input_clone.environment_id.clone(),
                        client_id,
                        from_time,
                        to_time,
                    )
                    .await
                {
                    Ok(summary) => {
                        // summary.success_rate & cache_hit_rate are already 0-100 percentages from logic layer
                        let success_rate = summary.success_rate;
                        let cache_hit_rate = summary.cache_hit_rate;
                        Ok(GqlEvaluationSummary {
                            total_evaluations: summary.total_evaluations,
                            successful_evaluations: summary.successful_evaluations,
                            cached_evaluations: summary.cached_evaluations,
                            unique_users: summary.unique_users,
                            top_feature_key: summary.top_feature_key,
                            success_rate: round_pct(success_rate),
                            cache_hit_rate: round_pct(cache_hit_rate),
                            generated_at: now.to_rfc3339(),
                        })
                    }
                    Err(e) => Err(format!("Failed to get evaluation summary: {}", e)),
                }
            }
        };

        let stream = stream! {
            // Send initial data immediately on subscription connect
            match fetch_summary(&logic, &input, client_id).await {
                Ok(summary) => {
                    yield Ok(summary);
                }
                Err(e) => {
                    yield Err(e.into());
                }
            }

            // Continue listening for events and send updated data
            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        log::debug!("[subscriptions] evaluation event received; recomputing aggregation");
                        match fetch_summary(&logic, &input, client_id).await {
                            Ok(summary) => {
                                yield Ok(summary);
                            }
                            Err(e) => {
                                yield Err(e.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlEvaluationSummary>> + Send>>
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
        if input.to_time < input.from_time {
            return Box::pin(futures_util::stream::once(async {
                Err("toTime must be >= fromTime".into())
            }))
                as std::pin::Pin<
                    Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>,
                >;
        }
        if (input.to_time - input.from_time).num_hours() > 24 {
            return Box::pin(futures_util::stream::once(async {
                Err("Time range cannot exceed 24 hours".into())
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

        let mut events_rx =
            match ctx.data::<tokio::sync::broadcast::Sender<
                crate::logic::feature_evaluation::FeatureEvaluationEvent,
            >>() {
                Ok(tx) => tx.subscribe(),
                Err(_) => {
                    return Box::pin(futures_util::stream::once(async {
                        Err("Evaluation events channel not found".into())
                    }))
                        as std::pin::Pin<
                            Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>,
                        >;
                }
            };

        let stream = stream! {
            // Helper closure to fetch and yield dashboard data
            async fn fetch_dashboard_data(
                logic: &Box<dyn FeatureEvaluationLogic>,
                input: &EvaluationRatesInput,
                client_id: Option<Uuid>
            ) -> Result<GqlEvaluationDashboardData, String> {
                let now = Utc::now();
                let upper = if input.to_time > now { now } else { input.to_time };
                let from_time = input.from_time;
                let (rates_result, summary_result) = tokio::join!(
                    logic.get_evaluation_rates(
                        input.feature_key.clone(),
                        input.environment_id.clone(),
                        client_id,
                        from_time,
                        upper,
                        input.interval_minutes,
                    ),
                    logic.get_evaluation_summary(
                        input.feature_key.clone(),
                        input.environment_id.clone(),
                        client_id,
                        from_time,
                        upper,
                    )
                );
                match (rates_result, summary_result) {
                    (Ok(rates), Ok(summary)) => {
                        let gql_rates = rates.into_iter().map(|rate| {
                            let success_rate = if rate.evaluation_count > 0 { (rate.success_count as f64 / rate.evaluation_count as f64) * 100.0 } else { 0.0 };
                            let cache_hit_rate = if rate.evaluation_count > 0 { (rate.prior_assignment_count as f64 / rate.evaluation_count as f64) * 100.0 } else { 0.0 };
                            GqlEvaluationRatePoint {
                                time_bucket: rate.time_bucket.to_rfc3339(),
                                evaluation_count: rate.evaluation_count,
                                success_count: rate.success_count,
                                prior_assignment_count: rate.prior_assignment_count,
                                success_rate: round_pct(success_rate),
                                cache_hit_rate: round_pct(cache_hit_rate),
                            }
                        }).collect();
                        Ok(GqlEvaluationDashboardData {
                            rates: gql_rates,
                            summary: GqlEvaluationSummary {
                                total_evaluations: summary.total_evaluations,
                                successful_evaluations: summary.successful_evaluations,
                                cached_evaluations: summary.cached_evaluations,
                                unique_users: summary.unique_users,
                                top_feature_key: summary.top_feature_key,
                                success_rate: round_pct(summary.success_rate),
                                cache_hit_rate: round_pct(summary.cache_hit_rate),
                                generated_at: now.to_rfc3339(),
                            },
                            generated_at: now.to_rfc3339(),
                        })
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        Err(format!("Failed to get evaluation dashboard data: {}", e))
                    }
                }
            }

            // Send initial data immediately on subscription connect
            match fetch_dashboard_data(&logic, &input, client_id).await {
                Ok(data) => {
                    yield Ok(data);
                }
                Err(e) => {
                    yield Err(e.into());
                }
            }

            // Continue listening for events and send updated data
            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        match fetch_dashboard_data(&logic, &input, client_id).await {
                            Ok(data) => {
                                yield Ok(data);
                            }
                            Err(e) => {
                                yield Err(e.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlEvaluationDashboardData>> + Send>>
    }

    /// Live stream of evaluations grouped by feature ("Top Features")
    /// Emits on each evaluation event with updated aggregation.
    async fn evaluations_by_feature_live(
        &self,
        ctx: &Context<'_>,
        input: EvaluationsByFeatureLiveInput,
    ) -> impl Stream<Item = GqlResult<Vec<GqlEvaluationByFeatureRow>>> {
        // Validate client_id format
        let client_id = match input.client_id.as_ref().map(|s| Uuid::parse_str(s)) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Invalid client ID format".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationByFeatureRow>>> + Send>,
                    >;
            }
            None => None,
        };

        let logic = match ctx.data::<Box<dyn FeatureEvaluationLogic>>() {
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<
                        Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationByFeatureRow>>> + Send>,
                    >;
            }
        };

        let mut events_rx =
            match ctx.data::<tokio::sync::broadcast::Sender<
                crate::logic::feature_evaluation::FeatureEvaluationEvent,
            >>() {
                Ok(tx) => tx.subscribe(),
                Err(_) => {
                    return Box::pin(futures_util::stream::once(async {
                        Err("Evaluation events channel not found".into())
                    }))
                        as std::pin::Pin<
                            Box<
                                dyn Stream<Item = GqlResult<Vec<GqlEvaluationByFeatureRow>>> + Send,
                            >,
                        >;
                }
            };

        // Helper closure to fetch and yield evaluation data
        let fetch_evaluations = |logic: &Box<dyn FeatureEvaluationLogic>,
                                  input: &EvaluationsByFeatureLiveInput,
                                  client_id: Option<Uuid>,
                                  seq: i64| {
            let logic_clone = logic.clone();
            let input_clone = input.clone();
            async move {
                // Calculate time range dynamically based on period (rolling window)
                let now = Utc::now();
                let (from_time, to_time) = calculate_time_range(input_clone.period, now);

                match logic_clone.get_evaluations_by_feature(
                    from_time,
                    to_time,
                    input_clone.environment_id.clone(),
                    client_id,
                    input_clone.limit,
                    input_clone.offset,
                ).await {
                    Ok(rows) => {
                        log::debug!("[subscriptions] evaluations_by_feature_live sending {} rows (seq={})", rows.len(), seq);
                        let emission_time = Utc::now().to_rfc3339();
                        let mapped = rows.into_iter().map(|r| GqlEvaluationByFeatureRow {
                            feature_key: r.feature_key,
                            total_evaluations: r.total_evaluations,
                            successful_evaluations: r.successful_evaluations,
                            cached_evaluations: r.cached_evaluations,
                            unique_users: r.unique_users,
                            last_evaluated_at: r.last_evaluated_at.to_rfc3339(),
                            sequence: seq,
                            emitted_at: emission_time.clone(),
                        }).collect();
                        Ok(mapped)
                    }
                    Err(e) => Err(format!("Failed to get evaluationsByFeature: {}", e)),
                }
            }
        };

        let mut seq: i64 = 0;
        let stream = stream! {
            // Send initial data immediately on subscription connect
            match fetch_evaluations(&logic, &input, client_id, seq).await {
                Ok(rows) => {
                    yield Ok(rows);
                }
                Err(e) => {
                    yield Err(e.into());
                }
            }

            // Continue listening for events and send updated data
            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        seq += 1;
                        log::debug!("[subscriptions] evaluation event received; recomputing evaluations by feature");
                        match fetch_evaluations(&logic, &input, client_id, seq).await {
                            Ok(rows) => {
                                yield Ok(rows);
                            }
                            Err(e) => {
                                yield Err(e.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        Box::pin(stream)
            as std::pin::Pin<
                Box<dyn Stream<Item = GqlResult<Vec<GqlEvaluationByFeatureRow>>> + Send>,
            >
    }

    /// Subscribe to real-time system metrics for dashboard KPIs
    /// Updates every 30 seconds or when evaluations occur
    ///
    /// # Returns
    /// Stream of system-wide metrics including feature count, client counts, evaluation counts, and success rates
    async fn system_metrics(
        &self,
        ctx: &Context<'_>,
    ) -> impl Stream<Item = GqlResult<GqlSystemMetrics>> {
        let feature_logic = match ctx.data::<Box<dyn FeatureLogic>>() {
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature logic not found in context".into())
                }))
                    as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlSystemMetrics>> + Send>>;
            }
        };

        let client_logic = match ctx.data::<Box<dyn ClientLogic>>() {
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Client logic not found in context".into())
                }))
                    as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlSystemMetrics>> + Send>>;
            }
        };

        let evaluation_logic = match ctx.data::<Box<dyn FeatureEvaluationLogic>>() {
            Ok(l) => l.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Feature evaluation logic not found in context".into())
                }))
                    as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlSystemMetrics>> + Send>>;
            }
        };

        let mut events_rx =
            match ctx.data::<tokio::sync::broadcast::Sender<
                crate::logic::feature_evaluation::FeatureEvaluationEvent,
            >>() {
                Ok(tx) => tx.subscribe(),
                Err(_) => {
                    return Box::pin(futures_util::stream::once(async {
                        Err("Evaluation events channel not found".into())
                    }))
                        as std::pin::Pin<
                            Box<dyn Stream<Item = GqlResult<GqlSystemMetrics>> + Send>,
                        >;
                }
            };

        // Helper closure to fetch system metrics
        let fetch_metrics =
            |feature_logic: &Box<dyn FeatureLogic>,
             client_logic: &Box<dyn ClientLogic>,
             evaluation_logic: &Box<dyn FeatureEvaluationLogic>| {
                let feature_logic_clone = feature_logic.clone();
                let client_logic_clone = client_logic.clone();
                let evaluation_logic_clone = evaluation_logic.clone();
                async move {
                    let now = Utc::now();

                    // Calculate time ranges
                    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
                    let today_end = now;
                    let yesterday_start = (now - chrono::Duration::days(1))
                        .date_naive()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc();
                    let yesterday_end = today_start;
                    let (from_7d, to_7d) = calculate_time_range(TimePeriod::D7, now);

                    // Fetch all metrics concurrently
                    let (
                        total_features_result,
                        active_clients_result,
                        total_clients_result,
                        evaluations_today_result,
                        evaluations_yesterday_result,
                        summary_7d_result,
                    ) = tokio::join!(
                        feature_logic_clone.count_features(None),
                        client_logic_clone.count_clients(None, Some(true)),
                        client_logic_clone.count_clients(None, None),
                        evaluation_logic_clone.count_evaluations(
                            today_start,
                            today_end,
                            None,
                            None,
                            None
                        ),
                        evaluation_logic_clone.count_evaluations(
                            yesterday_start,
                            yesterday_end,
                            None,
                            None,
                            None
                        ),
                        evaluation_logic_clone
                            .get_evaluation_summary(None, None, None, from_7d, to_7d)
                    );

                    // Handle results
                    match (
                        total_features_result,
                        active_clients_result,
                        total_clients_result,
                        evaluations_today_result,
                        evaluations_yesterday_result,
                        summary_7d_result,
                    ) {
                        (
                            Ok(total_features),
                            Ok(active_clients),
                            Ok(total_clients),
                            Ok(evaluations_today),
                            Ok(evaluations_yesterday),
                            Ok(summary),
                        ) => Ok(GqlSystemMetrics {
                            total_features,
                            active_clients,
                            total_clients,
                            evaluations_today,
                            evaluations_yesterday,
                            success_rate: round_pct(summary.success_rate),
                            total_evaluations_7d: summary.total_evaluations,
                            successful_evaluations_7d: summary.successful_evaluations,
                            generated_at: now.to_rfc3339(),
                        }),
                        _ => Err("Failed to fetch system metrics".to_string()),
                    }
                }
            };

        let stream = stream! {
            // Send initial data immediately on subscription connect
            match fetch_metrics(&feature_logic, &client_logic, &evaluation_logic).await {
                Ok(metrics) => {
                    yield Ok(metrics);
                }
                Err(e) => {
                    yield Err(e.into());
                }
            }

            // Continue listening for events and send updated data
            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        log::debug!("[subscriptions] evaluation event received; recomputing system metrics");
                        match fetch_metrics(&feature_logic, &client_logic, &evaluation_logic).await {
                            Ok(metrics) => {
                                yield Ok(metrics);
                            }
                            Err(e) => {
                                yield Err(e.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlSystemMetrics>> + Send>>
    }

    /// Subscribe to real-time recent activities for dashboard activity feed
    /// Updates every 45 seconds or when new activities are logged
    ///
    /// # Arguments
    /// * `page_size` - Number of activities to return (default: 10)
    /// * `page_number` - Page number for pagination (default: 1)
    /// * `activity_types` - Optional filter by activity types
    ///
    /// # Returns
    /// Stream of paginated activity log data
    async fn recent_activities(
        &self,
        ctx: &Context<'_>,
        page_size: Option<i32>,
        page_number: Option<i32>,
        activity_types: Option<Vec<String>>,
    ) -> impl Stream<Item = GqlResult<GqlActivityLogPage>> {
        let activity_repo = match ctx.data::<Arc<Box<dyn ActivityLogRepository>>>() {
            Ok(repo) => repo.clone(),
            Err(_) => {
                return Box::pin(futures_util::stream::once(async {
                    Err("Activity log repository not found in context".into())
                }))
                    as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlActivityLogPage>> + Send>>;
            }
        };

        // Set default pagination
        let page_sz = page_size.unwrap_or(10);
        let page_num = page_number.unwrap_or(1);

        // Helper closure to fetch activities
        let fetch_activities =
            |repo: &Arc<Box<dyn ActivityLogRepository>>,
             page_sz: i32,
             page_num: i32,
             activity_types: &Option<Vec<String>>| {
                let repo_clone = repo.clone();
                let activity_types_clone = activity_types.clone();
                async move {
                    let offset = (page_num - 1) * page_sz;

                    let filter = ActivityLogFilter {
                        activity_types: activity_types_clone,
                        entity_type: None,
                        entity_id: None,
                        actor_id: None,
                        from_date: None,
                        to_date: None,
                        limit: Some(page_sz),
                        offset: Some(offset),
                    };

                    match repo_clone.get_activities_paginated(filter).await {
                        Ok((activities, total)) => {
                            let items = activities
                                .into_iter()
                                .map(|a| GqlActivityLog {
                                    id: a.id.to_string(),
                                    activity_type: a.activity_type,
                                    entity_type: a.entity_type,
                                    entity_id: a.entity_id,
                                    actor_id: a.actor_id.map(|id| id.to_string()),
                                    actor_name: a.actor_name,
                                    description: a.description,
                                    metadata: a.metadata,
                                    created_at: a.created_at.to_rfc3339(),
                                })
                                .collect();

                            Ok(GqlActivityLogPage {
                                items,
                                page_number: page_num,
                                page_size: page_sz,
                                total,
                            })
                        }
                        Err(e) => Err(format!("Failed to fetch activities: {}", e)),
                    }
                }
            };

        // Create a stream that updates every 45 seconds
        let stream = stream! {
            // Send initial data immediately
            match fetch_activities(&activity_repo, page_sz, page_num, &activity_types).await {
                Ok(page) => {
                    yield Ok(page);
                }
                Err(e) => {
                    yield Err(e.into());
                    return; // Exit stream on error
                }
            }

            // Update every 45 seconds
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(45));
            interval.tick().await; // First tick completes immediately, skip it

            loop {
                interval.tick().await;
                log::debug!("[subscriptions] recent_activities interval tick; fetching updates");
                match fetch_activities(&activity_repo, page_sz, page_num, &activity_types).await {
                    Ok(page) => {
                        yield Ok(page);
                    }
                    Err(e) => {
                        yield Err(e.into());
                        break; // Exit loop on error
                    }
                }
            }
        };

        Box::pin(stream)
            as std::pin::Pin<Box<dyn Stream<Item = GqlResult<GqlActivityLogPage>> + Send>>
    }

    /// Real-time subscription for feature growth over time
    /// Updates every 60 seconds with time-series data showing feature creation trends
    ///
    /// # Arguments
    /// * `from_time` - Start time for feature growth data
    /// * `to_time` - End time for feature growth data
    /// * `interval` - Time interval: 'day', 'week', or 'month'
    /// * `team_id` - Optional filter by team ID
    ///
    /// # Returns
    /// Stream of feature growth points with time buckets and counts
    async fn feature_growth(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Start time for feature growth data")] from_time: DateTime<Utc>,
        #[graphql(desc = "End time for feature growth data")] to_time: DateTime<Utc>,
        #[graphql(desc = "Time interval: 'day', 'week', or 'month'")] interval: String,
        #[graphql(desc = "Filter by team ID (optional)")] team_id: Option<async_graphql::ID>,
    ) -> impl Stream<Item = GqlResult<Vec<crate::graphql::schema::FeatureGrowthPoint>>> {
        use crate::database::feature::FeatureRepository;
        use std::time::Duration;

        // Get the feature repository from context
        let feature_repo = match ctx.data::<Arc<Box<dyn FeatureRepository>>>() {
            Ok(repo) => repo.clone(),
            Err(e) => {
                return Box::pin(stream! {
                    yield Err(e.into());
                })
                    as std::pin::Pin<
                        Box<
                            dyn Stream<
                                    Item = GqlResult<
                                        Vec<crate::graphql::schema::FeatureGrowthPoint>,
                                    >,
                                > + Send,
                        >,
                    >;
            }
        };

        // Clone values for use in async block
        let from_time_clone = from_time;
        let to_time_clone = to_time;
        let interval_clone = interval.clone();
        let team_id_clone = team_id.clone();

        // Helper closure to fetch feature growth data
        let fetch_growth = move || {
            let repo = feature_repo.clone();
            let interval = interval_clone.clone();
            let team_id = team_id_clone.clone();

            async move {
                // Validate interval
                let valid_intervals = ["day", "week", "month"];
                if !valid_intervals.contains(&interval.as_str()) {
                    return Err(async_graphql::Error::new(
                        "Invalid interval. Must be 'day', 'week', or 'month'",
                    ));
                }

                // Convert team_id from GraphQL ID to UUID if provided
                let team_uuid = if let Some(id) = team_id {
                    Some(uuid::Uuid::parse_str(&id.to_string()).map_err(|e| {
                        async_graphql::Error::new(format!("Invalid team ID format: {}", e))
                    })?)
                } else {
                    None
                };

                // Call the repository method
                let results = repo
                    .get_feature_growth(from_time_clone, to_time_clone, interval.clone(), team_uuid)
                    .await
                    .map_err(|e| async_graphql::Error::new(format!("Database error: {}", e)))?;

                // Convert database results to GraphQL types
                Ok(results
                    .into_iter()
                    .map(|r| crate::graphql::schema::FeatureGrowthPoint {
                        time_bucket: r.time_bucket,
                        team_id: r.team_id.map(|id| id.into()),
                        team_name: r.team_name,
                        feature_count: r.feature_count,
                        cumulative_count: r.cumulative_count,
                    })
                    .collect())
            }
        };

        let stream = stream! {
            // Send initial data immediately
            match fetch_growth().await {
                Ok(growth_data) => yield Ok(growth_data),
                Err(e) => {
                    yield Err(e);
                    return; // Exit on initial error
                }
            }

            // Update every 60 seconds
            let mut interval_timer = tokio::time::interval(Duration::from_secs(60));
            interval_timer.tick().await; // First tick completes immediately

            loop {
                interval_timer.tick().await; // Wait for next interval

                match fetch_growth().await {
                    Ok(growth_data) => yield Ok(growth_data),
                    Err(e) => {
                        yield Err(e);
                        break; // Exit loop on error
                    }
                }
            }
        };

        Box::pin(stream)
            as std::pin::Pin<
                Box<
                    dyn Stream<Item = GqlResult<Vec<crate::graphql::schema::FeatureGrowthPoint>>>
                        + Send,
                >,
            >
    }
}

/// GraphQL output type for activity log entry
#[derive(SimpleObject, Clone)]
pub struct GqlActivityLog {
    pub id: String,
    #[graphql(name = "activityType")]
    pub activity_type: String,
    #[graphql(name = "entityType")]
    pub entity_type: String,
    #[graphql(name = "entityId")]
    pub entity_id: String,
    #[graphql(name = "actorId")]
    pub actor_id: Option<String>,
    #[graphql(name = "actorName")]
    pub actor_name: Option<String>,
    pub description: String,
    pub metadata: Option<serde_json::Value>,
    #[graphql(name = "createdAt")]
    pub created_at: String,
}

/// GraphQL output type for paginated activity log
#[derive(SimpleObject, Clone)]
pub struct GqlActivityLogPage {
    pub items: Vec<GqlActivityLog>,
    #[graphql(name = "pageNumber")]
    pub page_number: i32,
    #[graphql(name = "pageSize")]
    pub page_size: i32,
    pub total: i64,
}

/// GraphQL output type for system metrics
#[derive(SimpleObject, Clone)]
pub struct GqlSystemMetrics {
    #[graphql(name = "totalFeatures")]
    pub total_features: i64,
    #[graphql(name = "activeClients")]
    pub active_clients: i64,
    #[graphql(name = "totalClients")]
    pub total_clients: i64,
    #[graphql(name = "evaluationsToday")]
    pub evaluations_today: i64,
    #[graphql(name = "evaluationsYesterday")]
    pub evaluations_yesterday: i64,
    #[graphql(name = "successRate")]
    pub success_rate: f64,
    #[graphql(name = "totalEvaluations7d")]
    pub total_evaluations_7d: i64,
    #[graphql(name = "successfulEvaluations7d")]
    pub successful_evaluations_7d: i64,
    #[graphql(name = "generatedAt")]
    pub generated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test basic subscription input validation
    #[test]
    fn test_evaluation_rates_input_validation() {
        let now = Utc::now();
        let invalid_input = EvaluationRatesInput {
            feature_key: None,
            environment_id: None,
            client_id: None,
            from_time: now - chrono::Duration::hours(2),
            to_time: now,
            interval_minutes: 0, // invalid
        };
        assert!(invalid_input.interval_minutes < 1);
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
        let now = Utc::now();
        let from = now - chrono::Duration::hours(2);
        let rates_input = EvaluationRatesInput {
            feature_key: Some("test_feature".to_string()),
            environment_id: Some("prod".to_string()),
            client_id: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            from_time: from,
            to_time: now,
            interval_minutes: 5,
        };
        assert_eq!(rates_input.feature_key.as_ref().unwrap(), "test_feature");
        assert_eq!(rates_input.environment_id.as_ref().unwrap(), "prod");
        assert_eq!(rates_input.interval_minutes, 5);
        assert!(rates_input.to_time >= rates_input.from_time);

        let summary_input = EvaluationSummaryInput {
            feature_key: Some("test_feature".to_string()),
            environment_id: Some("prod".to_string()),
            client_id: None,
            period: TimePeriod::H24,
        };
        assert_eq!(summary_input.feature_key.as_ref().unwrap(), "test_feature");
        assert_eq!(summary_input.environment_id.as_ref().unwrap(), "prod");
        assert!(summary_input.client_id.is_none());
    }
}
