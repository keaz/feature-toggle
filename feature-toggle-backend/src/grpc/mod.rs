mod ingest;

pub mod pb {
    tonic::include_proto!("featuretoggle");
}

use crate::database::entity as db;
use crate::logic::metrics::{MetricLogic, MetricLogicError, TrackMetricInput};
use chrono::{DateTime, Utc};
use evaluation_engine as engine;
use futures_util::StreamExt;
use ingest::{
    EVALUATION_DURABILITY_ACK_TIMEOUT, EVALUATION_WRITER_QUEUE_CAPACITY, EvaluationWriteJob,
    PUSH_EVALUATION_DEDUP_MAX_ENTRIES, PUSH_EVALUATION_DEDUP_TTL, PushEvaluationRequestContext,
    PushEvaluationRequestDeduper, RequestedKeyMap, push_evaluation_request_fingerprint,
};
use pb::feature_evaluation_server::{FeatureEvaluation, FeatureEvaluationServer};
use pb::{EvaluateRequest, EvaluateResponse};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::codec::CompressionEncoding;
use tonic::{Request, Response, Status};
use uuid::Uuid;

pub use pb::feature_evaluation_server;
// re-export for server creation

// Minimal no-op implementation of UserFlagLogic to avoid cloning mocked repositories
struct NoopUserFlagLogic;

#[async_trait::async_trait]
impl crate::logic::user_flag::UserFlagLogic for NoopUserFlagLogic {
    async fn authenticate_client(
        &self,
        _client_id: &str,
        _client_secret: &str,
    ) -> Result<uuid::Uuid, crate::logic::user_flag::UserFlagLogicError> {
        Err(crate::logic::user_flag::UserFlagLogicError::InvalidInput(
            "user flag logic not available".into(),
        ))
    }
    async fn upsert_after_auth(
        &self,
        _user_id: &str,
        _feature_id: &str,
        _environment_id: &str,
        _assigned: bool,
        _variant: Option<String>,
    ) -> Result<(), crate::logic::user_flag::UserFlagLogicError> {
        Ok(())
    }
    async fn list_user_assignments(
        &self,
        _team_id: uuid::Uuid,
        _feature_id: Option<String>,
        _environment_id: Option<String>,
    ) -> Result<
        Vec<crate::database::user_flag_assignment::UserFlagAssignmentRow>,
        crate::logic::user_flag::UserFlagLogicError,
    > {
        Ok(Vec::new())
    }
    fn clone_box(&self) -> Box<dyn crate::logic::user_flag::UserFlagLogic> {
        Box::new(NoopUserFlagLogic)
    }
}

// Minimal no-op MetricLogic for tests that inject mocked repos
struct NoopMetricLogic;

#[async_trait::async_trait]
impl crate::logic::metrics::MetricLogic for NoopMetricLogic {
    async fn create_metric(
        &self,
        _team_id: Uuid,
        _key: String,
        _name: String,
        _description: Option<String>,
        _metric_type: crate::database::metrics::MetricType,
        _unit: Option<String>,
        _success_criteria: Option<serde_json::Value>,
    ) -> Result<crate::database::metrics::MetricRow, MetricLogicError> {
        Err(MetricLogicError::PermissionDenied(
            "metric creation not available in noop logic".into(),
        ))
    }

    async fn track_metrics(
        &self,
        _client_id: &str,
        _client_secret: &str,
        _events: Vec<TrackMetricInput>,
    ) -> Result<usize, MetricLogicError> {
        Ok(0)
    }

    async fn aggregate_metrics(
        &self,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
        _bucket: &str,
    ) -> Result<u64, MetricLogicError> {
        Ok(0)
    }

    async fn get_metric_results(
        &self,
        _feature_key: &str,
        _team_id: Option<Uuid>,
        _environment_id: Option<Uuid>,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
    ) -> Result<Vec<crate::database::metrics::MetricAggregationRow>, MetricLogicError> {
        Ok(vec![])
    }

    async fn list_metrics(
        &self,
        _team_id: Uuid,
    ) -> Result<Vec<crate::database::metrics::MetricRow>, MetricLogicError> {
        Ok(vec![])
    }

    fn clone_box(&self) -> Box<dyn crate::logic::metrics::MetricLogic> {
        Box::new(NoopMetricLogic)
    }
}

type ActiveSubscriptionMap =
    std::collections::HashMap<Uuid, std::collections::HashMap<Uuid, SubscriptionFilter>>;

#[derive(Clone, Debug)]
enum SubscriptionFilter {
    /// The stream receives all feature updates for the authenticated team.
    AllFeatures,
    /// The stream receives updates only for a concrete key set.
    Keys(std::collections::HashSet<String>),
}

impl SubscriptionFilter {
    fn from_feature_keys(feature_keys: Vec<String>) -> Self {
        if feature_keys.is_empty() {
            Self::AllFeatures
        } else {
            Self::Keys(feature_keys.into_iter().collect())
        }
    }

    fn matches(&self, feature_key: &str) -> bool {
        match self {
            Self::AllFeatures => true,
            Self::Keys(keys) => keys.contains(feature_key),
        }
    }

    fn snapshot_keys(
        &self,
        requested_keys: &std::collections::HashSet<String>,
    ) -> Option<std::collections::HashSet<String>> {
        match self {
            Self::AllFeatures => None,
            Self::Keys(keys) => {
                let mut combined = keys.clone();
                combined.extend(requested_keys.iter().cloned());
                Some(combined)
            }
        }
    }
}

async fn unregister_stream_subscription(
    requested_keys: &std::sync::Arc<tokio::sync::RwLock<RequestedKeyMap>>,
    subscriptions: &std::sync::Arc<tokio::sync::RwLock<ActiveSubscriptionMap>>,
    client_id: Uuid,
    stream_id: Uuid,
) {
    let mut map = subscriptions.write().await;
    if let Some(per_client) = map.get_mut(&client_id) {
        per_client.remove(&stream_id);
        if per_client.is_empty() {
            map.remove(&client_id);
            drop(map);
            requested_keys.write().await.remove(&client_id);
        }
    }
}

async fn stream_allows_feature(
    requested_keys: &std::sync::Arc<tokio::sync::RwLock<RequestedKeyMap>>,
    subscriptions: &std::sync::Arc<tokio::sync::RwLock<ActiveSubscriptionMap>>,
    client_id: Uuid,
    stream_id: Uuid,
    feature_key: &str,
) -> bool {
    let requested_hit = {
        let map = requested_keys.read().await;
        map.get(&client_id)
            .map(|keys| keys.contains(feature_key))
            .unwrap_or(false)
    };
    if requested_hit {
        return true;
    }

    let map = subscriptions.read().await;
    map.get(&client_id)
        .and_then(|per_client| per_client.get(&stream_id))
        .map(|filter| filter.matches(feature_key))
        .unwrap_or(false)
}

#[derive(Clone)]
struct EngineFeatureBase {
    id: String,
    key: String,
    feature_type: String,
    active: bool,
    enabled: bool,
    dependency_ids: Vec<Uuid>,
    stages: Vec<engine::FeatureStage>,
    variants: Vec<engine::FeatureVariant>,
}

pub struct FeatureEvaluationSvc {
    pool: sqlx::PgPool,
    feature_repo: Box<dyn crate::database::feature::FeatureRepository>,
    client_repo: Box<dyn crate::database::client::ClientRepository>,
    user_flag_repo: Box<dyn crate::database::user_flag_assignment::UserFlagAssignmentRepository>,
    user_flag_logic: Box<dyn crate::logic::user_flag::UserFlagLogic>,
    feature_evaluation_logic: Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>,
    metric_logic: Box<dyn MetricLogic>,
    updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
    // Async database writer channel - sends evaluation batches to background task
    evaluation_writer_tx: tokio::sync::mpsc::Sender<EvaluationWriteJob>,
    // Short-lived payload dedupe for retry-safe evaluation ingestion.
    push_evaluation_deduper: std::sync::Arc<PushEvaluationRequestDeduper>,
    // Tracks, per client_id, keys explicitly requested via GetFeatureByKeyRequest.
    requested_keys: std::sync::Arc<tokio::sync::RwLock<RequestedKeyMap>>,
    // Tracks active stream subscriptions per (client_id, stream_id).
    active_subscriptions: std::sync::Arc<tokio::sync::RwLock<ActiveSubscriptionMap>>,
}

impl Clone for FeatureEvaluationSvc {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            feature_repo: self.feature_repo.clone_box(),
            client_repo: self.client_repo.clone_box(),
            user_flag_repo: self.user_flag_repo.clone_box(),
            user_flag_logic: self.user_flag_logic.clone_box(),
            feature_evaluation_logic: self.feature_evaluation_logic.clone_box(),
            metric_logic: self.metric_logic.clone_box(),
            updates_tx: self.updates_tx.clone(),
            evaluation_writer_tx: self.evaluation_writer_tx.clone(),
            push_evaluation_deduper: self.push_evaluation_deduper.clone(),
            requested_keys: self.requested_keys.clone(),
            active_subscriptions: self.active_subscriptions.clone(),
        }
    }
}

fn map_metric_error(err: MetricLogicError) -> Status {
    match err {
        MetricLogicError::InvalidInput(msg) => Status::invalid_argument(msg),
        MetricLogicError::NotFound(msg) => Status::not_found(msg),
        MetricLogicError::RecordAlreadyExists(msg) => Status::already_exists(msg),
        MetricLogicError::Unauthenticated(msg) => Status::unauthenticated(msg),
        MetricLogicError::PermissionDenied(msg) => Status::permission_denied(msg),
        MetricLogicError::Database(e) => Status::internal(format!("db error: {e}")),
    }
}

impl FeatureEvaluationSvc {
    pub fn new(
        pool: sqlx::PgPool,
        updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
        evaluation_events_tx: tokio::sync::broadcast::Sender<
            crate::logic::feature_evaluation::FeatureEvaluationEvent,
        >,
    ) -> Self {
        let feature_repo = crate::database::feature::feature_repository(pool.clone());
        let client_repo = crate::database::client::client_repository(pool.clone());
        let user_flag_repo =
            crate::database::user_flag_assignment::user_flag_assignment_repository(pool.clone());
        let user_flag_logic =
            crate::logic::user_flag::user_flag_logic(client_repo.clone(), user_flag_repo.clone());
        let feature_evaluation_repo =
            crate::database::feature_evaluation::feature_evaluation_repository(pool.clone());
        // Use event-enabled logic so gRPC ingested evaluations broadcast to REST stream consumers.
        let feature_evaluation_logic =
            crate::logic::feature_evaluation::feature_evaluation_logic_with_events(
                feature_evaluation_repo,
                evaluation_events_tx.clone(),
            );
        let metric_repo = crate::database::metrics::metric_repository(pool.clone());
        let metric_logic =
            crate::logic::metrics::metric_logic(metric_repo, client_repo.clone_box());

        // Create mpsc channel for async database writes
        let (evaluation_writer_tx, evaluation_writer_rx) =
            tokio::sync::mpsc::channel::<EvaluationWriteJob>(EVALUATION_WRITER_QUEUE_CAPACITY);

        // Spawn background task to handle database writes
        let logic_clone = feature_evaluation_logic.clone_box();
        tokio::spawn(async move {
            Self::run_evaluation_writer(logic_clone, evaluation_writer_rx).await;
        });

        Self {
            pool,
            feature_repo,
            client_repo,
            user_flag_repo,
            user_flag_logic,
            feature_evaluation_logic,
            metric_logic,
            updates_tx,
            evaluation_writer_tx,
            push_evaluation_deduper: std::sync::Arc::new(PushEvaluationRequestDeduper::new(
                PUSH_EVALUATION_DEDUP_TTL,
                PUSH_EVALUATION_DEDUP_MAX_ENTRIES,
            )),
            requested_keys: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            active_subscriptions: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Background task that processes evaluation batches asynchronously
    /// This prevents database writes from blocking the gRPC stream
    async fn run_evaluation_writer(
        logic: Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>,
        mut rx: tokio::sync::mpsc::Receiver<EvaluationWriteJob>,
    ) {
        log::info!("Starting async evaluation writer task");
        while let Some(job) = rx.recv().await {
            let EvaluationWriteJob {
                evaluations,
                completion,
            } = job;
            let count = evaluations.len();
            let ack = match logic.record_evaluations_bulk(evaluations).await {
                Ok(stored) => {
                    log::debug!(
                        "Async writer stored {} evaluations (received {})",
                        stored.len(),
                        count
                    );
                    Ok(())
                }
                Err(e) => {
                    log::error!("Async writer failed to store {} evaluations: {}", count, e);
                    Err(e.to_string())
                }
            };

            if completion.send(ack).is_err() {
                log::debug!("Async evaluation write completed after caller stopped waiting");
            }
        }
        log::warn!("Evaluation writer task shutting down");
    }

    // test-friendly constructor to inject mocks
    pub fn new_with_repos(
        feature_repo: Box<dyn crate::database::feature::FeatureRepository>,
        client_repo: Box<dyn crate::database::client::ClientRepository>,
        updates_tx: tokio::sync::broadcast::Sender<pb::FeatureUpdate>,
        evaluation_events_tx: tokio::sync::broadcast::Sender<
            crate::logic::feature_evaluation::FeatureEvaluationEvent,
        >,
    ) -> Self {
        // Use a dummy pool; not used when repos are injected
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").expect("lazy pool");
        let user_flag_repo =
            crate::database::user_flag_assignment::user_flag_assignment_repository(pool.clone());
        // In test/mocked scenarios, avoid cloning the mocked client_repo which would require
        // a mock expectation on clone_box(). The failing tests don't exercise user_flag_* APIs,
        // so it's safe to plug a no-op logic implementation here.
        let user_flag_logic: Box<dyn crate::logic::user_flag::UserFlagLogic> =
            Box::new(NoopUserFlagLogic);
        let feature_evaluation_repo =
            crate::database::feature_evaluation::feature_evaluation_repository(pool.clone());
        let feature_evaluation_logic =
            crate::logic::feature_evaluation::feature_evaluation_logic_with_events(
                feature_evaluation_repo,
                evaluation_events_tx.clone(),
            );
        // No-op metric logic for tests (avoids cloning mocked repos)
        let metric_logic: Box<dyn MetricLogic> = Box::new(NoopMetricLogic);

        // Create mpsc channel for async database writes (test mode)
        let (evaluation_writer_tx, evaluation_writer_rx) =
            tokio::sync::mpsc::channel::<EvaluationWriteJob>(EVALUATION_WRITER_QUEUE_CAPACITY);

        // Spawn background task to handle database writes
        let logic_clone = feature_evaluation_logic.clone_box();
        tokio::spawn(async move {
            Self::run_evaluation_writer(logic_clone, evaluation_writer_rx).await;
        });

        Self {
            pool,
            feature_repo,
            client_repo,
            user_flag_repo,
            user_flag_logic,
            feature_evaluation_logic,
            metric_logic,
            updates_tx,
            evaluation_writer_tx,
            push_evaluation_deduper: std::sync::Arc::new(PushEvaluationRequestDeduper::new(
                PUSH_EVALUATION_DEDUP_TTL,
                PUSH_EVALUATION_DEDUP_MAX_ENTRIES,
            )),
            requested_keys: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            active_subscriptions: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    async fn map_db_feature_to_engine(&self, root: db::Feature) -> Result<engine::Feature, Status> {
        let repo = &self.feature_repo;
        let root_id = root.id;

        let mut feature_graph: std::collections::HashMap<Uuid, db::Feature> =
            std::collections::HashMap::new();
        let mut queue: std::collections::VecDeque<Uuid> = root
            .dependencies
            .iter()
            .map(|dependency| dependency.depends_on_id)
            .collect();

        feature_graph.insert(root.id, root);

        while let Some(feature_id) = queue.pop_front() {
            if feature_graph.contains_key(&feature_id) {
                continue;
            }

            let dependency_feature = repo
                .get_feature_by_id(feature_id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?;

            for nested_dependency in &dependency_feature.dependencies {
                if !feature_graph.contains_key(&nested_dependency.depends_on_id) {
                    queue.push_back(nested_dependency.depends_on_id);
                }
            }

            feature_graph.insert(dependency_feature.id, dependency_feature);
        }

        let mut base_map: std::collections::HashMap<Uuid, EngineFeatureBase> =
            std::collections::HashMap::new();
        for feature in feature_graph.values() {
            let (stages, variants) = self.map_db_feature_payload_to_engine(feature).await?;
            base_map.insert(
                feature.id,
                EngineFeatureBase {
                    id: feature.id.to_string(),
                    key: feature.key.clone(),
                    feature_type: format!("{:?}", feature.feature_type),
                    active: feature.active,
                    enabled: feature.active && feature.kill_switch_enabled,
                    dependency_ids: feature
                        .dependencies
                        .iter()
                        .map(|dependency| dependency.depends_on_id)
                        .collect(),
                    stages,
                    variants,
                },
            );
        }

        let mut memo = std::collections::HashMap::new();
        let mut visiting = std::collections::HashSet::new();
        Ok(Self::build_engine_dependency_graph(
            root_id,
            &base_map,
            &mut memo,
            &mut visiting,
        ))
    }

    async fn map_db_feature_payload_to_engine(
        &self,
        feature: &db::Feature,
    ) -> Result<(Vec<engine::FeatureStage>, Vec<engine::FeatureVariant>), Status> {
        let repo = &self.feature_repo;

        let db_stages = repo
            .get_feature_stages(feature.id)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;
        let mut stages = Vec::with_capacity(db_stages.len());
        for stage in db_stages {
            let criterias = repo
                .get_stage_criteria(stage.id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?;

            let mapped_criteria = criterias
                .into_iter()
                .map(|criterion| {
                    let rule_groups = criterion
                        .rule_groups
                        .into_iter()
                        .map(|group| engine::RuleGroup {
                            logic_operator: match group.logic_operator {
                                crate::database::entity::LogicOperator::And => {
                                    engine::LogicOperator::And
                                }
                                crate::database::entity::LogicOperator::Or => {
                                    engine::LogicOperator::Or
                                }
                            },
                            conditions: group
                                .conditions
                                .into_iter()
                                .map(|condition| {
                                    let operator = match condition.operator.to_uppercase().as_str()
                                    {
                                        "EQUALS" => engine::Operator::Equals,
                                        "NOTEQUALS" | "NOT_EQUALS" => engine::Operator::NotEquals,
                                        "GREATERTHAN" | "GREATER_THAN" => {
                                            engine::Operator::GreaterThan
                                        }
                                        "LESSTHAN" | "LESS_THAN" => engine::Operator::LessThan,
                                        "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => {
                                            engine::Operator::GreaterThanOrEqual
                                        }
                                        "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => {
                                            engine::Operator::LessThanOrEqual
                                        }
                                        "CONTAINS" => engine::Operator::Contains,
                                        "STARTSWITH" | "STARTS_WITH" => {
                                            engine::Operator::StartsWith
                                        }
                                        "ENDSWITH" | "ENDS_WITH" => engine::Operator::EndsWith,
                                        "REGEX" => engine::Operator::Regex,
                                        "IN" => engine::Operator::In,
                                        "NOTIN" | "NOT_IN" => engine::Operator::NotIn,
                                        "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => {
                                            engine::Operator::SemverGreaterThan
                                        }
                                        "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => {
                                            engine::Operator::SemverLessThan
                                        }
                                        _ => engine::Operator::In,
                                    };

                                    engine::RuleCondition {
                                        context_key: condition.context_key,
                                        operator,
                                        value: condition.value,
                                    }
                                })
                                .collect(),
                        })
                        .collect();

                    engine::StageCriterion {
                        priority: criterion.priority,
                        rule_groups,
                        variant_allocations: criterion
                            .variant_allocations
                            .into_iter()
                            .map(|allocation| engine::VariantAllocation {
                                variant_control: allocation.variant_control,
                                weight: allocation.weight,
                            })
                            .collect(),
                        variant_selection_mode: match criterion.variant_selection_mode {
                            crate::database::entity::VariantSelectionMode::SpecificVariant => {
                                engine::VariantSelectionMode::SpecificVariant
                            }
                            crate::database::entity::VariantSelectionMode::WeightedSplit => {
                                engine::VariantSelectionMode::WeightedSplit
                            }
                        },
                        selected_variant_control: criterion.selected_variant_control,
                    }
                })
                .collect::<Vec<_>>();

            stages.push(engine::FeatureStage {
                environment_id: stage.environment_id.to_string(),
                enabled: stage.enabled,
                criterias: mapped_criteria,
            });
        }

        let variants = if matches!(feature.feature_type, db::FeatureType::Contextual) {
            repo.get_feature_variants(feature.id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?
                .into_iter()
                .map(|variant| engine::FeatureVariant {
                    control: variant.control,
                    value: variant.value,
                })
                .collect()
        } else {
            vec![]
        };

        Ok((stages, variants))
    }

    fn build_engine_dependency_graph(
        feature_id: Uuid,
        base_map: &std::collections::HashMap<Uuid, EngineFeatureBase>,
        memo: &mut std::collections::HashMap<Uuid, engine::Feature>,
        visiting: &mut std::collections::HashSet<Uuid>,
    ) -> engine::Feature {
        if let Some(cached) = memo.get(&feature_id) {
            return cached.clone();
        }

        let Some(base) = base_map.get(&feature_id) else {
            return engine::Feature {
                id: feature_id.to_string(),
                key: feature_id.to_string(),
                feature_type: "Simple".to_string(),
                active: false,
                enabled: false,
                dependencies: vec![],
                stages: vec![],
                variants: vec![],
            };
        };

        if !visiting.insert(feature_id) {
            return engine::Feature {
                id: base.id.clone(),
                key: base.key.clone(),
                feature_type: base.feature_type.clone(),
                active: false,
                enabled: false,
                dependencies: vec![],
                stages: base.stages.clone(),
                variants: base.variants.clone(),
            };
        }

        let dependencies = base
            .dependency_ids
            .iter()
            .map(|dependency_id| {
                Self::build_engine_dependency_graph(*dependency_id, base_map, memo, visiting)
            })
            .collect::<Vec<_>>();

        visiting.remove(&feature_id);

        let feature = engine::Feature {
            id: base.id.clone(),
            key: base.key.clone(),
            feature_type: base.feature_type.clone(),
            active: base.active,
            enabled: base.enabled,
            dependencies,
            stages: base.stages.clone(),
            variants: base.variants.clone(),
        };
        memo.insert(feature_id, feature.clone());
        feature
    }

    async fn map_db_feature_to_full(&self, f: db::Feature) -> Result<pb::FeatureFull, Status> {
        let repo = &self.feature_repo;

        // Map stages and load criterias for each
        let stages = repo.get_feature_stages(f.id).await;
        if stages.is_err() {
            return Err(Status::internal(format!(
                "db error: {}",
                stages.err().unwrap()
            )));
        }
        let stages = stages.unwrap();
        let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(stages.len());
        for s in stages.iter() {
            let crits = repo
                .get_stage_criteria(s.id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?;
            let criterias = crits
                .into_iter()
                .map(|c| {
                    // Map rule groups
                    let rule_groups = c
                        .rule_groups
                        .into_iter()
                        .map(|group| pb::RuleGroup {
                            id: group.id.to_string(),
                            logic_operator: match group.logic_operator {
                                crate::database::entity::LogicOperator::And => "AND".to_string(),
                                crate::database::entity::LogicOperator::Or => "OR".to_string(),
                            },
                            conditions: group
                                .conditions
                                .into_iter()
                                .map(|cond| pb::RuleCondition {
                                    id: cond.id.to_string(),
                                    context_key: cond.context_key,
                                    operator: cond.operator,
                                    value: cond.value.to_string(),
                                    order_index: cond.order_index,
                                })
                                .collect(),
                        })
                        .collect();

                    // Map variant allocations
                    let variant_allocations = c
                        .variant_allocations
                        .into_iter()
                        .map(|alloc| pb::VariantAllocation {
                            variant_control: alloc.variant_control,
                            weight: alloc.weight,
                        })
                        .collect();

                    pb::StageCriterionFull {
                        id: c.id.to_string(),
                        stage_id: c.stage_id.to_string(),
                        priority: c.priority,
                        rule_groups,
                        variant_allocations,
                        variant_selection_mode: match c.variant_selection_mode {
                            crate::database::entity::VariantSelectionMode::WeightedSplit => {
                                "WEIGHTED_SPLIT".to_string()
                            }
                            crate::database::entity::VariantSelectionMode::SpecificVariant => {
                                "SPECIFIC_VARIANT".to_string()
                            }
                        },
                        selected_variant_control: c.selected_variant_control.unwrap_or_default(),
                    }
                })
                .collect::<Vec<_>>();

            stage_msgs.push(pb::FeatureStageFull {
                id: s.id.to_string(),
                environment_id: s.environment_id.to_string(),
                order_index: s.order_index,
                position: s.position.clone(),
                enabled: s.enabled,
                criterias,
            });
        }

        // Map dependencies
        let deps = f
            .dependencies
            .iter()
            .map(|d| pb::FeatureDependencyFull {
                feature_id: d.feature_id.to_string(),
                depends_on_id: d.depends_on_id.to_string(),
            })
            .collect::<Vec<_>>();

        // Load variants from database only for Contextual features
        let variant_msgs = if matches!(f.feature_type, db::FeatureType::Contextual) {
            let db_variants = repo
                .get_feature_variants(f.id)
                .await
                .map_err(|e| Status::internal(format!("db error: {}", e)))?;

            db_variants
                .into_iter()
                .map(|v| pb::FeatureVariant {
                    control: v.control,
                    value: serde_json::to_string(&v.value).unwrap_or_default(),
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        let feature = pb::FeatureFull {
            id: f.id.to_string(),
            key: f.key,
            description: f.description.unwrap_or_default(),
            feature_type: format!("{:?}", f.feature_type),
            team_id: f.team_id.to_string(),
            created_at: f.created_at.to_rfc3339(),
            active: f.active,
            kill_switch_enabled: f.kill_switch_enabled,
            kill_switch_activated_at: f
                .kill_switch_activated_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            rollback_scheduled_at: f
                .rollback_scheduled_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            stages: stage_msgs,
            dependencies: deps,
            variants: variant_msgs,
        };

        Ok(feature)
    }
}

#[tonic::async_trait]
impl FeatureEvaluation for FeatureEvaluationSvc {
    async fn evaluate(
        &self,
        request: Request<EvaluateRequest>,
    ) -> Result<Response<EvaluateResponse>, Status> {
        let req = request.into_inner();

        // Validate inputs
        if req.client_id.is_empty() {
            return Err(Status::invalid_argument("client_id is required"));
        }
        if req.client_secret.is_empty() {
            return Err(Status::invalid_argument("client_secret is required"));
        }
        if req.feature_key.is_empty() {
            return Err(Status::invalid_argument("feature_key is required"));
        }

        let client_id = Uuid::parse_str(&req.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;

        // Fetch client -> team
        let client_repo = &self.client_repo;
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;

        // Validate client secret and status
        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != req.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }

        let team_id = client.team_id;

        // Fetch feature by key within team
        let feature_repo = &self.feature_repo;
        let mut features = feature_repo
            .get_features(team_id, Some(req.feature_key.clone()), None)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;

        let db_feature = features.pop().ok_or_else(|| {
            Status::not_found("feature with given key not found for client's team")
        })?;

        // Check kill switch first - if disabled, return false regardless of other criteria
        if !db_feature.kill_switch_enabled {
            return Ok(Response::new(EvaluateResponse { enabled: false }));
        }

        let eng_feature = self.map_db_feature_to_engine(db_feature.clone()).await?;

        // Convert proto context to engine context format
        let mut attributes = std::collections::HashMap::new();
        let mut targeting_key = String::new();

        for c in req.context {
            if c.key == "bucketingKey" {
                targeting_key = c.value;
            } else {
                attributes.insert(c.key, serde_json::json!(c.value));
            }
        }

        let ec = engine::FeatureEvaluationContext {
            flag_key: db_feature.key,
            context: engine::ContextObject {
                targeting_key,
                environment_id: req.environment_id,
                attributes,
            },
        };

        let result = engine::evaluate(&ec, &eng_feature);

        // For backward compatibility, return just the boolean value
        let enabled = result.value.as_bool().unwrap_or(false);

        Ok(Response::new(EvaluateResponse { enabled }))
    }

    async fn get_feature_by_key(
        &self,
        request: Request<pb::GetFeatureByKeyRequest>,
    ) -> Result<Response<pb::GetFeatureByKeyResponse>, Status> {
        let req = request.into_inner();

        if req.client_id.is_empty() {
            return Err(Status::invalid_argument("client_id is required"));
        }
        if req.client_secret.is_empty() {
            return Err(Status::invalid_argument("client_secret is required"));
        }
        if req.feature_key.is_empty() {
            return Err(Status::invalid_argument("feature_key is required"));
        }

        let client_id = Uuid::parse_str(&req.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;

        // Fetch client -> team
        let client_repo = &self.client_repo;
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;

        // Validate client secret and status
        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != req.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }

        let team_id = client.team_id;

        // Fetch feature by key within team
        let feature_repo = &self.feature_repo;
        let mut features = feature_repo
            .get_features(team_id, Some(req.feature_key.clone()), None)
            .await
            .map_err(|e| Status::internal(format!("db error: {}", e)))?;

        let response = if let Some(db_feature) = features.pop() {
            let feature_msg = self.map_db_feature_to_full(db_feature).await?;

            // Track that this client requested this feature key for future update filtering
            {
                let mut map = self.requested_keys.write().await;
                let entry = map.entry(client_id).or_default();
                entry.insert(req.feature_key.clone());
            }

            pb::GetFeatureByKeyResponse {
                feature: Some(feature_msg),
            }
        } else {
            // Feature not found - return None instead of error
            pb::GetFeatureByKeyResponse { feature: None }
        };

        Ok(Response::new(response))
    }

    async fn get_client_info(
        &self,
        request: Request<pb::GetClientInfoRequest>,
    ) -> Result<Response<pb::GetClientInfoResponse>, Status> {
        let req = request.into_inner();

        if req.client_id.is_empty() {
            return Err(Status::invalid_argument("client_id is required"));
        }
        if req.client_secret.is_empty() {
            return Err(Status::invalid_argument("client_secret is required"));
        }

        let client_id = Uuid::parse_str(&req.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;

        // Fetch and authenticate client
        let client_repo = &self.client_repo;
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;

        // Validate client secret and status
        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != req.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }

        // Map client type to string
        let client_type_str = match client.client_type {
            crate::database::entity::ClientType::Web => "Web",
            crate::database::entity::ClientType::Backend => "Backend",
        };

        let response = pb::GetClientInfoResponse {
            id: client.id.to_string(),
            team_id: client.team_id.to_string(),
            name: client.name,
            description: client.description.unwrap_or_default(),
            enabled: client.enabled,
            client_type: client_type_str.to_string(),
            web_origins: client.web_origins.unwrap_or_default(),
            environment_id: client.environment_id.to_string(),
        };

        Ok(Response::new(response))
    }

    async fn push_user_assignments(
        &self,
        request: Request<tonic::Streaming<pb::UserFlagAssignment>>,
    ) -> Result<Response<pb::Ack>, Status> {
        let mut stream = request.into_inner();

        // Read first message to authenticate and then process the rest with same creds
        let first_msg = match stream.next().await {
            Some(Ok(m)) => m,
            Some(Err(e)) => return Err(Status::internal(format!("stream error: {}", e))),
            None => return Err(Status::invalid_argument("empty stream")),
        };

        // Authenticate using logic
        match self
            .user_flag_logic
            .authenticate_client(&first_msg.client_id, &first_msg.client_secret)
            .await
        {
            Ok(_) => {}
            Err(e) => {
                return Err(match e {
                    crate::logic::user_flag::UserFlagLogicError::InvalidInput(m) => {
                        Status::invalid_argument(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::NotFound(_) => {
                        Status::not_found("client not found")
                    }
                    crate::logic::user_flag::UserFlagLogicError::PermissionDenied(m) => {
                        Status::permission_denied(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::Unauthenticated(m) => {
                        Status::unauthenticated(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::DatabaseError(e) => {
                        Status::internal(format!("db error: {}", e))
                    }
                });
            }
        }

        // Process the first payload then the rest via logic
        let variant = if first_msg.variant.is_empty() {
            None
        } else {
            Some(first_msg.variant)
        };
        if let Err(e) = self
            .user_flag_logic
            .upsert_after_auth(
                &first_msg.user_id,
                &first_msg.feature_id,
                &first_msg.environment_id,
                first_msg.assigned,
                variant,
            )
            .await
        {
            return Err(match e {
                crate::logic::user_flag::UserFlagLogicError::InvalidInput(m) => {
                    Status::invalid_argument(m)
                }
                crate::logic::user_flag::UserFlagLogicError::DatabaseError(e) => {
                    Status::internal(format!("db error: {}", e))
                }
                _ => Status::internal("unexpected error"),
            });
        }

        while let Some(msg) = stream.next().await {
            match msg {
                Ok(m) => {
                    let variant = if m.variant.is_empty() {
                        None
                    } else {
                        Some(m.variant)
                    };
                    if let Err(e) = self
                        .user_flag_logic
                        .upsert_after_auth(
                            &m.user_id,
                            &m.feature_id,
                            &m.environment_id,
                            m.assigned,
                            variant,
                        )
                        .await
                    {
                        return Err(match e {
                            crate::logic::user_flag::UserFlagLogicError::InvalidInput(m) => {
                                Status::invalid_argument(m)
                            }
                            crate::logic::user_flag::UserFlagLogicError::DatabaseError(e) => {
                                Status::internal(format!("db error: {}", e))
                            }
                            _ => Status::internal("unexpected error"),
                        });
                    }
                }
                Err(e) => return Err(Status::internal(format!("stream error: {}", e))),
            }
        }

        Ok(Response::new(pb::Ack {
            message_id: uuid::Uuid::new_v4().to_string(),
        }))
    }

    async fn list_user_assignments(
        &self,
        request: Request<pb::ListUserFlagAssignmentsRequest>,
    ) -> Result<Response<pb::ListUserFlagAssignmentsResponse>, Status> {
        let req = request.into_inner();

        // Authenticate using logic to obtain team_id
        let team_id = match self
            .user_flag_logic
            .authenticate_client(&req.client_id, &req.client_secret)
            .await
        {
            Ok(team_id) => team_id,
            Err(e) => {
                return Err(match e {
                    crate::logic::user_flag::UserFlagLogicError::InvalidInput(m) => {
                        Status::invalid_argument(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::NotFound(_) => {
                        Status::not_found("client not found")
                    }
                    crate::logic::user_flag::UserFlagLogicError::PermissionDenied(m) => {
                        Status::permission_denied(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::Unauthenticated(m) => {
                        Status::unauthenticated(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::DatabaseError(e) => {
                        Status::internal(format!("db error: {}", e))
                    }
                });
            }
        };

        let rows = match self
            .user_flag_logic
            .list_user_assignments(team_id, Some(req.feature_id), Some(req.environment_id))
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                return Err(match e {
                    crate::logic::user_flag::UserFlagLogicError::InvalidInput(m) => {
                        Status::invalid_argument(m)
                    }
                    crate::logic::user_flag::UserFlagLogicError::DatabaseError(e) => {
                        Status::internal(format!("db error: {}", e))
                    }
                    _ => Status::internal("unexpected error"),
                });
            }
        };

        let assignments = rows
            .into_iter()
            .map(|r| pb::UserFlagAssignment {
                user_id: r.user_id,
                feature_id: r.feature_id.to_string(),
                environment_id: r.environment_id.to_string(),
                assigned: r.assigned,
                client_id: String::new(),
                client_secret: String::new(),
                variant: r.variant.unwrap_or_default(),
            })
            .collect::<Vec<_>>();

        Ok(Response::new(pb::ListUserFlagAssignmentsResponse {
            assignments,
        }))
    }

    type StreamUpdatesStream = ReceiverStream<Result<pb::FeatureUpdate, Status>>;

    async fn stream_updates(
        &self,
        request: Request<tonic::Streaming<pb::StreamRequest>>,
    ) -> Result<Response<Self::StreamUpdatesStream>, Status> {
        let mut in_stream = request.into_inner();

        // Expect first message to be SubscribeRequest
        let first = in_stream
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("missing subscribe message"))??;
        let subscribe = match first.payload {
            Some(pb::stream_request::Payload::Subscribe(s)) => s,
            _ => return Err(Status::invalid_argument("first message must be subscribe")),
        };

        // Authenticate similar to other methods
        if subscribe.client_id.is_empty() || subscribe.client_secret.is_empty() {
            return Err(Status::invalid_argument(
                "client_id and client_secret are required",
            ));
        }
        let client_id = Uuid::parse_str(&subscribe.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;
        let client_repo = &self.client_repo;
        let client = client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;
        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != subscribe.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }
        let team_id = client.team_id;

        // Prepare outgoing channel
        let (out_tx, out_rx) = mpsc::channel::<Result<pb::FeatureUpdate, Status>>(64);

        let stream_id = Uuid::new_v4();
        let subscription_filter = SubscriptionFilter::from_feature_keys(subscribe.feature_keys);
        {
            let mut map = self.active_subscriptions.write().await;
            let per_client = map.entry(client_id).or_default();
            per_client.insert(stream_id, subscription_filter.clone());
        }

        // Include keys requested via unary GetFeatureByKey so long-lived streams
        // can pick up those updates without reconnecting. These keys remain
        // client-scoped only while the client still has at least one live stream.
        let requested_keys_for_client = {
            let map = self.requested_keys.read().await;
            map.get(&client_id).cloned().unwrap_or_default()
        };

        // Send initial snapshot
        {
            let feature_repo = &self.feature_repo;

            let features_to_send =
                match subscription_filter.snapshot_keys(&requested_keys_for_client) {
                    None => {
                        log::info!("gRPC: Sending full snapshot for client {}", client_id);
                        feature_repo
                            .get_features(team_id, None, None)
                            .await
                            .map_err(|e| Status::internal(format!("db error: {}", e)))?
                    }
                    Some(subscription_keys) => {
                        if subscription_keys.is_empty() {
                            vec![]
                        } else {
                            log::info!(
                                "gRPC: Sending snapshot of {} feature key(s) for client {}",
                                subscription_keys.len(),
                                client_id
                            );
                            let mut all_features = Vec::new();
                            for feature_key in &subscription_keys {
                                let features = feature_repo
                                    .get_features(team_id, Some(feature_key.clone()), None)
                                    .await
                                    .map_err(|e| Status::internal(format!("db error: {}", e)))?;
                                all_features.extend(features);
                            }
                            all_features
                        }
                    }
                };

            log::info!(
                "gRPC: Snapshot contains {} features",
                features_to_send.len()
            );

            // Send each feature as a snapshot update
            for f in features_to_send {
                let full = self.map_db_feature_to_full(f).await?;
                let _ = out_tx
                    .send(Ok(pb::FeatureUpdate {
                        message_id: uuid::Uuid::new_v4().to_string(),
                        action: pb::feature_update::Action::Snapshot as i32,
                        feature: Some(full),
                        feature_key: String::new(),
                        error: String::new(),
                    }))
                    .await;
            }

            log::info!("gRPC: Snapshot sent successfully");
        }

        // Subscribe to shared broadcaster for live updates
        let mut rx = self.updates_tx.subscribe();
        let out_tx_clone = out_tx.clone();
        let requested_keys_clone = self.requested_keys.clone();
        let subscriptions_clone = self.active_subscriptions.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(update) => {
                        // Determine feature key for the update
                        let key_for_update = if let Some(ref feature) = update.feature {
                            feature.key.clone()
                        } else {
                            update.feature_key.clone()
                        };

                        let should_send = stream_allows_feature(
                            &requested_keys_clone,
                            &subscriptions_clone,
                            client_id,
                            stream_id,
                            &key_for_update,
                        )
                        .await;

                        if should_send {
                            log::info!(
                                "gRPC: Sending feature update message_id={} key='{}' to edge client",
                                update.message_id,
                                key_for_update
                            );
                            if out_tx_clone.send(Ok(update)).await.is_err() {
                                log::warn!("gRPC: Client stream closed, stopping update task");
                                break;
                            }
                        } else {
                            log::debug!(
                                "gRPC: Filtering out update message_id={} key='{}' (not in subscription keys)",
                                update.message_id,
                                key_for_update
                            );
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = out_tx_clone
                            .send(Ok(pb::FeatureUpdate {
                                message_id: uuid::Uuid::new_v4().to_string(),
                                action: pb::feature_update::Action::Error as i32,
                                feature: None,
                                feature_key: String::new(),
                                error: "lagged".into(),
                            }))
                            .await;
                        break;
                    }
                }
            }
            unregister_stream_subscription(
                &requested_keys_clone,
                &subscriptions_clone,
                client_id,
                stream_id,
            )
            .await;
        });

        // Ensure cleanup even when no further broadcast messages arrive.
        let cleanup_tx = out_tx.clone();
        let cleanup_requested_keys = self.requested_keys.clone();
        let cleanup_subscriptions = self.active_subscriptions.clone();
        tokio::spawn(async move {
            cleanup_tx.closed().await;
            unregister_stream_subscription(
                &cleanup_requested_keys,
                &cleanup_subscriptions,
                client_id,
                stream_id,
            )
            .await;
        });

        // Handle incoming heartbeats/acks (optional). We keep the stream alive by draining inputs.
        let drain_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = in_stream.next().await {
                match msg {
                    Ok(pb::StreamRequest {
                        payload: Some(pb::stream_request::Payload::Heartbeat(_hb)),
                    }) => {
                        let _ = drain_tx
                            .send(Ok(pb::FeatureUpdate {
                                message_id: uuid::Uuid::new_v4().to_string(),
                                action: pb::feature_update::Action::Heartbeat as i32,
                                feature: None,
                                feature_key: String::new(),
                                error: String::new(),
                            }))
                            .await;
                    }
                    Ok(_) => { /* ignore other kinds for now */ }
                    Err(_) => break,
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(out_rx)))
    }

    async fn push_evaluation_events(
        &self,
        request: Request<pb::PushEvaluationEventsRequest>,
    ) -> Result<Response<pb::PushEvaluationEventsResponse>, Status> {
        let req = request.into_inner();
        let input_count = req.events.len();

        if input_count == 0 {
            return Ok(Response::new(pb::PushEvaluationEventsResponse {
                message_id: uuid::Uuid::new_v4().to_string(),
                processed_count: 0,
            }));
        }

        // Authenticate using the first event (all events from same client)
        let first_event = &req.events[0];
        if first_event.client_id.is_empty() || first_event.client_secret.is_empty() {
            return Err(Status::invalid_argument(
                "client_id and client_secret are required",
            ));
        }

        let client_id = Uuid::parse_str(&first_event.client_id)
            .map_err(|_| Status::invalid_argument("client_id must be a UUID"))?;

        // Fetch and validate client
        let client = self
            .client_repo
            .get_client_by_id(client_id)
            .await
            .map_err(|e| Status::not_found(format!("client not found: {}", e)))?;

        if !client.enabled {
            return Err(Status::permission_denied("client is disabled"));
        }
        if client.api_key != first_event.client_secret {
            return Err(Status::unauthenticated("invalid client_secret"));
        }

        let request_fingerprint = push_evaluation_request_fingerprint(&req).map_err(|e| {
            Status::internal(format!("failed to fingerprint evaluation payload: {e}"))
        })?;
        if self
            .push_evaluation_deduper
            .contains_recent(&request_fingerprint)
            .await
        {
            log::debug!(
                "Skipping duplicate push_evaluation_events batch ({} events)",
                input_count
            );
            return Ok(Response::new(pb::PushEvaluationEventsResponse {
                message_id: uuid::Uuid::new_v4().to_string(),
                processed_count: input_count as i32,
            }));
        }

        // Convert proto events to database format
        let mut evaluations = Vec::with_capacity(input_count);
        for event in req.events {
            if event.feature_key.is_empty() {
                return Err(Status::invalid_argument("feature_key cannot be empty"));
            }
            if event.environment_id.is_empty() {
                return Err(Status::invalid_argument("environment_id cannot be empty"));
            }

            let evaluated_at = if event.evaluated_at_unix_ms > 0 {
                sqlx::types::chrono::DateTime::from_timestamp_millis(event.evaluated_at_unix_ms)
                    .unwrap_or_else(sqlx::types::chrono::Utc::now)
            } else {
                sqlx::types::chrono::Utc::now()
            };

            // Convert context to JSON
            let evaluation_context =
                PushEvaluationRequestContext::from_proto(&event.evaluation_context).to_json_value();

            let user_context = if event.user_context.is_empty() {
                None
            } else {
                Some(event.user_context)
            };

            let variant = if event.variant.is_empty() {
                None
            } else {
                Some(event.variant.clone())
            };

            // For evaluation_value, use variant_value if provided, otherwise use the boolean result
            // variant_value comes from the edge server and contains the actual variant value as JSON
            let evaluation_value = if !event.variant_value.is_empty() {
                // Parse the variant_value JSON string
                serde_json::from_str::<serde_json::Value>(&event.variant_value)
                    .ok()
                    .or_else(|| {
                        log::warn!(
                            "Failed to parse variant_value as JSON for feature '{}', using as string",
                            event.feature_key
                        );
                        Some(serde_json::json!(event.variant_value))
                    })
            } else {
                Some(serde_json::json!(event.evaluation_result))
            };

            // evaluation_success is true if evaluation didn't fail
            // For now, we consider all evaluations successful (edge server only sends successful ones)
            // In the future, we should add an explicit success field to the proto
            let evaluation_success = true;

            evaluations.push(
                crate::database::feature_evaluation::CreateFeatureEvaluation {
                    feature_key: event.feature_key,
                    environment_id: event.environment_id,
                    client_id,
                    evaluated_at,
                    #[allow(deprecated)]
                    evaluation_result: event.evaluation_result,
                    evaluation_context,
                    user_context,
                    prior_assignment: event.prior_assignment,
                    evaluation_success,
                    evaluation_value,
                    variant,
                },
            );
        }

        // Ack only after a bounded queue handoff and a durability confirmation
        // from the writer task. ResourceExhausted tells the edge to retry
        // instead of assuming the batch was durably recorded.
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let job = EvaluationWriteJob {
            evaluations,
            completion: completion_tx,
        };
        let send_result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            self.evaluation_writer_tx.send(job),
        )
        .await;

        match send_result {
            Ok(Ok(())) => {
                match tokio::time::timeout(EVALUATION_DURABILITY_ACK_TIMEOUT, completion_rx).await {
                    Ok(Ok(Ok(()))) => {
                        self.push_evaluation_deduper
                            .remember(request_fingerprint)
                            .await;
                        log::debug!(
                            "Persisted {} evaluation events after bounded queueing",
                            input_count
                        );
                        Ok(Response::new(pb::PushEvaluationEventsResponse {
                            message_id: uuid::Uuid::new_v4().to_string(),
                            processed_count: input_count as i32,
                        }))
                    }
                    Ok(Ok(Err(err))) => {
                        log::error!("Async writer failed to persist evaluation events: {}", err);
                        Err(Status::internal(format!(
                            "failed to persist evaluation events: {err}"
                        )))
                    }
                    Ok(Err(_)) => {
                        log::error!("Async writer dropped persistence ack channel");
                        Err(Status::internal(
                            "failed to confirm persistence of evaluation events",
                        ))
                    }
                    Err(_) => {
                        log::warn!(
                            "Timed out waiting for durability ack for {} evaluation events",
                            input_count
                        );
                        Err(Status::resource_exhausted(
                            "evaluation ingest writer is slow; retry later",
                        ))
                    }
                }
            }
            Ok(Err(e)) => {
                log::error!("Failed to queue evaluation events: {}", e);
                Err(Status::internal("failed to queue evaluation events"))
            }
            Err(_) => {
                log::warn!(
                    "Timed out queueing {} evaluation events (writer queue backpressure)",
                    input_count
                );
                Err(Status::resource_exhausted(
                    "evaluation ingest queue is saturated; retry later",
                ))
            }
        }
    }

    async fn track_metrics(
        &self,
        request: Request<pb::TrackMetricRequest>,
    ) -> Result<Response<pb::TrackMetricResponse>, Status> {
        let req = request.into_inner();

        let mut inputs = Vec::with_capacity(req.events.len());
        for event in req.events {
            let environment_id = if event.environment_id.is_empty() {
                None
            } else {
                Some(
                    Uuid::parse_str(&event.environment_id)
                        .map_err(|_| Status::invalid_argument("invalid environment_id"))?,
                )
            };

            let metadata = if event.metadata.trim().is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str::<serde_json::Value>(&event.metadata)
                        .map_err(|_| Status::invalid_argument("metadata must be valid JSON"))?,
                )
            };

            let timestamp = if event.timestamp_unix_ms > 0 {
                Some(
                    DateTime::<Utc>::from_timestamp_millis(event.timestamp_unix_ms)
                        .ok_or_else(|| Status::invalid_argument("timestamp_unix_ms is invalid"))?,
                )
            } else {
                None
            };

            inputs.push(TrackMetricInput {
                metric_key: event.metric_key,
                feature_key: if event.feature_key.is_empty() {
                    None
                } else {
                    Some(event.feature_key)
                },
                environment_id,
                user_context: event.user_context,
                variant: if event.variant.is_empty() {
                    None
                } else {
                    Some(event.variant)
                },
                value: event.value,
                metadata,
                timestamp,
            });
        }

        let processed = self
            .metric_logic
            .track_metrics(&req.client_id, &req.client_secret, inputs)
            .await
            .map_err(map_metric_error)?;

        Ok(Response::new(pb::TrackMetricResponse {
            processed_count: processed as i32,
        }))
    }
}

pub async fn serve(
    pool: sqlx::PgPool,
    addr: std::net::SocketAddr,
    updates_tx: broadcast::Sender<pb::FeatureUpdate>,
    evaluation_events_tx: broadcast::Sender<
        crate::logic::feature_evaluation::FeatureEvaluationEvent,
    >,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let svc = FeatureEvaluationSvc::new(pool, updates_tx.clone(), evaluation_events_tx.clone());
    let svc = FeatureEvaluationServer::new(svc).accept_compressed(CompressionEncoding::Gzip);
    tonic::transport::Server::builder()
        .add_service(svc)
        .serve(addr)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::client::MockClientRepository;
    use crate::database::entity as db;
    use crate::database::feature::MockFeatureRepository;
    use crate::database::feature_evaluation::CreateFeatureEvaluation;
    use std::sync::Arc;
    use tokio::sync::{Notify, mpsc};
    use tokio::time::{Duration, timeout};

    fn test_service(
        evaluation_writer_tx: mpsc::Sender<EvaluationWriteJob>,
    ) -> FeatureEvaluationSvc {
        let pool = sqlx::PgPool::connect_lazy("postgres://unused").expect("lazy pool");
        let feature_repo = Box::new(MockFeatureRepository::new());
        let client_repo = Box::new(MockClientRepository::new());
        let user_flag_repo =
            crate::database::user_flag_assignment::user_flag_assignment_repository(pool.clone());
        let feature_evaluation_repo =
            crate::database::feature_evaluation::feature_evaluation_repository(pool.clone());
        let (events_tx, _events_rx) = tokio::sync::broadcast::channel::<
            crate::logic::feature_evaluation::FeatureEvaluationEvent,
        >(8);
        let feature_evaluation_logic =
            crate::logic::feature_evaluation::feature_evaluation_logic_with_events(
                feature_evaluation_repo,
                events_tx,
            );
        let metric_logic: Box<dyn MetricLogic> = Box::new(NoopMetricLogic);

        FeatureEvaluationSvc {
            pool,
            feature_repo,
            client_repo,
            user_flag_repo,
            user_flag_logic: Box::new(NoopUserFlagLogic),
            feature_evaluation_logic,
            metric_logic,
            updates_tx: tokio::sync::broadcast::channel(8).0,
            evaluation_writer_tx,
            push_evaluation_deduper: Arc::new(PushEvaluationRequestDeduper::new(
                PUSH_EVALUATION_DEDUP_TTL,
                PUSH_EVALUATION_DEDUP_MAX_ENTRIES,
            )),
            requested_keys: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            active_subscriptions: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    fn spawn_acknowledging_writer() -> (
        mpsc::Sender<EvaluationWriteJob>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, mut rx) = mpsc::channel::<EvaluationWriteJob>(2);
        let handle = tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let _ = job.completion.send(Ok(()));
            }
        });
        (tx, handle)
    }

    fn spawn_gated_writer(
        gate: Arc<Notify>,
    ) -> (
        mpsc::Sender<EvaluationWriteJob>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, mut rx) = mpsc::channel::<EvaluationWriteJob>(1);
        let handle = tokio::spawn(async move {
            gate.notified().await;
            while let Some(job) = rx.recv().await {
                let _ = job.completion.send(Ok(()));
            }
        });
        (tx, handle)
    }

    fn test_client(id: Uuid, team_id: Uuid, environment_id: Uuid, secret: &str) -> db::Client {
        db::Client {
            id,
            team_id,
            environment_id,
            name: "client".into(),
            description: None,
            enabled: true,
            client_type: db::ClientType::Backend,
            api_key: secret.to_string(),
            web_origins: None,
        }
    }

    fn push_event(
        feature_key: &str,
        environment_id: &str,
        client_id: &Uuid,
        client_secret: &str,
        value: bool,
        evaluated_at_unix_ms: i64,
    ) -> pb::FeatureEvaluationEvent {
        pb::FeatureEvaluationEvent {
            feature_key: feature_key.to_string(),
            environment_id: environment_id.to_string(),
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            evaluation_result: value,
            evaluation_context: vec![pb::Context {
                key: "bucketingKey".to_string(),
                value: "user-1".to_string(),
            }],
            user_context: "user-1".to_string(),
            evaluated_at_unix_ms,
            prior_assignment: false,
            variant: String::new(),
            variant_value: String::new(),
        }
    }

    fn test_evaluation(
        feature_key: &str,
        environment_id: &str,
        client_id: Uuid,
        evaluated_at_unix_ms: i64,
    ) -> CreateFeatureEvaluation {
        CreateFeatureEvaluation {
            feature_key: feature_key.to_string(),
            environment_id: environment_id.to_string(),
            client_id,
            evaluated_at: sqlx::types::chrono::DateTime::from_timestamp_millis(
                evaluated_at_unix_ms,
            )
            .unwrap_or_else(sqlx::types::chrono::Utc::now),
            #[allow(deprecated)]
            evaluation_result: true,
            evaluation_context: Some(serde_json::json!({"bucketingKey":"user-1"})),
            user_context: Some("user-1".to_string()),
            prior_assignment: false,
            evaluation_success: true,
            evaluation_value: Some(serde_json::json!(true)),
            variant: None,
        }
    }

    #[test]
    fn push_evaluation_request_fingerprint_ignores_context_order() {
        let request_a = pb::PushEvaluationEventsRequest {
            events: vec![pb::FeatureEvaluationEvent {
                feature_key: "feature-a".to_string(),
                environment_id: "env-a".to_string(),
                client_id: Uuid::new_v4().to_string(),
                client_secret: "secret".to_string(),
                evaluation_result: true,
                evaluation_context: vec![
                    pb::Context {
                        key: "region".to_string(),
                        value: "us".to_string(),
                    },
                    pb::Context {
                        key: "tier".to_string(),
                        value: "beta".to_string(),
                    },
                ],
                user_context: "user-1".to_string(),
                evaluated_at_unix_ms: 42,
                prior_assignment: false,
                variant: "control".to_string(),
                variant_value: "{\"enabled\":true}".to_string(),
            }],
        };
        let request_b = pb::PushEvaluationEventsRequest {
            events: vec![pb::FeatureEvaluationEvent {
                evaluation_context: vec![
                    pb::Context {
                        key: "tier".to_string(),
                        value: "beta".to_string(),
                    },
                    pb::Context {
                        key: "region".to_string(),
                        value: "us".to_string(),
                    },
                ],
                ..request_a.events[0].clone()
            }],
        };

        let fingerprint_a = push_evaluation_request_fingerprint(&request_a).unwrap();
        let fingerprint_b = push_evaluation_request_fingerprint(&request_b).unwrap();

        assert_eq!(fingerprint_a, fingerprint_b);
    }

    #[tokio::test]
    async fn push_evaluation_request_deduper_expires_entries() {
        let deduper = PushEvaluationRequestDeduper::new(std::time::Duration::from_millis(5), 8);

        deduper.remember("fingerprint-a".to_string()).await;
        assert!(deduper.contains_recent("fingerprint-a").await);

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert!(!deduper.contains_recent("fingerprint-a").await);
    }

    #[tokio::test]
    async fn push_evaluation_request_deduper_stays_bounded() {
        let deduper = PushEvaluationRequestDeduper::new(std::time::Duration::from_secs(60), 1);

        deduper.remember("fingerprint-a".to_string()).await;
        deduper.remember("fingerprint-b".to_string()).await;

        assert!(!deduper.contains_recent("fingerprint-a").await);
        assert!(deduper.contains_recent("fingerprint-b").await);
    }

    #[tokio::test]
    async fn push_evaluation_events_dedupes_identical_payloads_after_durable_ack() {
        let (writer_tx, writer_handle) = spawn_acknowledging_writer();
        let svc = test_service(writer_tx);
        let client_id = Uuid::new_v4();
        let client_secret = "secret-123";

        let mut client_repo = MockClientRepository::new();
        let client = test_client(client_id, Uuid::new_v4(), Uuid::new_v4(), client_secret);
        client_repo.expect_get_client_by_id().returning(move |id| {
            if id == client_id {
                Ok(client.clone())
            } else {
                Err(crate::Error::NotFound(id))
            }
        });
        let svc = FeatureEvaluationSvc {
            client_repo: Box::new(client_repo),
            ..svc
        };
        let request = pb::PushEvaluationEventsRequest {
            events: vec![push_event(
                "feature-a",
                "env-a",
                &client_id,
                client_secret,
                true,
                42,
            )],
        };

        let first = svc
            .push_evaluation_events(Request::new(request.clone()))
            .await;
        assert!(first.is_ok());

        let second = svc.push_evaluation_events(Request::new(request)).await;
        assert!(second.is_ok());
        assert_eq!(second.unwrap().into_inner().processed_count, 1);

        writer_handle.abort();
    }

    #[tokio::test]
    async fn push_evaluation_events_returns_resource_exhausted_when_writer_queue_is_saturated() {
        let gate = Arc::new(Notify::new());
        let (writer_tx, writer_handle) = spawn_gated_writer(gate.clone());
        let svc = test_service(writer_tx.clone());
        let client_id = Uuid::new_v4();
        let client_secret = "secret-123";
        let team_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();

        let mut client_repo = MockClientRepository::new();
        let client = test_client(client_id, team_id, env_id, client_secret);
        client_repo.expect_get_client_by_id().returning(move |id| {
            if id == client_id {
                Ok(client.clone())
            } else {
                Err(crate::Error::NotFound(id))
            }
        });
        let svc = FeatureEvaluationSvc {
            client_repo: Box::new(client_repo),
            ..svc
        };
        let second = pb::PushEvaluationEventsRequest {
            events: vec![push_event(
                "feature-b",
                "env-a",
                &client_id,
                client_secret,
                true,
                43,
            )],
        };

        let (occupied_completion_tx, _occupied_completion_rx) = tokio::sync::oneshot::channel();
        writer_tx
            .send(EvaluationWriteJob {
                evaluations: vec![test_evaluation("feature-a", "env-a", client_id, 42)],
                completion: occupied_completion_tx,
            })
            .await
            .expect("failed to prefill bounded writer queue");

        let second_resp = timeout(
            Duration::from_secs(2),
            svc.push_evaluation_events(Request::new(second)),
        )
        .await
        .expect("push_evaluation_events should resolve via timeout");
        assert!(
            matches!(second_resp, Err(status) if status.code() == tonic::Code::ResourceExhausted)
        );

        gate.notify_one();

        writer_handle.abort();
    }
}
