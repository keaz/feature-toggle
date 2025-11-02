use async_graphql::{Context, Enum, InputObject, Result as GqlResult, SimpleObject, Subscription};
use async_stream::stream;
use chrono::{DateTime, Utc};
use futures_util::stream::Stream;
use std::pin::Pin;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::database::activity_log::{ActivityLogFilter, ActivityLogRepository};
use crate::database::feature_evaluation::{
    EvaluationByFeature, EvaluationRatePoint, EvaluationSummary,
};
use crate::logic::client::ClientLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::feature_evaluation::{FeatureEvaluationEvent, FeatureEvaluationLogic};
use std::sync::Arc;

/// Typed alias for GraphQL subscription streams returned by resolvers.
type GqlStream<T> = Pin<Box<dyn Stream<Item = GqlResult<T>> + Send>>;

// Helper to round a percentage value (already in 0-100 range) to 2 decimal places
fn round_pct(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Convenience helper to emit a single error event and close the stream early.
fn stream_error<T>(message: impl Into<String>) -> GqlStream<T> {
    let message = message.into();
    Box::pin(futures_util::stream::once(async move {
        Err(async_graphql::Error::new(message))
    }))
}

/// Emit a one-shot stream that propagates an existing GraphQL error.
fn stream_failure<T>(error: async_graphql::Error) -> GqlStream<T> {
    Box::pin(futures_util::stream::once(async move { Err(error) }))
}

/// Parse an optional string UUID field into the typed value used in logic layer.
fn parse_optional_uuid(input: &Option<String>, field_name: &str) -> Result<Option<Uuid>, String> {
    match input.as_ref() {
        Some(raw) => Uuid::parse_str(raw)
            .map(Some)
            .map_err(|_| format!("Invalid {} format", field_name)),
        None => Ok(None),
    }
}

/// Convert domain rate rows into GraphQL-friendly points with rounded percentages.
fn map_rate_points(source: Vec<EvaluationRatePoint>) -> Vec<GqlEvaluationRatePoint> {
    source
        .into_iter()
        .map(|rate| {
            let success_rate = if rate.evaluation_count > 0 {
                (rate.success_count as f64 / rate.evaluation_count as f64) * 100.0
            } else {
                0.0
            };
            let cache_hit_rate = if rate.evaluation_count > 0 {
                (rate.prior_assignment_count as f64 / rate.evaluation_count as f64) * 100.0
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
        .collect()
}

/// Convert aggregated summary data into a GraphQL response using the provided timestamp.
fn map_summary(summary: EvaluationSummary, generated_at: DateTime<Utc>) -> GqlEvaluationSummary {
    GqlEvaluationSummary {
        total_evaluations: summary.total_evaluations,
        successful_evaluations: summary.successful_evaluations,
        cached_evaluations: summary.cached_evaluations,
        unique_users: summary.unique_users,
        top_feature_key: summary.top_feature_key,
        success_rate: round_pct(summary.success_rate),
        cache_hit_rate: round_pct(summary.cache_hit_rate),
        generated_at: generated_at.to_rfc3339(),
    }
}

/// Clamp a timestamp so we never query for data in the future.
fn clamp_to_now(target: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    if target > now { now } else { target }
}

/// Fetch the evaluation logic dependency from the request context.
fn get_evaluation_logic(
    ctx: &Context<'_>,
) -> Result<Box<dyn FeatureEvaluationLogic>, async_graphql::Error> {
    ctx.data::<Box<dyn FeatureEvaluationLogic>>()
        .map(|logic| logic.clone())
        .map_err(|_| async_graphql::Error::new("Feature evaluation logic not found in context"))
}

/// Subscribe to the broadcast channel that emits feature evaluation events.
fn evaluation_events_receiver(
    ctx: &Context<'_>,
) -> Result<broadcast::Receiver<FeatureEvaluationEvent>, async_graphql::Error> {
    ctx.data::<tokio::sync::broadcast::Sender<FeatureEvaluationEvent>>()
        .map(|tx| tx.subscribe())
        .map_err(|_| async_graphql::Error::new("Evaluation events channel not found"))
}

/// Invoke the logic layer to load evaluation rates and format for GraphQL clients.
async fn load_rates(
    logic: &Box<dyn FeatureEvaluationLogic>,
    feature_key: Option<String>,
    environment_id: Option<String>,
    client_id: Option<Uuid>,
    from_time: DateTime<Utc>,
    to_time: DateTime<Utc>,
    interval_minutes: i32,
) -> Result<Vec<GqlEvaluationRatePoint>, String> {
    logic
        .get_evaluation_rates(
            feature_key,
            environment_id,
            client_id,
            from_time,
            to_time,
            interval_minutes,
        )
        .await
        .map(map_rate_points)
        .map_err(|e| format!("Failed to get evaluation rates: {}", e))
}

/// Load evaluation summary aggregates and convert to GraphQL response.
async fn load_summary(
    logic: &Box<dyn FeatureEvaluationLogic>,
    feature_key: Option<String>,
    environment_id: Option<String>,
    client_id: Option<Uuid>,
    from_time: DateTime<Utc>,
    to_time: DateTime<Utc>,
    generated_at: DateTime<Utc>,
) -> Result<GqlEvaluationSummary, String> {
    logic
        .get_evaluation_summary(feature_key, environment_id, client_id, from_time, to_time)
        .await
        .map(|summary| map_summary(summary, generated_at))
        .map_err(|e| format!("Failed to get evaluation summary: {}", e))
}

/// Aggregate both rates and summary for the dashboard view in one round-trip.
async fn load_dashboard_data(
    logic: &Box<dyn FeatureEvaluationLogic>,
    input: &EvaluationRatesInput,
    client_id: Option<Uuid>,
    from_time: DateTime<Utc>,
    to_time: DateTime<Utc>,
    generated_at: DateTime<Utc>,
) -> Result<GqlEvaluationDashboardData, String> {
    let (rates_result, summary_result) = tokio::join!(
        logic.get_evaluation_rates(
            input.feature_key.clone(),
            input.environment_id.clone(),
            client_id,
            from_time,
            to_time,
            input.interval_minutes,
        ),
        logic.get_evaluation_summary(
            input.feature_key.clone(),
            input.environment_id.clone(),
            client_id,
            from_time,
            to_time,
        )
    );

    match (rates_result, summary_result) {
        (Ok(rates), Ok(summary)) => Ok(GqlEvaluationDashboardData {
            rates: map_rate_points(rates),
            summary: map_summary(summary, generated_at),
            generated_at: generated_at.to_rfc3339(),
        }),
        (Err(e), _) | (_, Err(e)) => Err(format!("Failed to get evaluation dashboard data: {}", e)),
    }
}

/// Pull the top-features aggregation and adapt it for GraphQL.
async fn load_evaluations_by_feature(
    logic: &Box<dyn FeatureEvaluationLogic>,
    input: &EvaluationsByFeatureLiveInput,
    client_id: Option<Uuid>,
    sequence: i64,
    emitted_at: DateTime<Utc>,
) -> Result<Vec<GqlEvaluationByFeatureRow>, String> {
    let (from_time, to_time) = calculate_time_range(input.period, emitted_at);
    logic
        .get_evaluations_by_feature(
            from_time,
            to_time,
            input.environment_id.clone(),
            client_id,
            input.limit,
            input.offset,
        )
        .await
        .map(|rows| {
            let emitted_at = emitted_at.to_rfc3339();
            rows.into_iter()
                .map(|row: EvaluationByFeature| GqlEvaluationByFeatureRow {
                    feature_key: row.feature_key,
                    total_evaluations: row.total_evaluations,
                    successful_evaluations: row.successful_evaluations,
                    cached_evaluations: row.cached_evaluations,
                    unique_users: row.unique_users,
                    last_evaluated_at: row.last_evaluated_at.to_rfc3339(),
                    sequence,
                    emitted_at: emitted_at.clone(),
                })
                .collect()
        })
        .map_err(|e| format!("Failed to get evaluationsByFeature: {}", e))
}

/// Compute high-level system metrics used on the dashboard summary cards.
async fn load_system_metrics(
    feature_logic: &Box<dyn FeatureLogic>,
    client_logic: &Box<dyn ClientLogic>,
    evaluation_logic: &Box<dyn FeatureEvaluationLogic>,
) -> Result<GqlSystemMetrics, String> {
    let now = Utc::now();

    let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let today_end = now;
    let yesterday_start = (now - chrono::Duration::days(1))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let yesterday_end = today_start;
    let (from_7d, to_7d) = calculate_time_range(TimePeriod::D7, now);

    let feature_logic_clone = feature_logic.clone();
    let client_logic_active = client_logic.clone();
    let client_logic_total = client_logic.clone();
    let evaluation_logic_today = evaluation_logic.clone();
    let evaluation_logic_yesterday = evaluation_logic.clone();
    let evaluation_logic_summary = evaluation_logic.clone();

    let (
        total_features_result,
        active_clients_result,
        total_clients_result,
        evaluations_today_result,
        evaluations_yesterday_result,
        summary_7d_result,
    ) = tokio::join!(
        feature_logic_clone.count_features(None),
        client_logic_active.count_clients(None, Some(true)),
        client_logic_total.count_clients(None, None),
        evaluation_logic_today.count_evaluations(today_start, today_end, None, None, None),
        evaluation_logic_yesterday.count_evaluations(
            yesterday_start,
            yesterday_end,
            None,
            None,
            None
        ),
        evaluation_logic_summary.get_evaluation_summary(None, None, None, from_7d, to_7d)
    );

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
    ) -> GqlStream<Vec<GqlEvaluationRatePoint>> {
        if !(1..=60).contains(&input.interval_minutes) {
            return stream_error("Interval must be between 1 and 60 minutes");
        }

        let client_id = match parse_optional_uuid(&input.client_id, "client ID") {
            Ok(id) => id,
            Err(err) => return stream_error(err),
        };

        let logic = match get_evaluation_logic(ctx) {
            Ok(logic) => logic,
            Err(err) => return stream_failure(err),
        };

        let mut events_rx = match evaluation_events_receiver(ctx) {
            Ok(rx) => rx,
            Err(err) => return stream_failure(err),
        };

        let stream = stream! {
            let now = Utc::now();
            let (from_time, to_time) = calculate_time_range(input.period, now);
            match load_rates(
                &logic,
                input.feature_key.clone(),
                input.environment_id.clone(),
                client_id,
                from_time,
                to_time,
                input.interval_minutes,
            ).await {
                Ok(rates) => yield Ok(rates),
                Err(err) => yield Err(err.into()),
            }

            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        log::debug!("[subscriptions] evaluation event received; recomputing aggregation");
                        let now = Utc::now();
                        let (from_time, to_time) = calculate_time_range(input.period, now);
                        match load_rates(
                            &logic,
                            input.feature_key.clone(),
                            input.environment_id.clone(),
                            client_id,
                            from_time,
                            to_time,
                            input.interval_minutes,
                        ).await {
                            Ok(rates) => yield Ok(rates),
                            Err(err) => yield Err(err.into()),
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
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
    ) -> GqlStream<Vec<GqlEvaluationRatePoint>> {
        if !(1..=60).contains(&input.interval_minutes) {
            return stream_error("Interval must be between 1 and 60 minutes");
        }
        if input.to_time < input.from_time {
            return stream_error("toTime must be >= fromTime");
        }
        let duration_hours = (input.to_time - input.from_time).num_hours();
        if duration_hours > 24 {
            return stream_error("Time range cannot exceed 24 hours");
        }

        let client_id = match parse_optional_uuid(&input.client_id, "client ID") {
            Ok(id) => id,
            Err(err) => return stream_error(err),
        };

        let logic = match get_evaluation_logic(ctx) {
            Ok(logic) => logic,
            Err(err) => return stream_failure(err),
        };

        let mut events_rx = match evaluation_events_receiver(ctx) {
            Ok(rx) => rx,
            Err(err) => return stream_failure(err),
        };

        let stream = stream! {
            let now = Utc::now();
            let upper = clamp_to_now(input.to_time, now);
            match load_rates(
                &logic,
                input.feature_key.clone(),
                input.environment_id.clone(),
                client_id,
                input.from_time,
                upper,
                input.interval_minutes,
            ).await {
                Ok(rates) => yield Ok(rates),
                Err(err) => yield Err(err.into()),
            }

            loop {
                match events_rx.recv().await {
                    Ok(_evt) => {
                        let now = Utc::now();
                        let upper = clamp_to_now(input.to_time, now);
                        match load_rates(
                            &logic,
                            input.feature_key.clone(),
                            input.environment_id.clone(),
                            client_id,
                            input.from_time,
                            upper,
                            input.interval_minutes,
                        ).await {
                            Ok(rates) => yield Ok(rates),
                            Err(err) => yield Err(err.into()),
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        Box::pin(stream)
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
    ) -> GqlStream<GqlEvaluationSummary> {
        let client_id = match parse_optional_uuid(&input.client_id, "client ID") {
            Ok(id) => id,
            Err(err) => return stream_error(err),
        };

        let logic = match get_evaluation_logic(ctx) {
            Ok(logic) => logic,
            Err(err) => return stream_failure(err),
        };

        let mut events_rx = match evaluation_events_receiver(ctx) {
            Ok(rx) => rx,
            Err(err) => return stream_failure(err),
        };

        let stream = stream! {
            let now = Utc::now();
            let (from_time, to_time) = calculate_time_range(input.period, now);
            match load_summary(
                &logic,
                input.feature_key.clone(),
                input.environment_id.clone(),
                client_id,
                from_time,
                to_time,
                now,
            ).await {
                Ok(summary) => yield Ok(summary),
                Err(err) => yield Err(err.into()),
            }

            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        log::debug!("[subscriptions] evaluation event received; recomputing aggregation");
                        let now = Utc::now();
                        let (from_time, to_time) = calculate_time_range(input.period, now);
                        match load_summary(
                            &logic,
                            input.feature_key.clone(),
                            input.environment_id.clone(),
                            client_id,
                            from_time,
                            to_time,
                            now,
                        ).await {
                            Ok(summary) => yield Ok(summary),
                            Err(err) => yield Err(err.into()),
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
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
    ) -> GqlStream<GqlEvaluationDashboardData> {
        if input.interval_minutes < 1 || input.interval_minutes > 60 {
            return stream_error("Interval must be between 1 and 60 minutes");
        }
        if input.to_time < input.from_time {
            return stream_error("toTime must be >= fromTime");
        }
        if (input.to_time - input.from_time).num_hours() > 24 {
            return stream_error("Time range cannot exceed 24 hours");
        }

        let client_id = match parse_optional_uuid(&input.client_id, "client ID") {
            Ok(id) => id,
            Err(err) => return stream_error(err),
        };

        let logic = match get_evaluation_logic(ctx) {
            Ok(logic) => logic,
            Err(err) => return stream_failure(err),
        };

        let mut events_rx = match evaluation_events_receiver(ctx) {
            Ok(rx) => rx,
            Err(err) => return stream_failure(err),
        };

        let stream = stream! {
            let now = Utc::now();
            let upper = clamp_to_now(input.to_time, now);
            match load_dashboard_data(
                &logic,
                &input,
                client_id,
                input.from_time,
                upper,
                now,
            ).await {
                Ok(data) => yield Ok(data),
                Err(err) => yield Err(err.into()),
            }

            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        let now = Utc::now();
                        let upper = clamp_to_now(input.to_time, now);
                        match load_dashboard_data(
                            &logic,
                            &input,
                            client_id,
                            input.from_time,
                            upper,
                            now,
                        ).await {
                            Ok(data) => yield Ok(data),
                            Err(err) => yield Err(err.into()),
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
    }

    /// Live stream of evaluations grouped by feature ("Top Features")
    /// Emits on each evaluation event with updated aggregation.
    async fn evaluations_by_feature_live(
        &self,
        ctx: &Context<'_>,
        input: EvaluationsByFeatureLiveInput,
    ) -> GqlStream<Vec<GqlEvaluationByFeatureRow>> {
        let client_id = match parse_optional_uuid(&input.client_id, "client ID") {
            Ok(id) => id,
            Err(err) => return stream_error(err),
        };

        let logic = match get_evaluation_logic(ctx) {
            Ok(logic) => logic,
            Err(err) => return stream_failure(err),
        };

        let mut events_rx = match evaluation_events_receiver(ctx) {
            Ok(rx) => rx,
            Err(err) => return stream_failure(err),
        };

        let mut seq: i64 = 0;
        let stream = stream! {
            let emitted_at = Utc::now();
            match load_evaluations_by_feature(&logic, &input, client_id, seq, emitted_at).await {
                Ok(rows) => {
                    log::debug!(
                        "[subscriptions] evaluations_by_feature_live sending {} rows (seq={})",
                        rows.len(),
                        seq
                    );
                    yield Ok(rows);
                }
                Err(err) => {
                    yield Err(err.into());
                }
            }

            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        seq += 1;
                        log::debug!("[subscriptions] evaluation event received; recomputing evaluations by feature");
                        let emitted_at = Utc::now();
                        match load_evaluations_by_feature(&logic, &input, client_id, seq, emitted_at).await {
                            Ok(rows) => {
                                log::debug!(
                                    "[subscriptions] evaluations_by_feature_live sending {} rows (seq={})",
                                    rows.len(),
                                    seq
                                );
                                yield Ok(rows);
                            }
                            Err(err) => {
                                yield Err(err.into());
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        Box::pin(stream)
    }

    /// Subscribe to real-time system metrics for dashboard KPIs
    /// Updates every 30 seconds or when evaluations occur
    ///
    /// # Returns
    /// Stream of system-wide metrics including feature count, client counts, evaluation counts, and success rates
    async fn system_metrics(&self, ctx: &Context<'_>) -> GqlStream<GqlSystemMetrics> {
        let feature_logic = match ctx.data::<Box<dyn FeatureLogic>>() {
            Ok(l) => l.clone(),
            Err(_) => {
                return stream_error("Feature logic not found in context");
            }
        };

        let client_logic = match ctx.data::<Box<dyn ClientLogic>>() {
            Ok(l) => l.clone(),
            Err(_) => {
                return stream_error("Client logic not found in context");
            }
        };

        let evaluation_logic = match get_evaluation_logic(ctx) {
            Ok(logic) => logic,
            Err(err) => return stream_failure(err),
        };

        let mut events_rx = match evaluation_events_receiver(ctx) {
            Ok(rx) => rx,
            Err(err) => return stream_failure(err),
        };

        let stream = stream! {
            match load_system_metrics(&feature_logic, &client_logic, &evaluation_logic).await {
                Ok(metrics) => yield Ok(metrics),
                Err(err) => yield Err(err.into()),
            }

            loop {
                match events_rx.recv().await {
                    Ok(_) => {
                        log::debug!("[subscriptions] evaluation event received; recomputing system metrics");
                        match load_system_metrics(&feature_logic, &client_logic, &evaluation_logic).await {
                            Ok(metrics) => yield Ok(metrics),
                            Err(err) => yield Err(err.into()),
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Box::pin(stream)
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
    ) -> GqlStream<GqlActivityLogPage> {
        let activity_repo = match ctx.data::<Arc<Box<dyn ActivityLogRepository>>>() {
            Ok(repo) => repo.clone(),
            Err(_) => {
                return stream_error("Activity log repository not found in context");
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
    ) -> GqlStream<Vec<crate::graphql::schema::FeatureGrowthPoint>> {
        use crate::database::feature::FeatureRepository;
        use std::time::Duration;

        // Get the feature repository from context
        let feature_repo = match ctx.data::<Arc<Box<dyn FeatureRepository>>>() {
            Ok(repo) => repo.clone(),
            Err(e) => return stream_failure(e.into()),
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
