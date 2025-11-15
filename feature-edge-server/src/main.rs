use actix_web::{App, HttpServer, web};
use std::{net::SocketAddr, sync::Arc, time::Duration};
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
}

pub struct ClientInfoCache {
    // Cache with TTL for client info
    cache: moka::future::Cache<String, pb::GetClientInfoResponse>,
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
        }
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
    cache: Arc<FeatureCache>,
    mapped_cache: Arc<MappedFeatureCache>,
    client_info_cache: Arc<ClientInfoCache>,
    grpc: Arc<
        tokio::sync::Mutex<
            pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
        >,
    >,
    client_id: String,
    client_secret: String,
    connected: Arc<std::sync::atomic::AtomicBool>,
    // Sticky assignments cache with variant information and pending flush queue
    assigned_cache: Arc<dashmap::DashMap<String, CachedAssignment>>,
    pending_assignments: Arc<tokio::sync::Mutex<Vec<grpc_client::UserAssignment>>>,
    flush_interval: Duration,
    // Evaluation events tracking (using channel for lock-free writes)
    evaluation_event_tx: tokio::sync::mpsc::UnboundedSender<EvaluationEvent>,
    evaluation_flush_interval: Duration,
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
}

pub struct FeatureCache {
    // LRU cache with configurable max capacity
    by_key: moka::future::Cache<String, pb::FeatureFull>,
    // Secondary index for looking up by ID (also LRU)
    by_id: moka::future::Cache<String, String>,
}

impl FeatureCache {
    /// Create a new FeatureCache with the specified maximum capacity
    /// When capacity is exceeded, least recently used items are evicted
    pub fn new(max_capacity: u64) -> Self {
        tracing::info!("Initializing FeatureCache with max_capacity={}", max_capacity);
        Self {
            by_key: moka::future::Cache::new(max_capacity),
            by_id: moka::future::Cache::new(max_capacity),
        }
    }

    pub async fn upsert(&self, f: pb::FeatureFull) {
        let key = f.key.clone();
        let id = f.id.clone();

        // Insert into both caches
        self.by_key.insert(key.clone(), f).await;
        self.by_id.insert(id, key).await;
    }

    pub async fn delete_by_key(&self, key: &str) -> Option<String> {
        if let Some(f) = self.by_key.get(key).await {
            let feature_id = f.id.clone();

            // Remove from both caches
            self.by_key.invalidate(key).await;
            self.by_id.invalidate(&feature_id).await;

            Some(feature_id)
        } else {
            None
        }
    }

    pub async fn get_by_key(&self, key: &str) -> Option<pb::FeatureFull> {
        self.by_key.get(key).await
    }

    /// Get all cached feature keys
    /// Note: This iterates over all cached entries, use sparingly
    pub async fn get_all_keys(&self) -> Vec<String> {
        // Run a sync operation to collect all keys
        self.by_key.run_pending_tasks().await;

        // Iterate over cache entries
        let mut keys = Vec::new();
        self.by_key.iter().for_each(|(key, _)| {
            keys.push(key.as_ref().clone());
        });
        keys
    }

    /// Get current cache size (number of entries)
    pub fn entry_count(&self) -> u64 {
        self.by_key.entry_count()
    }

    /// Get cache statistics
    pub async fn weighted_size(&self) -> u64 {
        self.by_key.weighted_size()
    }

    /// Run pending cache tasks (useful for testing)
    #[cfg(test)]
    pub async fn run_pending_tasks(&self) {
        self.by_key.run_pending_tasks().await;
        self.by_id.run_pending_tasks().await;
    }
}

/// Cache for pre-mapped engine::Feature to avoid repeated allocations
pub struct MappedFeatureCache {
    // Cache with Arc for zero-cost cloning
    cache: moka::future::Cache<String, Arc<evaluation_engine::Feature>>,
}

impl MappedFeatureCache {
    pub fn new(max_capacity: u64) -> Self {
        tracing::info!("Initializing MappedFeatureCache with max_capacity={}", max_capacity);
        Self {
            cache: moka::future::Cache::new(max_capacity),
        }
    }

    pub async fn get(&self, key: &str) -> Option<Arc<evaluation_engine::Feature>> {
        self.cache.get(key).await
    }

    pub async fn insert(&self, key: String, feature: Arc<evaluation_engine::Feature>) {
        self.cache.insert(key, feature).await;
    }

    pub async fn invalidate(&self, key: &str) {
        self.cache.invalidate(key).await;
    }

    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Run pending cache tasks (useful for testing)
    #[cfg(test)]
    pub async fn run_pending_tasks(&self) {
        self.cache.run_pending_tasks().await;
    }
}

impl AppState {
    pub async fn purge_assignments_for_feature(&self, feature_id: &str) {
        // DashMap allows concurrent iteration and removal
        self.assigned_cache.retain(|key, _| {
            key.split('|').nth(1) != Some(feature_id)
        });

        {
            let mut pending = self.pending_assignments.lock().await;
            pending.retain(|assignment| assignment.feature_id != feature_id);
        }
    }
}

fn setup_logger() -> actix_web::Result<(), Box<dyn std::error::Error>> {
    log4rs::init_file("log4rs.yaml", Default::default())?;
    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    paths(handlers::evaluate_handler, handlers::health_handler),
    components(schemas(handlers::EvaluateHttpRequest, handlers::EvaluateHttpResponse, handlers::EvaluateContext)),
    tags((name = "edge", description = "Edge evaluation API"))
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
    let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

    // Create unbounded channel for evaluation events
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

    let state = AppState {
        cache: Arc::new(FeatureCache::new(cfg.cache.max_capacity)),
        mapped_cache: Arc::new(MappedFeatureCache::new(cfg.cache.max_capacity)),
        client_info_cache: Arc::new(ClientInfoCache::new(cfg.cache.client_ttl())),
        grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        assigned_cache: Arc::new(dashmap::DashMap::new()),
        pending_assignments: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        flush_interval: cfg.flush.assignment_flush_interval(),
        evaluation_event_tx: event_tx,
        evaluation_flush_interval: cfg.flush.evaluation_flush_interval(),
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
    tokio::spawn(
        async move { grpc_client::run_evaluation_flush_task(evaluation_flush_state, event_rx).await },
    );

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

    #[actix_web::test]
    async fn test_feature_cache_ops() {
        let cache = FeatureCache::new(1000);
        let f1 = pb::FeatureFull {
            id: "test_id".to_string(),
            key: "test_key".to_string(),
            description: String::new(),
            feature_type: String::new(),
            team_id: String::new(),
            created_at: String::new(),
            active: true,
            kill_switch_enabled: false,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: String::new(),
            stages: vec![],
            dependencies: vec![],
            variants: vec![],
        };

        cache.upsert(f1.clone()).await;
        assert!(cache.get_by_key("test_key").await.is_some());
        let removed = cache.delete_by_key("test_key").await;
        assert_eq!(removed.as_deref(), Some("test_id"));
        assert!(cache.get_by_key("test_key").await.is_none());
    }

    #[tokio::test]
    async fn test_purge_assignments_for_feature() {
        let cache = Arc::new(FeatureCache::new(1000));
        let mapped_cache = Arc::new(MappedFeatureCache::new(1000));
        let client_info_cache = Arc::new(ClientInfoCache::new(Duration::from_secs(300)));
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState {
            cache,
            mapped_cache,
            client_info_cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "client".into(),
            client_secret: "secret".into(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(dashmap::DashMap::new()),
            pending_assignments: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            flush_interval: Duration::from_secs(60),
            evaluation_event_tx: event_tx,
            evaluation_flush_interval: Duration::from_secs(60),
            retry_config: config::RetryConfig::default(),
        };

        let feature_id = "fea-123";
        state.assigned_cache.insert(
            format!("user-1|{}|env-1", feature_id),
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
            },
        );
        state.assigned_cache.insert(
            format!("user-2|{}|env-1", feature_id),
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
            },
        );
        state.assigned_cache.insert(
            "user-3|other|env".to_string(),
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
            },
        );

        {
            let mut pending = state.pending_assignments.lock().await;
            pending.push(crate::grpc_client::UserAssignment {
                user_id: "user-1".into(),
                feature_id: feature_id.into(),
                environment_id: "env-1".into(),
                assigned: true,
                variant: None,
            });
            pending.push(crate::grpc_client::UserAssignment {
                user_id: "user-9".into(),
                feature_id: "other".into(),
                environment_id: "env-1".into(),
                assigned: true,
                variant: None,
            });
        }

        state.purge_assignments_for_feature(feature_id).await;

        assert_eq!(state.assigned_cache.len(), 1);
        assert!(state.assigned_cache.iter().all(|entry| !entry.key().contains(feature_id)));

        let pending = state.pending_assignments.lock().await;
        assert_eq!(pending.len(), 1);
        assert!(
            pending
                .iter()
                .all(|assignment| assignment.feature_id != feature_id)
        );
    }

    #[actix_web::test]
    async fn test_cache_with_capacity_limit() {
        // Create a cache with specific capacity
        let cache = FeatureCache::new(100);

        // Insert features
        for i in 1..=10 {
            let feature = pb::FeatureFull {
                id: format!("id_{}", i),
                key: format!("key_{}", i),
                description: String::new(),
                feature_type: String::new(),
                team_id: String::new(),
                created_at: String::new(),
                active: true,
                kill_switch_enabled: false,
                kill_switch_activated_at: String::new(),
                rollback_scheduled_at: String::new(),
                stages: vec![],
                dependencies: vec![],
                variants: vec![],
            };
            cache.upsert(feature).await;
        }

        // Run pending tasks
        cache.run_pending_tasks().await;

        // Verify entries are cached
        assert!(cache.entry_count() > 0, "Cache should have entries");
        assert!(cache.entry_count() <= 100, "Cache should respect max capacity");

        // Verify we can retrieve cached items
        assert!(cache.get_by_key("key_1").await.is_some());
        assert!(cache.get_by_key("key_5").await.is_some());
        assert!(cache.get_by_key("key_10").await.is_some());

        // Verify we can get all keys
        let keys = cache.get_all_keys().await;
        assert!(keys.len() >= 1, "Should have cached keys");

        // Note: Exact LRU eviction behavior is tested by moka's own tests
        // We just verify the cache works correctly with basic operations
    }

    #[actix_web::test]
    async fn test_cache_get_all_keys() {
        let cache = FeatureCache::new(100);

        // Insert multiple features
        for i in 1..=5 {
            let feature = pb::FeatureFull {
                id: format!("id_{}", i),
                key: format!("feature_{}", i),
                description: String::new(),
                feature_type: String::new(),
                team_id: String::new(),
                created_at: String::new(),
                active: true,
                kill_switch_enabled: false,
                kill_switch_activated_at: String::new(),
                rollback_scheduled_at: String::new(),
                stages: vec![],
                dependencies: vec![],
                variants: vec![],
            };
            cache.upsert(feature).await;
        }

        // Get all keys
        let keys = cache.get_all_keys().await;

        // Should have 5 keys
        assert_eq!(keys.len(), 5);

        // All expected keys should be present
        for i in 1..=5 {
            assert!(keys.contains(&format!("feature_{}", i)));
        }

        // Delete one feature
        cache.delete_by_key("feature_3").await;

        // Get all keys again
        let keys_after_delete = cache.get_all_keys().await;

        // Should have 4 keys now
        assert_eq!(keys_after_delete.len(), 4);
        assert!(!keys_after_delete.contains(&"feature_3".to_string()));
    }

    #[actix_web::test]
    async fn test_cache_entry_count() {
        let cache = FeatureCache::new(100);

        // Initially empty
        cache.run_pending_tasks().await;
        assert_eq!(cache.entry_count(), 0);

        // Add features
        for i in 1..=10 {
            let feature = pb::FeatureFull {
                id: format!("id_{}", i),
                key: format!("key_{}", i),
                description: String::new(),
                feature_type: String::new(),
                team_id: String::new(),
                created_at: String::new(),
                active: true,
                kill_switch_enabled: false,
                kill_switch_activated_at: String::new(),
                rollback_scheduled_at: String::new(),
                stages: vec![],
                dependencies: vec![],
                variants: vec![],
            };
            cache.upsert(feature).await;
        }

        // Run pending tasks to ensure all inserts are processed
        cache.run_pending_tasks().await;

        // Should have 10 entries
        assert_eq!(cache.entry_count(), 10);

        // Delete 3 features
        let _ = cache.delete_by_key("key_1").await;
        let _ = cache.delete_by_key("key_5").await;
        let _ = cache.delete_by_key("key_10").await;

        // Run pending tasks to process deletes
        cache.run_pending_tasks().await;

        // Should have 7 entries
        assert_eq!(cache.entry_count(), 7);
    }

    #[actix_web::test]
    async fn test_mapped_feature_cache_operations() {
        let mapped_cache = MappedFeatureCache::new(100);

        // Create a sample engine feature
        let engine_feature = Arc::new(evaluation_engine::Feature {
            enabled: true,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        });

        // Test insert and get
        mapped_cache.insert("test_key".to_string(), engine_feature.clone()).await;
        mapped_cache.run_pending_tasks().await;
        assert_eq!(mapped_cache.entry_count(), 1);

        let retrieved = mapped_cache.get("test_key").await;
        assert!(retrieved.is_some());
        let retrieved_feature = retrieved.unwrap();
        assert_eq!(retrieved_feature.enabled, true);

        // Test cache hit (should return the same Arc)
        let retrieved_again = mapped_cache.get("test_key").await.unwrap();
        assert!(Arc::ptr_eq(&retrieved_feature, &retrieved_again));

        // Test invalidate
        mapped_cache.invalidate("test_key").await;
        mapped_cache.run_pending_tasks().await;
        assert!(mapped_cache.get("test_key").await.is_none());

        // Test non-existent key
        assert!(mapped_cache.get("non_existent").await.is_none());
    }

    #[actix_web::test]
    async fn test_cache_delete_by_key_removes_from_both_indexes() {
        let cache = FeatureCache::new(100);

        let feature = pb::FeatureFull {
            id: "test_id_123".to_string(),
            key: "test_key_456".to_string(),
            description: String::new(),
            feature_type: String::new(),
            team_id: String::new(),
            created_at: String::new(),
            active: true,
            kill_switch_enabled: false,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: String::new(),
            stages: vec![],
            dependencies: vec![],
            variants: vec![],
        };

        cache.upsert(feature).await;
        cache.run_pending_tasks().await;

        // Verify it's in cache
        assert!(cache.get_by_key("test_key_456").await.is_some());
        assert_eq!(cache.entry_count(), 1);

        // Delete by key
        let deleted_id = cache.delete_by_key("test_key_456").await;

        // Should return the feature ID
        assert_eq!(deleted_id.as_deref(), Some("test_id_123"));

        // Run pending tasks to process the delete
        cache.run_pending_tasks().await;

        // Feature should no longer be in cache
        assert!(cache.get_by_key("test_key_456").await.is_none());
        assert_eq!(cache.entry_count(), 0);

        // Deleting non-existent key should return None
        let not_found = cache.delete_by_key("non_existent").await;
        assert!(not_found.is_none());
    }
}
