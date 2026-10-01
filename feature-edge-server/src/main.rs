use actix_web::{App, HttpServer, web};
use std::{
    net::SocketAddr,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};
use tonic::codec::CompressionEncoding;
use tonic::transport::Endpoint;
use tracing::{error, info};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod config;
mod grpc_client;
mod handlers;

mod pb {
    #![allow(clippy::all)]
    #![allow(warnings)]
    tonic::include_proto!("featuretoggle");
}

#[derive(Clone, Debug)]
pub struct CachedAssignment {
    pub value: serde_json::Value,
    pub variant: Option<String>,
    pub reason: evaluation_engine::EvaluationReason,
}

/// Shards of each per-feature assignment map. dashmap's default (4x the
/// cores) cache-pads every shard, which costs several KB per feature.
const ASSIGNMENT_SHARDS_PER_FEATURE: usize = 8;

/// Sticky assignments indexed by feature, so purging a feature is one
/// removal instead of a scan over every cached assignment.
#[derive(Default)]
pub struct AssignmentCache {
    // feature_id -> "user_id|environment_id" -> assignment
    by_feature: dashmap::DashMap<String, dashmap::DashMap<String, CachedAssignment>>,
}

impl AssignmentCache {
    fn user_key(user_id: &str, environment_id: &str) -> String {
        format!("{user_id}|{environment_id}")
    }

    pub fn get(
        &self,
        user_id: &str,
        feature_id: &str,
        environment_id: &str,
    ) -> Option<CachedAssignment> {
        self.by_feature
            .get(feature_id)?
            .get(&Self::user_key(user_id, environment_id))
            .map(|entry| entry.value().clone())
    }

    pub fn insert(
        &self,
        user_id: &str,
        feature_id: &str,
        environment_id: &str,
        assignment: CachedAssignment,
    ) {
        let key = Self::user_key(user_id, environment_id);
        // Common case: only a read lock on the outer shard.
        if let Some(users) = self.by_feature.get(feature_id) {
            users.insert(key, assignment);
            return;
        }
        self.by_feature
            .entry(feature_id.to_string())
            .or_insert_with(|| dashmap::DashMap::with_shard_amount(ASSIGNMENT_SHARDS_PER_FEATURE))
            .insert(key, assignment);
    }

    pub fn remove_feature(&self, feature_id: &str) {
        self.by_feature.remove(feature_id);
    }

    pub fn clear(&self) {
        self.by_feature.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_feature.iter().map(|users| users.len()).sum()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Sticky assignments waiting to be flushed to the backend. Purging a feature
/// bumps its generation instead of draining the queue: entries queued under
/// an older generation are dropped when popped.
#[derive(Default)]
pub struct PendingAssignments {
    queue: crossbeam::queue::SegQueue<(u64, grpc_client::UserAssignment)>,
    generations: dashmap::DashMap<String, u64>,
}

impl PendingAssignments {
    fn generation(&self, feature_id: &str) -> u64 {
        self.generations
            .get(feature_id)
            .map_or(0, |generation| *generation)
    }

    /// Queue an assignment under its feature's current generation.
    pub fn push(&self, assignment: grpc_client::UserAssignment) {
        let generation = self.generation(&assignment.feature_id);
        self.queue.push((generation, assignment));
    }

    /// Next assignment whose feature was not purged since it was queued.
    pub fn pop(&self) -> Option<grpc_client::UserAssignment> {
        while let Some((generation, assignment)) = self.queue.pop() {
            if generation == self.generation(&assignment.feature_id) {
                return Some(assignment);
            }
        }
        None
    }

    /// Drop every assignment of `feature_id` queued so far.
    pub fn purge_feature(&self, feature_id: &str) {
        *self.generations.entry(feature_id.to_string()).or_insert(0) += 1;
    }

    pub fn clear(&self) {
        while self.queue.pop().is_some() {}
    }
}

/// How long a rejected client credential is remembered. Kept short because a
/// client created or re-enabled after a failed attempt is rejected for up to
/// this long.
const CLIENT_INFO_FAILURE_TTL: Duration = Duration::from_secs(30);
const CLIENT_INFO_FAILURE_CAPACITY: u64 = 10_000;

pub struct ClientInfoCache {
    // Cache with TTL for client info
    cache: moka::future::Cache<String, pb::GetClientInfoResponse>,
    // Permanent auth failures (bad credentials, unknown or disabled client),
    // so repeated bad requests do not reach the backend.
    failures: moka::future::Cache<String, tonic::Code>,
}

impl ClientInfoCache {
    /// Create a new ClientInfoCache with TTL
    pub fn new(ttl: Duration) -> Self {
        tracing::info!("Initializing ClientInfoCache with TTL={:?}", ttl);
        Self {
            cache: moka::future::Cache::builder()
                .time_to_live(ttl)
                .max_capacity(1000) // Support up to 1000 different clients
                .build(),
            failures: moka::future::Cache::builder()
                .time_to_live(CLIENT_INFO_FAILURE_TTL)
                .max_capacity(CLIENT_INFO_FAILURE_CAPACITY)
                .build(),
        }
    }

    pub async fn get_failure(&self, key: &str) -> Option<tonic::Code> {
        self.failures.get(key).await
    }

    pub async fn insert_failure(&self, key: String, code: tonic::Code) {
        self.failures.insert(key, code).await;
    }

    pub async fn get(&self, client_id: &str) -> Option<pb::GetClientInfoResponse> {
        self.cache.get(client_id).await
    }

    pub async fn insert(&self, client_id: String, client_info: pb::GetClientInfoResponse) {
        self.cache.insert(client_id, client_info).await;
    }

    pub async fn invalidate(&self, client_id: &str) {
        self.cache.invalidate(client_id).await;
    }

    /// Get current cache size (number of entries)
    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }
}

#[derive(Clone)]
pub struct AppState {
    mapped_cache: Arc<MappedFeatureCache>,
    client_info_cache: Arc<ClientInfoCache>,
    grpc: Arc<
        tokio::sync::Mutex<
            pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
        >,
    >,
    client_id: String,
    client_secret: String,
    // Team of the configured client, learned from `GetClientInfo`. The edge
    // serves one team: the feature cache only holds this team's features.
    edge_team_id: Arc<std::sync::OnceLock<String>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    // Sticky assignments cache with variant information and pending flush queue
    assigned_cache: Arc<AssignmentCache>,
    pending_assignments: Arc<PendingAssignments>,
    flush_interval: Duration,
    assignment_flush_batch_size: usize,
    // Evaluation events tracking (using channel for lock-free writes)
    evaluation_event_tx: tokio::sync::mpsc::Sender<EvaluationEvent>,
    evaluation_flush_interval: Duration,
    evaluation_flush_batch_size: usize,
    evaluation_event_queue_capacity: usize,
    evaluation_event_dropped: Arc<AtomicU64>,
    // Retry configuration
    retry_config: config::RetryConfig,
}

#[derive(Clone, Debug)]
pub struct EvaluationEvent {
    pub feature_key: String,
    pub environment_id: String,
    pub evaluation_result: bool,
    pub evaluation_context: handlers::EvaluateContext,
    pub user_context: Option<String>,
    pub evaluated_at: std::time::SystemTime,
    pub prior_assignment: bool,
    pub variant: Option<String>,
    pub variant_value: Option<serde_json::Value>,
}

/// A cached feature together with the team that owns it. Feature keys are
/// unique only per team, so readers must check `team_id` before serving it.
#[derive(Clone)]
pub struct CachedFeature {
    pub team_id: Arc<str>,
    pub feature: Arc<evaluation_engine::Feature>,
}

/// Cache for pre-mapped engine::Feature to avoid repeated allocations
pub struct MappedFeatureCache {
    // Primary cache: feature_key -> owning team + Arc<Feature>
    by_key: moka::future::Cache<String, CachedFeature>,
    // Secondary index: feature_id -> feature_key
    by_id: moka::future::Cache<String, String>,
    // Dependency edges: feature_id -> depends_on_feature_ids
    dependency_ids: moka::future::Cache<String, Vec<String>>,
    // Negative cache: feature_key -> () for features that don't exist
    // TTL of 60 seconds so we periodically recheck if feature was created
    negative_cache: moka::future::Cache<String, ()>,
}

impl MappedFeatureCache {
    pub fn new(max_capacity: u64) -> Self {
        tracing::info!(
            "Initializing MappedFeatureCache with max_capacity={}",
            max_capacity
        );
        Self {
            by_key: moka::future::Cache::new(max_capacity),
            by_id: moka::future::Cache::new(max_capacity),
            dependency_ids: moka::future::Cache::new(max_capacity),
            negative_cache: moka::future::Cache::builder()
                .time_to_live(std::time::Duration::from_secs(60))
                .max_capacity(10000)
                .build(),
        }
    }

    /// Get feature by key regardless of the owning team. Request handlers
    /// must use [`Self::get_for_team`] instead.
    #[cfg(test)]
    pub async fn get(&self, key: &str) -> Option<Arc<evaluation_engine::Feature>> {
        self.by_key.get(key).await.map(|entry| entry.feature)
    }

    /// Get feature by key if it belongs to `team_id`.
    pub async fn get_for_team(
        &self,
        key: &str,
        team_id: &str,
    ) -> Option<Arc<evaluation_engine::Feature>> {
        self.by_key
            .get(key)
            .await
            .filter(|entry| &*entry.team_id == team_id)
            .map(|entry| entry.feature)
    }

    /// All cached features that belong to `team_id`.
    pub fn features_for_team(&self, team_id: &str) -> Vec<Arc<evaluation_engine::Feature>> {
        self.by_key
            .iter()
            .filter(|(_, entry)| &*entry.team_id == team_id)
            .map(|(_, entry)| entry.feature)
            .collect()
    }

    /// Get feature by key, or compute and insert it if not present.
    /// This uses moka's built-in request coalescing - if multiple concurrent
    /// requests ask for the same uncached key, only one will execute the
    /// init function while others wait for the result.
    pub async fn optionally_get_with<F, Fut>(&self, key: String, init: F) -> Option<CachedFeature>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<CachedFeature>>,
    {
        self.by_key
            .optionally_get_with(key, async move {
                let feature = init().await?;
                Some(feature)
            })
            .await
    }

    /// Update the by_id index (feature_id -> feature_key mapping)
    /// Used when inserting via optionally_get_with which only updates by_key
    pub async fn update_id_index(&self, id: &str, key: &str) {
        self.by_id.insert(id.to_string(), key.to_string()).await;
    }

    /// Check if a feature key is in the negative cache (doesn't exist in backend)
    pub async fn is_negative_cached(&self, key: &str) -> bool {
        self.negative_cache.get(key).await.is_some()
    }

    /// Add a feature key to the negative cache (mark as non-existent)
    pub async fn add_negative(&self, key: &str) {
        self.negative_cache.insert(key.to_string(), ()).await;
    }

    /// Remove a feature key from negative cache (called when feature is created)
    pub async fn remove_negative(&self, key: &str) {
        self.negative_cache.invalidate(key).await;
    }

    /// Get feature by ID (using secondary index)
    pub async fn get_by_id(&self, id: &str) -> Option<Arc<evaluation_engine::Feature>> {
        let key = self.by_id.get(id).await?;
        self.by_key.get(&key).await.map(|entry| entry.feature)
    }

    /// Insert a feature owned by `team_id` into cache (updates both indices)
    pub async fn insert(&self, team_id: &str, feature: Arc<evaluation_engine::Feature>) {
        self.insert_with_dependencies(team_id, feature, Vec::new())
            .await;
    }

    /// Insert a feature owned by `team_id` into cache and store dependency
    /// edges (updates all indices)
    pub async fn insert_with_dependencies(
        &self,
        team_id: &str,
        feature: Arc<evaluation_engine::Feature>,
        dependency_ids: Vec<String>,
    ) {
        let key = feature.key.clone();
        let id = feature.id.clone();

        self.by_key
            .insert(
                key.clone(),
                CachedFeature {
                    team_id: Arc::from(team_id),
                    feature,
                },
            )
            .await;
        self.negative_cache.invalidate(&key).await;
        self.by_id.insert(id.clone(), key).await;
        self.dependency_ids.insert(id, dependency_ids).await;
    }

    /// Persist dependency IDs for an already cached feature.
    pub async fn set_dependency_ids(&self, feature_id: &str, dependency_ids: Vec<String>) {
        self.dependency_ids
            .insert(feature_id.to_string(), dependency_ids)
            .await;
    }

    /// Get dependency IDs for a feature. Returns empty when no dependencies are recorded.
    pub async fn get_dependency_ids(&self, feature_id: &str) -> Vec<String> {
        self.dependency_ids
            .get(feature_id)
            .await
            .unwrap_or_default()
    }

    /// Invalidate feature by key
    pub async fn invalidate(&self, key: &str) {
        // Get the feature to find its ID before invalidating
        if let Some(entry) = self.by_key.get(key).await {
            self.by_id.invalidate(&entry.feature.id).await;
            self.dependency_ids.invalidate(&entry.feature.id).await;
        }
        self.by_key.invalidate(key).await;
    }

    /// Delete feature by key and return its ID
    pub async fn delete_by_key(&self, key: &str) -> Option<String> {
        let entry = self.by_key.get(key).await?;
        let id = entry.feature.id.clone();

        self.by_key.invalidate(key).await;
        self.by_id.invalidate(&id).await;
        self.dependency_ids.invalidate(&id).await;

        Some(id)
    }

    /// Get all feature keys
    pub async fn get_all_keys(&self) -> Vec<String> {
        self.by_key.iter().map(|(k, _)| k.to_string()).collect()
    }

    pub fn entry_count(&self) -> u64 {
        self.by_key.entry_count()
    }

    /// Clear all feature cache state so the next subscribe requests a fresh snapshot.
    pub async fn clear_all(&self) {
        self.by_key.invalidate_all();
        self.by_id.invalidate_all();
        self.dependency_ids.invalidate_all();
        self.negative_cache.invalidate_all();

        self.by_key.run_pending_tasks().await;
        self.by_id.run_pending_tasks().await;
        self.dependency_ids.run_pending_tasks().await;
        self.negative_cache.run_pending_tasks().await;
    }

    /// Run pending cache tasks (useful for testing)
    #[cfg(test)]
    pub async fn run_pending_tasks(&self) {
        self.by_key.run_pending_tasks().await;
        self.by_id.run_pending_tasks().await;
        self.dependency_ids.run_pending_tasks().await;
        self.negative_cache.run_pending_tasks().await;
    }
}

impl AppState {
    /// Team of the configured client, once known.
    pub fn edge_team_id(&self) -> Option<&str> {
        self.edge_team_id.get().map(String::as_str)
    }

    /// Record the configured client's team. The first non-empty value wins.
    pub fn record_edge_team_id(&self, team_id: &str) {
        if team_id.is_empty() {
            return;
        }
        match self.edge_team_id.get() {
            Some(known) if known != team_id => tracing::warn!(
                "Configured client reported team '{}', but the edge already serves team '{}'",
                team_id,
                known
            ),
            Some(_) => {}
            None => {
                if self.edge_team_id.set(team_id.to_string()).is_ok() {
                    info!("Edge serves features of team '{}'", team_id);
                }
            }
        }
    }

    pub fn clear_assignment_caches(&self) {
        self.assigned_cache.clear();
        self.pending_assignments.clear();
    }

    /// Drop the feature's cached and pending assignments. O(1): independent
    /// of how many assignments are cached or queued.
    pub async fn purge_assignments_for_feature(&self, feature_id: &str) {
        self.assigned_cache.remove_feature(feature_id);
        self.pending_assignments.purge_feature(feature_id);
    }

    pub fn purge_all_assignments(&self) {
        self.assigned_cache.clear();
        self.pending_assignments.clear();
    }
}

fn setup_logger() -> actix_web::Result<(), Box<dyn std::error::Error>> {
    log4rs::init_file("log4rs.yaml", Default::default())?;
    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::evaluate_handler,
        handlers::health_handler,
        handlers::ofrep_evaluate_flag,
        handlers::ofrep_evaluate_flags_bulk
    ),
    components(schemas(
        handlers::EvaluateHttpRequest,
        handlers::EvaluateHttpResponse,
        handlers::EvaluateRequestContext,
        handlers::OFREPContext,
        handlers::OFREPEvaluationRequest,
        handlers::OFREPBulkEvaluationRequest,
        handlers::OFREPBulkEvaluationQuery,
        handlers::OFREPSuccessResponse,
        handlers::OFREPErrorResponse,
        handlers::OFREPFlagEvaluation,
        handlers::OFREPBulkEvaluationSuccess,
        handlers::OFREPBulkEvaluationFailure,
        handlers::OFREPEventStream,
        handlers::OFREPEventStreamEndpoint
    )),
    tags(
        (name = "edge", description = "Edge evaluation API"),
        (name = "ofrep", description = "OpenFeature Remote Evaluation Protocol (OFREP) endpoints")
    )
)]
struct ApiDoc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger()?;

    // Load configuration from file and environment variables
    let cfg = config::load_config().map_err(|e| {
        error!("Failed to load configuration: {}", e);
        e
    })?;

    info!("Edge server configuration loaded");
    info!("Backend gRPC: {}", cfg.backend_grpc);
    info!("HTTP address: {}", cfg.http_addr);

    let http_addr: SocketAddr = cfg
        .http_addr
        .parse()
        .expect("invalid HTTP address in configuration");

    // Prepare gRPC client for direct calls (configured endpoint)
    let endpoint = Endpoint::from_shared(cfg.backend_grpc.clone())
        .expect("invalid gRPC address")
        .connect_timeout(cfg.grpc.connect_timeout())
        .timeout(cfg.grpc.timeout())
        .tcp_keepalive(cfg.grpc.tcp_keepalive())
        .http2_keep_alive_interval(cfg.grpc.http2_keepalive())
        .keep_alive_while_idle(cfg.grpc.keep_alive_while_idle)
        .concurrency_limit(cfg.grpc.concurrency_limit)
        .tcp_nodelay(cfg.grpc.tcp_nodelay);
    let channel = endpoint.connect().await?;
    let mut grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
    if matches!(cfg.grpc.compression, config::GrpcCompression::Gzip) {
        grpc_client = grpc_client
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip);
    }

    // Create bounded channel for evaluation events
    let evaluation_event_queue_capacity = cfg.flush.evaluation_event_queue_capacity.max(1);
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(evaluation_event_queue_capacity);

    let state = AppState {
        mapped_cache: Arc::new(MappedFeatureCache::new(cfg.cache.max_capacity)),
        client_info_cache: Arc::new(ClientInfoCache::new(cfg.cache.client_ttl())),
        grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        edge_team_id: Arc::new(std::sync::OnceLock::new()),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        assigned_cache: Arc::new(crate::AssignmentCache::default()),
        pending_assignments: Arc::new(crate::PendingAssignments::default()),
        flush_interval: cfg.flush.assignment_flush_interval(),
        assignment_flush_batch_size: cfg.flush.assignment_flush_batch_size(),
        evaluation_event_tx: event_tx,
        evaluation_flush_interval: cfg.flush.evaluation_flush_interval(),
        evaluation_flush_batch_size: cfg.flush.evaluation_flush_batch_size(),
        evaluation_event_queue_capacity,
        evaluation_event_dropped: Arc::new(AtomicU64::new(0)),
        retry_config: cfg.retry.clone(),
    };

    // On startup, fetch persisted user assignments from backend and warm the cache
    match grpc_client::load_user_assignments(&state).await {
        Ok(n) => info!("loaded {} user assignments from backend", n),
        Err(e) => error!("failed to load user assignments: {}", e),
    }

    // Start stream sync task
    let stream_state = state.clone();
    let grpc_addr_clone = cfg.backend_grpc.clone();
    tokio::spawn(async move { grpc_client::run_stream_task(stream_state, grpc_addr_clone).await });

    // Start periodic flush task
    let flush_state = state.clone();
    tokio::spawn(async move { grpc_client::run_flush_task(flush_state).await });

    // Start periodic evaluation events flush task
    let evaluation_flush_state = state.clone();
    tokio::spawn(async move {
        grpc_client::run_evaluation_flush_task(evaluation_flush_state, event_rx).await
    });

    info!(
        "feature-edge-server listening on {} (HTTP), streaming from {}",
        http_addr, cfg.backend_grpc
    );

    let openapi = ApiDoc::openapi();

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .service(SwaggerUi::new("/docs/{_:.*}").url("/api-doc/openapi.json", openapi.clone()))
            .route("/health", web::get().to(handlers::health_handler))
            .route("/evaluate", web::post().to(handlers::evaluate_handler))
            // OFREP (OpenFeature Remote Evaluation Protocol) endpoints
            .route(
                "/ofrep/v1/evaluate/flags",
                web::post().to(handlers::ofrep_evaluate_flags_bulk),
            )
            .route(
                "/ofrep/v1/evaluate/flags/{key}",
                web::post().to(handlers::ofrep_evaluate_flag),
            )
    })
    .bind(http_addr)?
    .run()
    .await?;

    Ok(())
}

// Keep minimal tests that don't depend on handler logic
#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        let mapped_cache = Arc::new(MappedFeatureCache::new(1000));
        let client_info_cache = Arc::new(ClientInfoCache::new(Duration::from_secs(300)));
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(10);
        AppState {
            mapped_cache,
            client_info_cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "client".into(),
            client_secret: "secret".into(),
            edge_team_id: Arc::new(std::sync::OnceLock::new()),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(crate::AssignmentCache::default()),
            pending_assignments: Arc::new(crate::PendingAssignments::default()),
            flush_interval: Duration::from_secs(60),
            assignment_flush_batch_size: 1000,
            evaluation_event_tx: event_tx,
            evaluation_flush_interval: Duration::from_secs(60),
            evaluation_flush_batch_size: 500,
            evaluation_event_queue_capacity: 10,
            evaluation_event_dropped: Arc::new(AtomicU64::new(0)),
            retry_config: config::RetryConfig::default(),
        }
    }

    fn sticky_true() -> CachedAssignment {
        CachedAssignment {
            value: serde_json::json!(true),
            variant: None,
            reason: evaluation_engine::EvaluationReason::TargetingMatch,
        }
    }

    fn pending(user_id: &str, feature_id: &str) -> crate::grpc_client::UserAssignment {
        crate::grpc_client::UserAssignment {
            user_id: user_id.into(),
            feature_id: feature_id.into(),
            environment_id: "env-1".into(),
            assigned: true,
            variant: None,
        }
    }

    /// Purge cost must not grow with the total number of assignments.
    #[tokio::test]
    async fn purge_cost_does_not_depend_on_total_assignments() {
        const FEATURES: usize = 1_000;
        const USERS_PER_FEATURE: usize = 100;
        let state = test_state();
        for feature in 0..FEATURES {
            let feature_id = format!("feature-{feature}");
            for user in 0..USERS_PER_FEATURE {
                let user_id = format!("user-{user}");
                state
                    .assigned_cache
                    .insert(&user_id, &feature_id, "env-1", sticky_true());
                state
                    .pending_assignments
                    .push(pending(&user_id, &feature_id));
            }
        }

        let started = std::time::Instant::now();
        for feature in 0..FEATURES {
            state
                .purge_assignments_for_feature(&format!("feature-{feature}"))
                .await;
        }
        let elapsed = started.elapsed();

        assert!(state.assigned_cache.is_empty());
        assert!(state.pending_assignments.pop().is_none());
        assert!(
            elapsed < Duration::from_secs(1),
            "{FEATURES} purges over {} assignments took {elapsed:?}",
            FEATURES * USERS_PER_FEATURE
        );
    }

    #[tokio::test]
    async fn purge_keeps_other_features_and_later_assignments() {
        let state = test_state();
        state
            .assigned_cache
            .insert("user-1", "x", "env-1", sticky_true());
        state
            .assigned_cache
            .insert("user-1", "y", "env-1", sticky_true());
        state.pending_assignments.push(pending("user-1", "x"));
        state.pending_assignments.push(pending("user-1", "y"));

        state.purge_assignments_for_feature("x").await;
        // Assignments made after the purge are kept.
        state
            .assigned_cache
            .insert("user-2", "x", "env-1", sticky_true());
        state.pending_assignments.push(pending("user-2", "x"));
        // Purging a feature without assignments is a no-op.
        state.purge_assignments_for_feature("unknown").await;

        assert!(state.assigned_cache.get("user-1", "x", "env-1").is_none());
        assert!(state.assigned_cache.get("user-1", "y", "env-1").is_some());
        assert!(state.assigned_cache.get("user-2", "x", "env-1").is_some());

        let mut remaining = Vec::new();
        while let Some(assignment) = state.pending_assignments.pop() {
            remaining.push((assignment.user_id, assignment.feature_id));
        }
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                ("user-1".to_string(), "y".to_string()),
                ("user-2".to_string(), "x".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn purge_all_assignments_clears_everything() {
        let state = test_state();
        state
            .assigned_cache
            .insert("user-1", "x", "env-1", sticky_true());
        state.pending_assignments.push(pending("user-1", "x"));

        state.purge_all_assignments();

        assert!(state.assigned_cache.is_empty());
        assert!(state.pending_assignments.pop().is_none());
        // The queue still accepts new assignments afterwards.
        state.pending_assignments.push(pending("user-2", "x"));
        assert!(state.pending_assignments.pop().is_some());
    }

    #[tokio::test]
    async fn test_purge_assignments_for_feature() {
        let state = test_state();

        let feature_id = "fea-123";
        state.assigned_cache.insert(
            "user-1",
            feature_id,
            "env-1",
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
                reason: evaluation_engine::EvaluationReason::TargetingMatch,
            },
        );
        state.assigned_cache.insert(
            "user-2",
            feature_id,
            "env-1",
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
                reason: evaluation_engine::EvaluationReason::TargetingMatch,
            },
        );
        state.assigned_cache.insert(
            "user-3",
            "other",
            "env",
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
                reason: evaluation_engine::EvaluationReason::TargetingMatch,
            },
        );

        // Lock-free push!
        state
            .pending_assignments
            .push(crate::grpc_client::UserAssignment {
                user_id: "user-1".into(),
                feature_id: feature_id.into(),
                environment_id: "env-1".into(),
                assigned: true,
                variant: None,
            });
        state
            .pending_assignments
            .push(crate::grpc_client::UserAssignment {
                user_id: "user-9".into(),
                feature_id: "other".into(),
                environment_id: "env-1".into(),
                assigned: true,
                variant: None,
            });

        state.purge_assignments_for_feature(feature_id).await;

        assert_eq!(state.assigned_cache.len(), 1);
        assert!(
            state
                .assigned_cache
                .get("user-1", feature_id, "env-1")
                .is_none()
        );
        assert!(
            state
                .assigned_cache
                .get("user-2", feature_id, "env-1")
                .is_none()
        );
        assert!(state.assigned_cache.get("user-3", "other", "env").is_some());

        // Drain queue to check contents
        let mut remaining = Vec::new();
        while let Some(assignment) = state.pending_assignments.pop() {
            remaining.push(assignment);
        }
        assert_eq!(remaining.len(), 1);
        assert!(
            remaining
                .iter()
                .all(|assignment| assignment.feature_id != feature_id)
        );
    }

    #[actix_web::test]
    async fn test_mapped_feature_cache_operations() {
        let mapped_cache = MappedFeatureCache::new(100);

        // Create a sample engine feature
        let engine_feature = Arc::new(evaluation_engine::Feature {
            id: "test_id".to_string(),
            key: "test_key".to_string(),
            feature_type: "Simple".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        });

        // Test insert and get
        mapped_cache.insert("team-1", engine_feature.clone()).await;
        mapped_cache.run_pending_tasks().await;
        assert_eq!(mapped_cache.entry_count(), 1);

        let retrieved = mapped_cache.get("test_key").await;
        assert!(retrieved.is_some());
        let retrieved_feature = retrieved.unwrap();
        assert!(retrieved_feature.enabled);

        // Test cache hit (should return the same Arc)
        let retrieved_again = mapped_cache.get("test_key").await.unwrap();
        assert!(Arc::ptr_eq(&retrieved_feature, &retrieved_again));

        // Test invalidate
        mapped_cache.invalidate("test_key").await;
        mapped_cache.run_pending_tasks().await;
        assert!(mapped_cache.get("test_key").await.is_none());

        // Successful inserts should clear stale negative-cache entries.
        mapped_cache.add_negative("neg_key").await;
        assert!(mapped_cache.is_negative_cached("neg_key").await);

        let recovered_feature = Arc::new(evaluation_engine::Feature {
            id: "neg_id".to_string(),
            key: "neg_key".to_string(),
            feature_type: "Simple".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        });
        mapped_cache
            .insert_with_dependencies(
                "team-1",
                recovered_feature.clone(),
                vec!["dep-1".to_string()],
            )
            .await;
        mapped_cache.run_pending_tasks().await;
        assert!(mapped_cache.get("neg_key").await.is_some());
        assert!(mapped_cache.get_by_id("neg_id").await.is_some());
        assert!(!mapped_cache.is_negative_cached("neg_key").await);
        assert_eq!(
            mapped_cache.get_dependency_ids("neg_id").await,
            vec!["dep-1".to_string()]
        );

        // Test non-existent key
        assert!(mapped_cache.get("non_existent").await.is_none());
    }

    #[test]
    fn test_ofrep_context_serialization() {
        use handlers::OFREPContext;

        // Test that OFREPContext properly deserializes with both targetingKey and attributes
        let json_str = r#"{"targetingKey":"user-123","environment_id":"env-prod","country":"US"}"#;
        let context: OFREPContext = serde_json::from_str(json_str).unwrap();

        assert_eq!(context.targeting_key, "user-123");
        assert_eq!(
            context.attributes.get("environment_id").unwrap(),
            "env-prod"
        );
        assert_eq!(context.attributes.get("country").unwrap(), "US");

        // Test that it works with minimal attributes
        let minimal_json = r#"{"targetingKey":"user-456"}"#;
        let minimal_context: OFREPContext = serde_json::from_str(minimal_json).unwrap();
        assert_eq!(minimal_context.targeting_key, "user-456");
        assert!(minimal_context.attributes.is_empty());
    }

    #[test]
    fn test_evaluate_request_context_deserialization() {
        use handlers::EvaluateRequestContext;

        let json_str = r#"{"bucketingKey":"user-123","environment_id":"env-prod","country":"US"}"#;
        let context: EvaluateRequestContext = serde_json::from_str(json_str).unwrap();

        assert_eq!(context.bucketing_key, "user-123");
        assert_eq!(
            context.attributes.get("environment_id").unwrap(),
            "env-prod"
        );
        assert_eq!(context.attributes.get("country").unwrap(), "US");
    }

    #[test]
    fn test_ofrep_response_serialization() {
        use handlers::{OFREPErrorResponse, OFREPSuccessResponse};

        // Test success response
        let success = OFREPSuccessResponse {
            key: "test-flag".to_string(),
            value: Some(serde_json::json!(true)),
            reason: "TARGETING_MATCH".to_string(),
            variant: Some("treatment".to_string()),
            metadata: None,
        };

        let json = serde_json::to_string(&success).unwrap();
        assert!(json.contains("\"key\":\"test-flag\""));
        assert!(json.contains("\"value\":true"));
        assert!(json.contains("\"reason\":\"TARGETING_MATCH\""));

        // Test error response
        let error = OFREPErrorResponse {
            key: "test-flag".to_string(),
            error_code: "FLAG_NOT_FOUND".to_string(),
            error_details: Some("The requested flag does not exist".to_string()),
            metadata: None,
        };

        let error_json = serde_json::to_string(&error).unwrap();
        assert!(error_json.contains("\"key\":\"test-flag\""));
        assert!(error_json.contains("\"errorCode\":\"FLAG_NOT_FOUND\""));
        assert!(error_json.contains("\"errorDetails\":"));
    }

    #[test]
    fn test_ofrep_evaluation_reasons_are_valid() {
        // Verify all evaluation reasons match OFREP spec (using JSON serialization)
        let valid_reasons = ["STATIC", "TARGETING_MATCH", "SPLIT", "DISABLED", "UNKNOWN"];

        // Test that our engine reasons serialize correctly to JSON (SCREAMING_SNAKE_CASE)
        let reason1 = evaluation_engine::EvaluationReason::Static;
        let json1 = serde_json::to_string(&reason1)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_reasons.contains(&json1.as_str()),
            "Reason '{}' not in OFREP spec",
            json1
        );

        let reason2 = evaluation_engine::EvaluationReason::TargetingMatch;
        let json2 = serde_json::to_string(&reason2)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_reasons.contains(&json2.as_str()),
            "Reason '{}' not in OFREP spec",
            json2
        );

        let reason3 = evaluation_engine::EvaluationReason::Split;
        let json3 = serde_json::to_string(&reason3)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_reasons.contains(&json3.as_str()),
            "Reason '{}' not in OFREP spec",
            json3
        );

        let reason4 = evaluation_engine::EvaluationReason::Disabled;
        let json4 = serde_json::to_string(&reason4)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_reasons.contains(&json4.as_str()),
            "Reason '{}' not in OFREP spec",
            json4
        );

        let reason5 = evaluation_engine::EvaluationReason::Unknown;
        let json5 = serde_json::to_string(&reason5)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_reasons.contains(&json5.as_str()),
            "Reason '{}' not in OFREP spec",
            json5
        );
    }

    #[test]
    fn test_ofrep_error_codes_are_valid() {
        // Verify all error codes match OFREP spec (using JSON serialization)
        let valid_codes = [
            "PARSE_ERROR",
            "TARGETING_KEY_MISSING",
            "INVALID_CONTEXT",
            "GENERAL",
            "FLAG_NOT_FOUND",
        ];

        let code1 = evaluation_engine::ErrorCode::ParseError;
        let json1 = serde_json::to_string(&code1)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_codes.contains(&json1.as_str()),
            "Error code '{}' not in OFREP spec",
            json1
        );

        let code2 = evaluation_engine::ErrorCode::TargetingKeyMissing;
        let json2 = serde_json::to_string(&code2)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_codes.contains(&json2.as_str()),
            "Error code '{}' not in OFREP spec",
            json2
        );

        let code3 = evaluation_engine::ErrorCode::InvalidContext;
        let json3 = serde_json::to_string(&code3)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_codes.contains(&json3.as_str()),
            "Error code '{}' not in OFREP spec",
            json3
        );

        let code4 = evaluation_engine::ErrorCode::General;
        let json4 = serde_json::to_string(&code4)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_codes.contains(&json4.as_str()),
            "Error code '{}' not in OFREP spec",
            json4
        );

        let code5 = evaluation_engine::ErrorCode::FlagNotFound;
        let json5 = serde_json::to_string(&code5)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(
            valid_codes.contains(&json5.as_str()),
            "Error code '{}' not in OFREP spec",
            json5
        );
    }
}
