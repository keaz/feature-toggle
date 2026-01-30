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
    pub reason: evaluation_engine::EvaluationReason,
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
    pub variant_value: Option<serde_json::Value>,
}

/// Cache for pre-mapped engine::Feature to avoid repeated allocations
pub struct MappedFeatureCache {
    // Primary cache: feature_key -> Arc<Feature>
    by_key: moka::future::Cache<String, Arc<evaluation_engine::Feature>>,
    // Secondary index: feature_id -> feature_key
    by_id: moka::future::Cache<String, String>,
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
            negative_cache: moka::future::Cache::builder()
                .time_to_live(std::time::Duration::from_secs(60))
                .max_capacity(10000)
                .build(),
        }
    }

    /// Get feature by key
    pub async fn get(&self, key: &str) -> Option<Arc<evaluation_engine::Feature>> {
        self.by_key.get(key).await
    }

    /// Get feature by key, or compute and insert it if not present.
    /// This uses moka's built-in request coalescing - if multiple concurrent
    /// requests ask for the same uncached key, only one will execute the
    /// init function while others wait for the result.
    pub async fn optionally_get_with<F, Fut>(
        &self,
        key: String,
        init: F,
    ) -> Option<Arc<evaluation_engine::Feature>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<Arc<evaluation_engine::Feature>>>,
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
    paths(handlers::evaluate_handler, handlers::health_handler, handlers::ofrep_evaluate_flag),
    components(schemas(
        handlers::EvaluateHttpRequest,
        handlers::EvaluateHttpResponse,
        handlers::EvaluateContext,
        handlers::OFREPContext,
        handlers::OFREPEvaluationRequest,
        handlers::OFREPSuccessResponse,
        handlers::OFREPErrorResponse
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
            // OFREP (OpenFeature Remote Evaluation Protocol) endpoint
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
                reason: evaluation_engine::EvaluationReason::TargetingMatch,
            },
        );
        state.assigned_cache.insert(
            format!("user-2|{}|env-1", feature_id),
            CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
                reason: evaluation_engine::EvaluationReason::TargetingMatch,
            },
        );
        state.assigned_cache.insert(
            "user-3|other|env".to_string(),
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

    #[test]
    fn test_ofrep_context_serialization() {
        use handlers::{OFREPContext, OFREPEvaluationRequest};
        use std::collections::HashMap;

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
    fn test_ofrep_response_serialization() {
        use handlers::{OFREPErrorResponse, OFREPSuccessResponse};

        // Test success response
        let success = OFREPSuccessResponse {
            key: None, // Key should be None for single eval success
            value: Some(serde_json::json!(true)),
            reason: "TARGETING_MATCH".to_string(),
            variant: Some("treatment".to_string()),
            metadata: None,
        };

        let json = serde_json::to_string(&success).unwrap();
        assert!(!json.contains("\"key\":")); // key should be omitted when None
        assert!(json.contains("\"value\":true"));
        assert!(json.contains("\"reason\":\"TARGETING_MATCH\""));

        // Test error response
        let error = OFREPErrorResponse {
            key: "test-flag".to_string(),
            error_code: "FLAG_NOT_FOUND".to_string(),
            error_details: Some("The requested flag does not exist".to_string()),
        };

        let error_json = serde_json::to_string(&error).unwrap();
        assert!(error_json.contains("\"key\":\"test-flag\""));
        assert!(error_json.contains("\"errorCode\":\"FLAG_NOT_FOUND\""));
        assert!(error_json.contains("\"errorDetails\":"));
    }

    #[test]
    fn test_ofrep_evaluation_reasons_are_valid() {
        // Verify all evaluation reasons match OFREP spec (using JSON serialization)
        let valid_reasons = vec!["STATIC", "TARGETING_MATCH", "SPLIT", "DISABLED", "UNKNOWN"];

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
        let valid_codes = vec![
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
