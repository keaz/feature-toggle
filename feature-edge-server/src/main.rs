use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use evaluation_engine as engine;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_stream::StreamExt;
use tonic::transport::Endpoint;
use tracing::{error, info};

mod pb {
    #![allow(clippy::all)]
    #![allow(warnings)]
    tonic::include_proto!("featuretoggle");
}

#[derive(Clone)]
pub struct AppState {
    cache: Arc<FeatureCache>,
    grpc: Arc<tokio::sync::Mutex<pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>>>, 
    client_id: String,
    client_secret: String,
}

#[derive(Default)]
pub struct FeatureCache {
    by_key: RwLock<HashMap<String, pb::FeatureFull>>, // key -> feature
    by_id: RwLock<HashMap<String, String>>,            // id -> key
}

impl FeatureCache {
    async fn upsert(&self, f: pb::FeatureFull) {
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
    async fn delete_by_key(&self, key: &str) {
        let mut by_key = self.by_key.write().await;
        if let Some(f) = by_key.remove(key) {
            let mut by_id = self.by_id.write().await;
            by_id.remove(&f.id);
        }
    }
    async fn get_by_key(&self, key: &str) -> Option<pb::FeatureFull> {
        let by_key = self.by_key.read().await;
        by_key.get(key).cloned()
    }
    async fn snapshot(&self, features: Vec<pb::FeatureFull>) {
        let mut by_key = self.by_key.write().await;
        let mut by_id = self.by_id.write().await;
        by_key.clear();
        by_id.clear();
        for f in features.into_iter() {
            by_id.insert(f.id.clone(), f.key.clone());
            by_key.insert(f.key.clone(), f);
        }
    }
}

#[derive(Deserialize)]
struct EvaluateHttpRequest {
    feature_key: String,
    environment_id: String,
    context: Vec<HttpContext>,
    client_id: Option<String>,
    client_secret: Option<String>,
}

#[derive(Deserialize)]
struct HttpContext { key: String, value: String }

#[derive(Serialize)]
struct EvaluateHttpResponse { enabled: bool }

fn map_proto_to_engine(f: &pb::FeatureFull) -> engine::Feature {
    let stages = f
        .stages
        .iter()
        .map(|s| engine::FeatureStage {
            environment_id: s.environment_id.clone(),
            enabled: s.enabled,
            bucketing_key: if s.bucketing_key.is_empty() { None } else { Some(s.bucketing_key.clone()) },
            criterias: s
                .criterias
                .iter()
                .map(|c| engine::StageCriterion {
                    context_key: c.context_key.clone(),
                    context: engine::StageContext {
                        key: c.context.as_ref().map(|x| x.key.clone()).unwrap_or_default(),
                        entries: c.context.as_ref().map(|x| x.entries.clone()).unwrap_or_default(),
                    },
                    rollout_percentage: c.rollout_percentage,
                })
                .collect(),
        })
        .collect();

    engine::Feature {
        enabled: true,             // Top-level flag not present in proto; default to true
        dependencies: vec![],      // For minimal implementation, ignore dependency recursion
        stages,
    }
}

fn map_http_context_to_engine(feature_key: String, environment_id: String, ctx: Vec<HttpContext>) -> engine::FeatureEvaluationContext {
    engine::FeatureEvaluationContext {
        feature: feature_key,
        environment_id,
        context: ctx
            .into_iter()
            .map(|c| engine::Context { key: c.key, value: c.value })
            .collect(),
    }
}

pub async fn evaluate_handler(app: web::Data<AppState>, req: web::Json<EvaluateHttpRequest>) -> actix_web::Result<web::Json<EvaluateHttpResponse>> {
    let req = req.into_inner();
    let feature_key = req.feature_key.clone();

    // Resolve credentials (prefer request overrides)
    let client_id = req.client_id.clone().unwrap_or_else(|| app.client_id.clone());
    let client_secret = req.client_secret.clone().unwrap_or_else(|| app.client_secret.clone());

    // Try cache
    let feature_opt = app.cache.get_by_key(&feature_key).await;
    let feature = if let Some(f) = feature_opt { f } else {
        // Fetch via gRPC GetFeatureByKey
        let mut client = app.grpc.lock().await;
        let request = pb::GetFeatureByKeyRequest { feature_key: feature_key.clone(), client_id: client_id.clone(), client_secret: client_secret.clone() };
        match client.get_feature_by_key(tonic::Request::new(request)).await {
            Ok(resp) => {
                let feature = resp.into_inner().feature.expect("server returned no feature");
                app.cache.upsert(feature.clone()).await;
                feature
            }
            Err(e) => {
                error!("gRPC GetFeatureByKey error: {}", e);
                return Err(actix_web::error::ErrorBadGateway("backend unavailable"));
            }
        }
    };

    let engine_feature = map_proto_to_engine(&feature);
    let ec = map_http_context_to_engine(feature_key, req.environment_id, req.context);
    let enabled = engine::evaluate(ec, engine_feature);
    Ok(web::Json(EvaluateHttpResponse { enabled }))
}

pub async fn health_handler() -> impl Responder { HttpResponse::Ok().body("OK") }

async fn run_stream_task(app: AppState, grpc_addr: String) {
    use tokio_stream::wrappers::ReceiverStream;
    loop {
        // reconnect loop
        let endpoint = Endpoint::from_shared(grpc_addr.clone())
            .expect("invalid gRPC address")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Duration::from_secs(20))
            .keep_alive_while_idle(true)
            .concurrency_limit(256)
            .tcp_nodelay(true);
        match endpoint.connect().await {
            Ok(channel) => {
                let mut client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
                info!("Connected to backend gRPC {}",&grpc_addr);

                let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);

                // Send initial Subscribe BEFORE opening the streaming call so the server receives it immediately
                let subscribe = pb::SubscribeRequest {
                    client_id: app.client_id.clone(),
                    client_secret: app.client_secret.clone(),
                    feature_keys: vec![],
                    environment_id: "".into(),
                };
                let initial = pb::StreamRequest { payload: Some(pb::stream_request::Payload::Subscribe(subscribe)) };
                let _ = tx.send(initial).await;

                // Heartbeats
                let hb_tx = tx.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                        let _ = hb_tx.send(pb::StreamRequest{ payload: Some(pb::stream_request::Payload::Heartbeat(pb::Heartbeat{ ts_unix_ms: ts }))}).await;
                    }
                });

                let req_stream = ReceiverStream::new(rx);
                let response = match client.stream_updates(req_stream).await {
                    Ok(r) => r,
                    Err(e) => { error!("failed to open stream: {}", e); tokio::time::sleep(Duration::from_secs(3)).await; continue; }
                };
                let mut inbound = response.into_inner();

                // Process updates
                while let Some(msg) = inbound.next().await {
                    match msg {
                        Ok(update) => {
                            use pb::feature_update::Action;
                            match update.action {
                                x if x == Action::Upsert as i32 || x == Action::Snapshot as i32 => {
                                    if let Some(f) = update.feature { app.cache.upsert(f).await; }
                                }
                                x if x == Action::Delete as i32 => {
                                    if !update.feature_key.is_empty() { app.cache.delete_by_key(&update.feature_key).await; }
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            error!("stream recv error: {}", e);
                            break; // reconnect
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect gRPC: {}", e);
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // init tracing
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();

    // Config
    let grpc_addr = std::env::var("EDGE_BACKEND_GRPC").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let http_addr: SocketAddr = std::env::var("EDGE_HTTP_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".into()).parse().expect("invalid EDGE_HTTP_ADDR");
    let client_id = std::env::var("EDGE_CLIENT_ID").unwrap_or_else(|_| "a1b2c3d4-0000-4000-8000-000000000001".into());
    let client_secret = std::env::var("EDGE_CLIENT_SECRET").unwrap_or_else(|_| "TEST_WEB_KEY_1".into());

    // Prepare gRPC client for direct calls (configured endpoint)
    let endpoint = Endpoint::from_shared(grpc_addr.clone())
        .expect("invalid gRPC address")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .http2_keep_alive_interval(Duration::from_secs(20))
        .keep_alive_while_idle(true)
        .concurrency_limit(256)
        .tcp_nodelay(true);
    let channel = endpoint.connect().await?;
    let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

    let state = AppState {
        cache: Arc::new(FeatureCache::default()),
        grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
    };

    // Start stream sync task
    let stream_state = state.clone();
    let grpc_addr_clone = grpc_addr.clone();
    tokio::spawn(async move { run_stream_task(stream_state, grpc_addr_clone).await });

    info!("feature-edge-server listening on {} (HTTP), streaming from {}", http_addr, grpc_addr);

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .route("/health", web::get().to(health_handler))
            .route("/evaluate", web::post().to(evaluate_handler))
    })
    .bind(http_addr)?
    .run()
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test;

    fn make_feature(key: &str, env: &str, enabled: bool, context_key: &str, allowed: &[&str], rollout: i32) -> pb::FeatureFull {
        pb::FeatureFull {
            id: format!("{}_id", key),
            key: key.to_string(),
            description: String::new(),
            feature_type: String::new(),
            team_id: String::new(),
            created_at: String::new(),
            stages: vec![pb::FeatureStageFull {
                id: "stage1".into(),
                environment_id: env.into(),
                order_index: 0,
                position: "start".into(),
                enabled,
                bucketing_key: String::new(),
                criterias: vec![pb::StageCriterionFull {
                    id: "crit1".into(),
                    context_key: context_key.into(),
                    context: Some(pb::CriterionContext { key: context_key.into(), entries: allowed.iter().map(|s| s.to_string()).collect() }),
                    rollout_percentage: rollout,
                }],
            }],
            dependencies: vec![],
        }
    }

    async fn test_state_with_cache(feature: pb::FeatureFull) -> AppState {
        let cache = Arc::new(FeatureCache::default());
        let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        let grpc = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let state = AppState { cache: cache.clone(), grpc: Arc::new(tokio::sync::Mutex::new(grpc)), client_id: "c".into(), client_secret: "s".into() };
        // seed cache
        cache.upsert(feature).await;
        state
    }

    fn _make_lazy_channel_client() -> pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel> {
            let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
            pb::feature_evaluation_client::FeatureEvaluationClient::new(channel)
        }

        fn test_state_empty_cache() -> AppState {
        let cache = Arc::new(FeatureCache::default());
        let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        let grpc = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        AppState { cache, grpc: Arc::new(tokio::sync::Mutex::new(grpc)), client_id: "c".into(), client_secret: "s".into() }
    }

    #[actix_web::test]
    async fn test_map_http_context() {
        let ctx = vec![HttpContext{ key: "user.id".into(), value: "u1".into() }, HttpContext{ key: "country".into(), value: "US".into() }];
        let ec = map_http_context_to_engine("feat".into(), "env1".into(), ctx);
        assert_eq!(ec.feature, "feat");
        assert_eq!(ec.environment_id, "env1");
        assert_eq!(ec.context.len(), 2);
    }

    #[actix_web::test]
    async fn test_map_proto_to_engine() {
        let f = make_feature("f1", "env1", true, "country", &["US"], 100);
        let eng = map_proto_to_engine(&f);
        assert!(eng.enabled);
        assert_eq!(eng.stages.len(), 1);
        assert_eq!(eng.stages[0].environment_id, "env1");
        assert!(eng.stages[0].criterias.len() > 0);
    }

    #[actix_web::test]
    async fn test_feature_cache_ops() {
        let cache = FeatureCache::default();
        let f1 = make_feature("k1", "env", true, "country", &["US"], 100);
        cache.upsert(f1.clone()).await;
        assert!(cache.get_by_key("k1").await.is_some());
        cache.delete_by_key("k1").await;
        assert!(cache.get_by_key("k1").await.is_none());
        // snapshot
        let f2 = make_feature("k2", "env", true, "country", &["US"], 50);
        let f3 = make_feature("k3", "env", true, "country", &["US"], 0);
        cache.snapshot(vec![f2.clone(), f3.clone()]).await;
        assert!(cache.get_by_key("k2").await.is_some());
        assert!(cache.get_by_key("k3").await.is_some());
    }

    #[actix_web::test]
    async fn test_evaluate_handler_cache_hit_true() {
        let feature = make_feature("featA", "env1", true, "country", &["US"], 100);
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state);
        let req = EvaluateHttpRequest{
            feature_key: "featA".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext{ key: "user.id".into(), value: "u1".into() }, HttpContext{ key: "country".into(), value: "US".into() }],
            client_id: None,
            client_secret: None,
        };
        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(resp.into_inner().enabled);
    }

    #[actix_web::test]
    async fn test_evaluate_handler_overrides_credentials_cache_hit() {
        let feature = make_feature("featC", "env1", true, "country", &["US"], 100);
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state);
        let req = EvaluateHttpRequest{
            feature_key: "featC".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext{ key: "user.id".into(), value: "u2".into() }, HttpContext{ key: "country".into(), value: "US".into() }],
            client_id: Some("override_id".into()),
            client_secret: Some("override_secret".into()),
        };
        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(resp.into_inner().enabled);
    }

    #[actix_web::test]
    async fn test_evaluate_handler_cache_hit_false_wrong_env() {
        let feature = make_feature("featB", "env2", true, "country", &["US"], 100);
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state);
        let req = EvaluateHttpRequest{
            feature_key: "featB".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext{ key: "user.id".into(), value: "u1".into() }, HttpContext{ key: "country".into(), value: "US".into() }],
            client_id: None,
            client_secret: None,
        };
        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(!resp.into_inner().enabled);
    }

    #[actix_web::test]
    async fn test_evaluate_handler_cache_miss_backend_error() {
        let state = test_state_empty_cache();
        let app_data = web::Data::new(state);
        let req = EvaluateHttpRequest{
            feature_key: "unknown".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext{ key: "user.id".into(), value: "u1".into() }],
            client_id: Some("cid".into()),
            client_secret: Some("sec".into()),
        };
        let err = evaluate_handler(app_data, web::Json(req)).await.err().expect("expected error");
        use actix_web::ResponseError;
        assert_eq!(err.as_response_error().status_code().as_u16(), 502);
    }

    #[actix_web::test]
    async fn test_health_endpoint() {
        let app = test::init_service(App::new().route("/health", web::get().to(health_handler))).await;
        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }
}
