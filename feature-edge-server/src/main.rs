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
    // Sticky assignments cache with variant information and pending flush queue (lock-free!)
    assigned_cache: Arc<dashmap::DashMap<String, CachedAssignment>>,
    pending_assignments: Arc<crossbeam::queue::SegQueue<grpc_client::UserAssignment>>,
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

/// Cache for pre-mapped engine::Feature to avoid repeated allocations
pub struct MappedFeatureCache {
    // Primary cache: feature_key -> Arc<Feature>
    by_key: moka::future::Cache<String, Arc<evaluation_engine::Feature>>,
    // Secondary index: feature_id -> feature_key
    by_id: moka::future::Cache<String, String>,
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
        }
    }

    /// Get feature by key
    pub async fn get(&self, key: &str) -> Option<Arc<evaluation_engine::Feature>> {
        self.by_key.get(key).await
    }

    /// Get feature by ID (using secondary index)
    pub async fn get_by_id(&self, id: &str) -> Option<Arc<evaluation_engine::Feature>> {
        let key = self.by_id.get(id).await?;
        self.by_key.get(&key).await
    }

    /// Insert feature into cache (updates both indices)
    pub async fn insert(&self, feature: Arc<evaluation_engine::Feature>) {
        let key = feature.key.clone();
        let id = feature.id.clone();

        self.by_key.insert(key.clone(), feature).await;
        self.by_id.insert(id, key).await;
    }

    /// Invalidate feature by key
    pub async fn invalidate(&self, key: &str) {
        // Get the feature to find its ID before invalidating
        if let Some(feature) = self.by_key.get(key).await {
            self.by_id.invalidate(&feature.id).await;
        }
        self.by_key.invalidate(key).await;
    }

    /// Delete feature by key and return its ID
    pub async fn delete_by_key(&self, key: &str) -> Option<String> {
        let feature = self.by_key.get(key).await?;
        let id = feature.id.clone();

        self.by_key.invalidate(key).await;
        self.by_id.invalidate(&id).await;

        Some(id)
    }

    /// Get all feature keys
    pub async fn get_all_keys(&self) -> Vec<String> {
        self.by_key.iter().map(|(k, _)| k.to_string()).collect()
    }

    pub fn entry_count(&self) -> u64 {
        self.by_key.entry_count()
    }

    /// Run pending cache tasks (useful for testing)
    #[cfg(test)]
    pub async fn run_pending_tasks(&self) {
        self.by_key.run_pending_tasks().await;
        self.by_id.run_pending_tasks().await;
    }
}

impl AppState {
    pub async fn purge_assignments_for_feature(&self, feature_id: &str) {
        // DashMap allows concurrent iteration and removal
        self.assigned_cache
            .retain(|key, _| key.split('|').nth(1) != Some(feature_id));

        // SegQueue doesn't have retain, so we drain, filter, and re-add
        let mut to_keep = Vec::new();
        while let Some(assignment) = self.pending_assignments.pop() {
            if assignment.feature_id != feature_id {
                to_keep.push(assignment);
            }
        }
        // Re-add the assignments we want to keep
        for assignment in to_keep {
            self.pending_assignments.push(assignment);
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
        mapped_cache: Arc::new(MappedFeatureCache::new(cfg.cache.max_capacity)),
        client_info_cache: Arc::new(ClientInfoCache::new(cfg.cache.client_ttl())),
        grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        assigned_cache: Arc::new(dashmap::DashMap::new()),
        pending_assignments: Arc::new(crossbeam::queue::SegQueue::new()),
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

    #[tokio::test]
    async fn test_purge_assignments_for_feature() {
        let mapped_cache = Arc::new(MappedFeatureCache::new(1000));
        let client_info_cache = Arc::new(ClientInfoCache::new(Duration::from_secs(300)));
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState {
            mapped_cache,
            client_info_cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "client".into(),
            client_secret: "secret".into(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(dashmap::DashMap::new()),
            pending_assignments: Arc::new(crossbeam::queue::SegQueue::new()),
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
                .iter()
                .all(|entry| !entry.key().contains(feature_id))
        );

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
        mapped_cache.insert(engine_feature.clone()).await;
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
}
