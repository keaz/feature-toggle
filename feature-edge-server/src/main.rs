use actix_web::{App, HttpServer, web};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
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

#[derive(Clone)]
pub struct AppState {
    cache: Arc<FeatureCache>,
    grpc: Arc<
        tokio::sync::Mutex<
            pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
        >,
    >,
    client_id: String,
    client_secret: String,
    connected: Arc<std::sync::atomic::AtomicBool>,
    // Sticky assignments cache with variant information and pending flush queue
    assigned_cache: Arc<RwLock<std::collections::HashMap<String, CachedAssignment>>>,
    pending_assignments: Arc<RwLock<Vec<grpc_client::UserAssignment>>>,
    flush_interval: Duration,
    // Evaluation events tracking
    pending_evaluation_events: Arc<RwLock<Vec<EvaluationEvent>>>,
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

#[derive(Default)]
pub struct FeatureCache {
    by_key: RwLock<HashMap<String, pb::FeatureFull>>, // key -> feature
    by_id: RwLock<HashMap<String, String>>,           // id -> key
}

impl FeatureCache {
    pub async fn upsert(&self, f: pb::FeatureFull) {
        let key = f.key.clone();
        let id = f.id.clone();
        {
            let mut by_key = self.by_key.write().await;
            by_key.insert(key.clone(), f);
        }
        {
            let mut by_id = self.by_id.write().await;
            by_id.insert(id, key);
        }
    }

    pub async fn delete_by_key(&self, key: &str) -> Option<String> {
        let mut by_key = self.by_key.write().await;
        if let Some(f) = by_key.remove(key) {
            let feature_id = f.id.clone();
            let mut by_id = self.by_id.write().await;
            by_id.remove(&f.id);
            Some(feature_id)
        } else {
            None
        }
    }

    pub async fn get_by_key(&self, key: &str) -> Option<pb::FeatureFull> {
        let by_key = self.by_key.read().await;
        by_key.get(key).cloned()
    }

    /// Get all cached feature keys
    pub async fn get_all_keys(&self) -> Vec<String> {
        let by_key = self.by_key.read().await;
        by_key.keys().cloned().collect()
    }
}

impl AppState {
    pub async fn purge_assignments_for_feature(&self, feature_id: &str) {
        {
            let mut cache = self.assigned_cache.write().await;
            let keys: Vec<String> = cache
                .keys()
                .filter(|entry| entry.split('|').nth(1) == Some(feature_id))
                .cloned()
                .collect();
            for key in keys {
                cache.remove(&key);
            }
        }

        {
            let mut pending = self.pending_assignments.write().await;
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

    let state = AppState {
        cache: Arc::new(FeatureCache::default()),
        grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        assigned_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
        pending_assignments: Arc::new(RwLock::new(Vec::new())),
        flush_interval: cfg.flush.assignment_flush_interval(),
        pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
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
        async move { grpc_client::run_evaluation_flush_task(evaluation_flush_state).await },
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
        let cache = FeatureCache::default();
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
        let cache = Arc::new(FeatureCache::default());
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let state = AppState {
            cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "client".into(),
            client_secret: "secret".into(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            pending_assignments: Arc::new(RwLock::new(Vec::new())),
            flush_interval: Duration::from_secs(60),
            pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
            evaluation_flush_interval: Duration::from_secs(60),
            retry_config: config::RetryConfig::default(),
        };

        let feature_id = "fea-123";
        {
            let mut cache = state.assigned_cache.write().await;
            cache.insert(
                format!("user-1|{}|env-1", feature_id),
                CachedAssignment {
                    value: serde_json::json!(true),
                    variant: None,
                },
            );
            cache.insert(
                format!("user-2|{}|env-1", feature_id),
                CachedAssignment {
                    value: serde_json::json!(true),
                    variant: None,
                },
            );
            cache.insert(
                "user-3|other|env".to_string(),
                CachedAssignment {
                    value: serde_json::json!(true),
                    variant: None,
                },
            );
        }

        {
            let mut pending = state.pending_assignments.write().await;
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

        let cache = state.assigned_cache.read().await;
        assert_eq!(cache.len(), 1);
        assert!(cache.keys().all(|entry| !entry.contains(feature_id)));
        drop(cache);

        let pending = state.pending_assignments.read().await;
        assert_eq!(pending.len(), 1);
        assert!(
            pending
                .iter()
                .all(|assignment| assignment.feature_id != feature_id)
        );
    }
}
