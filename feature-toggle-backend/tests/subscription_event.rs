use async_graphql::{Object, Request, Schema};
use chrono::{Duration, Utc};
use feature_toggle_backend::database::feature_evaluation::{
    CreateFeatureEvaluation, EvaluationRatePoint, EvaluationSummary, FeatureEvaluationRow,
};
use feature_toggle_backend::graphql::subscription::FeatureEvaluationSubscription;
use feature_toggle_backend::logic::feature_evaluation::{
    FeatureEvaluationEvent, FeatureEvaluationLogic, FeatureEvaluationLogicError,
};
use futures_util::StreamExt;
use uuid::Uuid;

// Helper builds schema with injected mock logic and broadcast sender
// Minimal stub logic implementing only needed methods for subscription test
struct StubLogic {
    rates: Vec<EvaluationRatePoint>,
    summary: EvaluationSummary,
}

#[async_trait::async_trait]
impl FeatureEvaluationLogic for StubLogic {
    async fn record_evaluation(
        &self,
        _: String,
        _: String,
        _: uuid::Uuid,
        _: chrono::DateTime<Utc>,
        _: bool,
        _: Option<serde_json::Value>,
        _: Option<String>,
        _: bool,
    ) -> Result<FeatureEvaluationRow, FeatureEvaluationLogicError> {
        Err(FeatureEvaluationLogicError::InvalidInput("not used".into()))
    }
    async fn record_evaluations_bulk(
        &self,
        _: Vec<CreateFeatureEvaluation>,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError> {
        Ok(vec![])
    }
    async fn get_evaluations(
        &self,
        _: feature_toggle_backend::database::feature_evaluation::FeatureEvaluationFilter,
    ) -> Result<Vec<FeatureEvaluationRow>, FeatureEvaluationLogicError> {
        Ok(vec![])
    }
    async fn get_evaluation_count(
        &self,
        _: feature_toggle_backend::database::feature_evaluation::FeatureEvaluationFilter,
    ) -> Result<i64, FeatureEvaluationLogicError> {
        Ok(0)
    }
    async fn get_evaluation_rates(
        &self,
        _: Option<String>,
        _: Option<String>,
        _: Option<uuid::Uuid>,
        _: Option<uuid::Uuid>,
        _: chrono::DateTime<Utc>,
        _: chrono::DateTime<Utc>,
        _: i32,
    ) -> Result<Vec<EvaluationRatePoint>, FeatureEvaluationLogicError> {
        Ok(self.rates.clone())
    }
    async fn get_evaluation_summary(
        &self,
        _: Option<String>,
        _: Option<String>,
        _: Option<uuid::Uuid>,
        _: Option<uuid::Uuid>,
        _: chrono::DateTime<Utc>,
        _: chrono::DateTime<Utc>,
    ) -> Result<EvaluationSummary, FeatureEvaluationLogicError> {
        Ok(self.summary.clone())
    }
    async fn count_evaluations(
        &self,
        _: chrono::DateTime<Utc>,
        _: chrono::DateTime<Utc>,
        _: Option<String>,
        _: Option<uuid::Uuid>,
        _: Option<String>,
        _: Option<uuid::Uuid>,
    ) -> Result<i64, FeatureEvaluationLogicError> {
        Ok(0)
    }
    async fn get_evaluations_by_feature(
        &self,
        _: chrono::DateTime<Utc>,
        _: chrono::DateTime<Utc>,
        _: Option<String>,
        _: Option<uuid::Uuid>,
        _: Option<uuid::Uuid>,
        _: Option<i32>,
        _: Option<i32>,
    ) -> Result<
        Vec<feature_toggle_backend::database::feature_evaluation::EvaluationByFeature>,
        FeatureEvaluationLogicError,
    > {
        Ok(vec![])
    }
    fn clone_box(&self) -> Box<dyn FeatureEvaluationLogic> {
        Box::new(StubLogic {
            rates: self.rates.clone(),
            summary: self.summary.clone(),
        })
    }
}

struct RootQuery;

#[Object]
impl RootQuery {
    async fn ping(&self) -> &str {
        "pong"
    }
}

struct RootMutation;

#[Object]
impl RootMutation {
    async fn noop(&self) -> bool {
        true
    }
}

fn build_schema(
    stub_logic: StubLogic,
    sender: tokio::sync::broadcast::Sender<FeatureEvaluationEvent>,
) -> Schema<RootQuery, RootMutation, FeatureEvaluationSubscription> {
    Schema::build(RootQuery, RootMutation, FeatureEvaluationSubscription)
        .data(Box::new(stub_logic) as Box<dyn FeatureEvaluationLogic>)
        .data(sender)
        .finish()
}

#[tokio::test]
async fn subscription_emits_on_event() {
    // Arrange: broadcast channel
    let (tx, _rx) = tokio::sync::broadcast::channel::<FeatureEvaluationEvent>(16);

    // Time window for test
    let now = Utc::now();
    let from = now - Duration::minutes(5);

    // Prepare mock logic
    let stub_logic = StubLogic {
        rates: vec![EvaluationRatePoint {
            time_bucket: Utc::now(),
            evaluation_count: 10,
            success_count: 7,
            prior_assignment_count: 3,
        }],
        summary: EvaluationSummary {
            total_evaluations: 10,
            successful_evaluations: 7,
            cached_evaluations: 3,
            unique_users: 1,
            top_feature_key: None,
            success_rate: 70.0,
            cache_hit_rate: 30.0,
        },
    };

    // Build schema
    let schema = build_schema(stub_logic, tx.clone());

    // (Input object built inline below)

    // Execute subscription manually using schema.execute_stream
    let mut stream = schema.execute_stream({
        let mut req = Request::new("subscription($input: EvaluationRatesInput!){ evaluationRates(input:$input){ timeBucket evaluationCount successCount priorAssignmentCount successRate cacheHitRate } }");
    let mut outer: async_graphql::indexmap::IndexMap<async_graphql::Name, async_graphql::Value> = Default::default();
    let mut input_obj: async_graphql::indexmap::IndexMap<async_graphql::Name, async_graphql::Value> = Default::default();
    input_obj.insert(async_graphql::Name::new("featureKey"), async_graphql::Value::Null);
    input_obj.insert(async_graphql::Name::new("environmentId"), async_graphql::Value::Null);
    input_obj.insert(async_graphql::Name::new("clientId"), async_graphql::Value::Null);
    input_obj.insert(async_graphql::Name::new("fromTime"), async_graphql::Value::String(from.to_rfc3339()));
    input_obj.insert(async_graphql::Name::new("toTime"), async_graphql::Value::String(now.to_rfc3339()));
    input_obj.insert(async_graphql::Name::new("intervalMinutes"), async_graphql::Value::Number((1).into()));
    outer.insert(async_graphql::Name::new("input"), async_graphql::Value::Object(input_obj));
        req = req.variables(async_graphql::Variables::from_value(async_graphql::Value::Object(outer)));
        req
    });

    // First, consume the initial response (sent immediately on subscription)
    if let Some(initial_resp) = stream.next().await {
        // Initial response should contain the stub data
        if let Some(data) = initial_resp.data.into_json().ok() {
            if let Some(rates) = data.get("evaluationRates") {
                if rates.is_array() {
                    // Initial response received successfully
                }
            }
        }
    }

    // Act: send an event
    let _ = tx.send(FeatureEvaluationEvent {
        event_id: Uuid::new_v4(),
        feature_key: "f".into(),
        environment_id: "env".into(),
        client_id: Uuid::new_v4(),
        evaluated_at: Utc::now(),
        evaluation_result: true,
        prior_assignment: false,
        user_context: None,
    });

    // Assert: next response (after the event) contains data array with one rate point
    let mut found = false;
    for _ in 0..5 {
        // limit attempts
        if let Some(resp) = stream.next().await {
            if let Some(data) = resp.data.into_json().ok() {
                if let Some(rates) = data.get("evaluationRates") {
                    if rates.is_array() && !rates.as_array().unwrap().is_empty() {
                        let first = &rates.as_array().unwrap()[0];
                        assert_eq!(first.get("evaluationCount").unwrap().as_i64().unwrap(), 10);
                        assert_eq!(first.get("successCount").unwrap().as_i64().unwrap(), 7);
                        assert_eq!(
                            first.get("priorAssignmentCount").unwrap().as_i64().unwrap(),
                            3
                        );
                        found = true;
                        break;
                    }
                }
            }
        }
        // brief yield
        tokio::task::yield_now().await;
    }
    assert!(found, "Did not receive rates emission after event");
}
