use actix_web::{App, HttpResponse, HttpServer, Responder, web};
use evaluation_engine as engine;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tokio_stream::StreamExt;
use tonic::transport::Endpoint;
use tracing::{error, info};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

mod pb {
    #![allow(clippy::all)]
    #![allow(warnings)]
    tonic::include_proto!("featuretoggle");
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
    // Sticky assignments cache and pending flush queue
    assigned_true: Arc<RwLock<std::collections::HashSet<String>>>,
    pending_assignments: Arc<RwLock<Vec<UserAssignment>>>,
    flush_interval: Duration,
    // Evaluation events tracking
    pending_evaluation_events: Arc<RwLock<Vec<EvaluationEvent>>>,
    evaluation_flush_interval: Duration,
}

#[derive(Clone, Debug)]
struct UserAssignment {
    user_id: String,
    feature_id: String,
    environment_id: String,
    assigned: bool,
}

#[derive(Clone, Debug)]
struct EvaluationEvent {
    feature_key: String,
    environment_id: String,
    evaluation_result: bool,
    evaluation_context: Vec<HttpContext>,
    user_context: Option<String>,
    evaluated_at: std::time::SystemTime,
    prior_assignment: bool,
}

fn assignment_key(user_id: &str, feature_id: &str, environment_id: &str) -> String {
    format!("{}|{}|{}", user_id, feature_id, environment_id)
}

#[derive(Default)]
pub struct FeatureCache {
    by_key: RwLock<HashMap<String, pb::FeatureFull>>, // key -> feature
    by_id: RwLock<HashMap<String, String>>,           // id -> key
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

#[derive(Deserialize, ToSchema, Clone)]
struct EvaluateHttpRequest {
    /// The feature key to evaluate
    feature_key: String,
    /// Environment identifier (e.g., "prod", "staging")
    environment_id: String,
    /// Context entries used for evaluation (key/value)
    context: Vec<HttpContext>,
    /// Optional client credentials overriding server defaults
    client_id: Option<String>,
    /// Optional client credentials overriding server defaults
    client_secret: Option<String>,
}

#[derive(Deserialize, ToSchema, Clone, Debug, PartialEq)]
struct HttpContext {
    /// Context key, e.g., "user.id" or a bucketing key
    key: String,
    /// Context value as string
    value: String,
}

#[derive(Serialize, ToSchema)]
struct EvaluateHttpResponse {
    /// Whether the feature is enabled under provided context
    enabled: bool,
}

fn map_proto_to_engine(f: &pb::FeatureFull) -> engine::Feature {
    let stages = f
        .stages
        .iter()
        .map(|s| engine::FeatureStage {
            environment_id: s.environment_id.clone(),
            enabled: s.enabled,
            bucketing_key: if s.bucketing_key.is_empty() {
                None
            } else {
                Some(s.bucketing_key.clone())
            },
            criterias: s
                .criterias
                .iter()
                .map(|c| engine::StageCriterion {
                    context_key: c.context_key.clone(),
                    context: engine::StageContext {
                        key: c
                            .context
                            .as_ref()
                            .map(|x| x.key.clone())
                            .unwrap_or_default(),
                        entries: c
                            .context
                            .as_ref()
                            .map(|x| x.entries.clone())
                            .unwrap_or_default(),
                    },
                    rollout_percentage: c.rollout_percentage,
                })
                .collect(),
        })
        .collect();

    engine::Feature {
        enabled: true,        // Top-level flag not present in proto; default to true
        dependencies: vec![], // For minimal implementation, ignore dependency recursion
        stages,
    }
}

fn map_http_context_to_engine(
    feature_key: String,
    environment_id: String,
    ctx: Vec<HttpContext>,
) -> engine::FeatureEvaluationContext {
    engine::FeatureEvaluationContext {
        feature: feature_key,
        environment_id,
        context: ctx
            .into_iter()
            .map(|c| engine::Context {
                key: c.key,
                value: c.value,
            })
            .collect(),
    }
}

fn resolve_credentials(app: &AppState, req: &EvaluateHttpRequest) -> (String, String) {
    let client_id = req
        .client_id
        .clone()
        .unwrap_or_else(|| app.client_id.clone());
    let client_secret = req
        .client_secret
        .clone()
        .unwrap_or_else(|| app.client_secret.clone());
    (client_id, client_secret)
}

async fn fetch_feature_via_grpc(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<pb::FeatureFull> {
    let mut client = app.grpc.lock().await;
    let request = pb::GetFeatureByKeyRequest {
        feature_key: feature_key.to_string(),
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
    };
    match client
        .get_feature_by_key(tonic::Request::new(request))
        .await
    {
        Ok(resp) => resp.into_inner().feature,
        Err(e) => {
            error!("gRPC GetFeatureByKey error: {}", e);
            None
        }
    }
}

async fn get_or_fetch_feature(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<pb::FeatureFull> {
    if let Some(f) = app.cache.get_by_key(feature_key).await {
        return Some(f);
    }
    let feature = fetch_feature_via_grpc(app, feature_key, client_id, client_secret).await?;
    app.cache.upsert(feature.clone()).await;
    Some(feature)
}

#[utoipa::path(
    post,
    path = "/evaluate",
    request_body = EvaluateHttpRequest,
    responses(
        (status = 200, description = "Evaluation result", body = EvaluateHttpResponse),
        (status = 502, description = "Backend unavailable"),
        (status = 400, description = "Invalid request")
    ),
    tag = "edge"
)]
async fn evaluate_handler(
    app: web::Data<AppState>,
    req: web::Json<EvaluateHttpRequest>,
) -> actix_web::Result<web::Json<EvaluateHttpResponse>> {
    let req = req.into_inner();
    let feature_key = req.feature_key.clone();

    let (client_id, client_secret) = resolve_credentials(&app, &req);
    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(&app, &feature_key, &client_id, &client_secret).await {
        Some(f) => f,
        None => {
            // Feature doesn't exist, return false
            return Ok(web::Json(EvaluateHttpResponse { enabled: false }));
        }
    };

    let stage = feature
        .stages
        .iter()
        .find(|s| s.environment_id == req.environment_id);

    if stage.is_none() || !stage.unwrap().enabled {
        return Ok(web::Json(EvaluateHttpResponse { enabled: false }));
    }

    let stage = stage.unwrap();
    let bucketing_key = stage.bucketing_key.clone();

    // Extract user.id if present
    let user_id_opt = req
        .context
        .iter()
        .find(|c| c.key == bucketing_key)
        .map(|c| c.value.clone());

    // If we have a prior assignment for this user+feature+env, short-circuit to true
    let (enabled, prior_assignment) = if let Some(user_id) = &user_id_opt {
        let key = assignment_key(user_id, &feature.id, &req.environment_id);
        if app.assigned_true.read().await.contains(&key) {
            (true, true) // cached assignment
        } else {
            let engine_feature = map_proto_to_engine(&feature);
            let ec = map_http_context_to_engine(
                feature_key,
                req.environment_id.clone(),
                req.context.clone(),
            );
            let enabled = engine::evaluate(ec, engine_feature);
            (enabled, false) // fresh evaluation
        }
    } else {
        let engine_feature = map_proto_to_engine(&feature);
        let ec = map_http_context_to_engine(
            feature_key,
            req.environment_id.clone(),
            req.context.clone(),
        );
        let enabled = engine::evaluate(ec, engine_feature);
        (enabled, false) // fresh evaluation
    };

    // Record the evaluation event for analytics
    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: req.environment_id.clone(),
        evaluation_result: enabled,
        evaluation_context: req.context.clone(),
        user_context: user_id_opt.clone(),
        evaluated_at: std::time::SystemTime::now(),
        prior_assignment,
    };

    {
        let mut pending_events = app.pending_evaluation_events.write().await;
        pending_events.push(evaluation_event);
    }

    // If evaluated true, remember and enqueue for flush
    if enabled && let Some(user_id) = user_id_opt {
        let key = assignment_key(&user_id, &feature.id, &req.environment_id);
        {
            let mut set = app.assigned_true.write().await;
            set.insert(key);
        }
        let mut pending = app.pending_assignments.write().await;
        pending.push(UserAssignment {
            user_id,
            feature_id: feature.id.clone(),
            environment_id: req.environment_id,
            assigned: true,
        });
    }

    Ok(web::Json(EvaluateHttpResponse { enabled }))
}

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Service is healthy"), (status = 503, description = "Service is not connected to backend")),
    tag = "edge"
)]
pub async fn health_handler(app: web::Data<AppState>) -> impl Responder {
    use std::sync::atomic::Ordering;
    if app.connected.load(Ordering::Relaxed) {
        HttpResponse::Ok().body("OK")
    } else {
        HttpResponse::ServiceUnavailable().body("UNAVAILABLE")
    }
}

fn build_endpoint(grpc_addr: &str) -> Endpoint {
    Endpoint::from_shared(grpc_addr.to_string())
        .expect("invalid gRPC address")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .http2_keep_alive_interval(Duration::from_secs(20))
        .keep_alive_while_idle(true)
        .concurrency_limit(256)
        .tcp_nodelay(true)
}

async fn send_initial_subscribe(tx: &tokio::sync::mpsc::Sender<pb::StreamRequest>, app: &AppState) {
    let subscribe = pb::SubscribeRequest {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
        feature_keys: vec![],
        environment_id: "".into(),
    };
    let initial = pb::StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(subscribe)),
    };
    let _ = tx.send(initial).await;
}

fn spawn_heartbeat(tx: tokio::sync::mpsc::Sender<pb::StreamRequest>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let _ = tx
                .send(pb::StreamRequest {
                    payload: Some(pb::stream_request::Payload::Heartbeat(pb::Heartbeat {
                        ts_unix_ms: ts,
                    })),
                })
                .await;
        }
    });
}

async fn open_streaming_call(
    mut client: pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
    rx: tokio::sync::mpsc::Receiver<pb::StreamRequest>,
) -> Result<tonic::Response<tonic::Streaming<pb::FeatureUpdate>>, tonic::Status> {
    use tokio_stream::wrappers::ReceiverStream;
    let req_stream = ReceiverStream::new(rx);
    client.stream_updates(req_stream).await
}

async fn handle_feature_update(app: &AppState, update: pb::FeatureUpdate) {
    use pb::feature_update::Action;
    match update.action {
        x if x == Action::Upsert as i32 || x == Action::Snapshot as i32 => {
            if let Some(f) = update.feature {
                app.cache.upsert(f).await;
            }
        }
        x if x == Action::Delete as i32 => {
            if !update.feature_key.is_empty() {
                app.cache.delete_by_key(&update.feature_key).await;
            }
        }
        _ => {}
    }
}

async fn load_user_assignments(app: &AppState) -> Result<usize, tonic::Status> {
    let req = pb::ListUserFlagAssignmentsRequest {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
        environment_id: String::new(),
        feature_id: String::new(),
    };
    let mut client = app.grpc.lock().await.clone();
    let resp = client.list_user_assignments(req).await?.into_inner();
    let mut count = 0usize;
    {
        let mut set = app.assigned_true.write().await;
        for a in resp.assignments.into_iter() {
            if a.assigned {
                let key = assignment_key(&a.user_id, &a.feature_id, &a.environment_id);
                set.insert(key);
                count += 1;
            }
        }
    }
    Ok(count)
}

async fn run_stream_task(app: AppState, grpc_addr: String) {
    use std::sync::atomic::Ordering;
    loop {
        // reset status while attempting to connect
        app.connected.store(false, Ordering::Relaxed);
        // reconnect loop
        let endpoint = build_endpoint(&grpc_addr);
        match endpoint.connect().await {
            Ok(channel) => {
                let client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
                info!("Connected to backend gRPC {}", &grpc_addr);

                let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);

                // Send initial Subscribe BEFORE opening the streaming call
                send_initial_subscribe(&tx, &app).await;

                // Heartbeats
                spawn_heartbeat(tx.clone());

                let response = match open_streaming_call(client, rx).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("failed to open stream: {}", e);
                        app.connected.store(false, Ordering::Relaxed);
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        continue;
                    }
                };
                // streaming successfully opened -> mark healthy
                app.connected.store(true, Ordering::Relaxed);
                let mut inbound = response.into_inner();

                // Process updates
                while let Some(msg) = inbound.next().await {
                    match msg {
                        Ok(update) => {
                            handle_feature_update(&app, update).await;
                        }
                        Err(e) => {
                            error!("stream recv error: {}", e);
                            break; // reconnect
                        }
                    }
                }
                // stream closed -> mark unhealthy
                app.connected.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                error!("Failed to connect gRPC: {}", e);
                app.connected.store(false, Ordering::Relaxed);
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_flush_task(app: AppState) {
    loop {
        tokio::time::sleep(app.flush_interval).await;
        // Drain pending assignments
        let to_send: Vec<UserAssignment> = {
            let mut lock = app.pending_assignments.write().await;
            if lock.is_empty() {
                Vec::new()
            } else {
                let v = lock.drain(..).collect::<Vec<_>>();
                v
            }
        };
        if to_send.is_empty() {
            continue;
        }

        // Build a request stream
        let (tx, rx) = tokio::sync::mpsc::channel::<pb::UserFlagAssignment>(to_send.len().max(1));
        let creds_first = pb::UserFlagAssignment {
            user_id: to_send[0].user_id.clone(),
            feature_id: to_send[0].feature_id.clone(),
            environment_id: to_send[0].environment_id.clone(),
            assigned: to_send[0].assigned,
            client_id: app.client_id.clone(),
            client_secret: app.client_secret.clone(),
        };
        // Spawn sender
        tokio::spawn({
            let _app_clone = app.clone();
            let rest = to_send[1..].to_vec();
            let tx_clone = tx.clone();
            async move {
                let _ = tx_clone.send(creds_first).await;
                for a in rest {
                    let _ = tx_clone
                        .send(pb::UserFlagAssignment {
                            user_id: a.user_id,
                            feature_id: a.feature_id,
                            environment_id: a.environment_id,
                            assigned: a.assigned,
                            client_id: String::new(),
                            client_secret: String::new(),
                        })
                        .await;
                }
            }
        });

        // Use a cloned client to avoid holding the lock
        let mut client = {
            let guard = app.grpc.lock().await;
            guard.clone()
        };
        use tokio_stream::wrappers::ReceiverStream;
        let stream = ReceiverStream::new(rx);
        match client.push_user_assignments(stream).await {
            Ok(_) => {
                // success, nothing to do
            }
            Err(e) => {
                error!("failed to push user assignments: {}", e);
                // requeue
                let mut lock = app.pending_assignments.write().await;
                lock.extend(to_send);
            }
        }
    }
}

async fn run_evaluation_flush_task(app: AppState) {
    loop {
        tokio::time::sleep(app.evaluation_flush_interval).await;

        // Drain pending evaluation events
        let to_send: Vec<EvaluationEvent> = {
            let mut lock = app.pending_evaluation_events.write().await;
            if lock.is_empty() {
                Vec::new()
            } else {
                let v = lock.drain(..).collect::<Vec<_>>();
                v
            }
        };

        if to_send.is_empty() {
            continue;
        }

        // Convert to proto format
        let mut proto_events = Vec::new();
        for event in to_send.iter() {
            let evaluated_at_unix_ms = event
                .evaluated_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let proto_context = event
                .evaluation_context
                .iter()
                .map(|ctx| pb::Context {
                    key: ctx.key.clone(),
                    value: ctx.value.clone(),
                })
                .collect();

            proto_events.push(pb::FeatureEvaluationEvent {
                feature_key: event.feature_key.clone(),
                environment_id: event.environment_id.clone(),
                client_id: app.client_id.clone(),
                client_secret: app.client_secret.clone(),
                evaluation_result: event.evaluation_result,
                evaluation_context: proto_context,
                user_context: event.user_context.clone().unwrap_or_default(),
                evaluated_at_unix_ms,
                prior_assignment: event.prior_assignment,
            });
        }

        let request = pb::PushEvaluationEventsRequest {
            events: proto_events,
        };

        // Use a cloned client to avoid holding the lock
        let mut client = {
            let guard = app.grpc.lock().await;
            guard.clone()
        };

        match client.push_evaluation_events(request).await {
            Ok(response) => {
                let resp = response.into_inner();
                info!(
                    "Successfully pushed {} evaluation events",
                    resp.processed_count
                );
            }
            Err(e) => {
                error!("Failed to push evaluation events: {}", e);
                // Requeue the events on failure
                let mut lock = app.pending_evaluation_events.write().await;
                lock.extend(to_send);
            }
        }
    }
}

fn setup_logger() -> actix_web::Result<(), Box<dyn std::error::Error>> {
    log4rs::init_file("log4rs.yaml", Default::default())?;
    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    paths(evaluate_handler, health_handler),
    components(schemas(EvaluateHttpRequest, EvaluateHttpResponse, HttpContext)),
    tags((name = "edge", description = "Edge evaluation API"))
)]
struct ApiDoc;

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_logger()?;
    // Config
    let grpc_addr =
        std::env::var("EDGE_BACKEND_GRPC").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    let http_addr: SocketAddr = std::env::var("EDGE_HTTP_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8081".into())
        .parse()
        .expect("invalid EDGE_HTTP_ADDR");
    let client_id = std::env::var("EDGE_CLIENT_ID")
        .unwrap_or_else(|_| "a1b2c3d4-0000-4000-8000-000000000001".into());
    let client_secret =
        std::env::var("EDGE_CLIENT_SECRET").unwrap_or_else(|_| "TEST_WEB_KEY_1".into());

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

    let flush_secs: u64 = std::env::var("EDGE_ASSIGNMENT_FLUSH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let evaluation_flush_secs: u64 = std::env::var("EDGE_EVALUATION_FLUSH_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30); // Default to 30 seconds for evaluation events

    let state = AppState {
        cache: Arc::new(FeatureCache::default()),
        grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
        connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        assigned_true: Arc::new(RwLock::new(std::collections::HashSet::new())),
        pending_assignments: Arc::new(RwLock::new(Vec::new())),
        flush_interval: Duration::from_secs(flush_secs),
        pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
        evaluation_flush_interval: Duration::from_secs(evaluation_flush_secs),
    };

    // On startup, fetch persisted user assignments from backend and warm the cache
    match load_user_assignments(&state).await {
        Ok(n) => info!("loaded {} user assignments from backend", n),
        Err(e) => error!("failed to load user assignments: {}", e),
    }

    // Start stream sync task
    let stream_state = state.clone();
    let grpc_addr_clone = grpc_addr.clone();
    tokio::spawn(async move { run_stream_task(stream_state, grpc_addr_clone).await });

    // Start periodic flush task
    let flush_state = state.clone();
    tokio::spawn(async move { run_flush_task(flush_state).await });

    // Start periodic evaluation events flush task
    let evaluation_flush_state = state.clone();
    tokio::spawn(async move { run_evaluation_flush_task(evaluation_flush_state).await });

    info!(
        "feature-edge-server listening on {} (HTTP), streaming from {}",
        http_addr, grpc_addr
    );

    let openapi = ApiDoc::openapi();

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .service(SwaggerUi::new("/docs/{_:.*}").url("/api-doc/openapi.json", openapi.clone()))
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

    fn make_feature(
        key: &str,
        env: &str,
        enabled: bool,
        context_key: &str,
        allowed: &[&str],
        rollout: i32,
    ) -> pb::FeatureFull {
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
                    context: Some(pb::CriterionContext {
                        key: context_key.into(),
                        entries: allowed.iter().map(|s| s.to_string()).collect(),
                    }),
                    rollout_percentage: rollout,
                }],
            }],
            dependencies: vec![],
        }
    }

    async fn test_state_with_cache(feature: pb::FeatureFull) -> AppState {
        let cache = Arc::new(FeatureCache::default());
        let channel = Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        let grpc = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let state = AppState {
            cache: cache.clone(),
            grpc: Arc::new(tokio::sync::Mutex::new(grpc)),
            client_id: "c".into(),
            client_secret: "s".into(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_true: Arc::new(RwLock::new(std::collections::HashSet::new())),
            pending_assignments: Arc::new(RwLock::new(Vec::new())),
            flush_interval: Duration::from_secs(10),
            pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
            evaluation_flush_interval: Duration::from_secs(30),
        };
        // seed cache
        cache.upsert(feature).await;
        state
    }

    fn _make_lazy_channel_client()
    -> pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel> {
        let channel = Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        pb::feature_evaluation_client::FeatureEvaluationClient::new(channel)
    }

    fn test_state_empty_cache() -> AppState {
        let cache = Arc::new(FeatureCache::default());
        let channel = Endpoint::from_static("http://127.0.0.1:9").connect_lazy();
        let grpc = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        AppState {
            cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc)),
            client_id: "c".into(),
            client_secret: "s".into(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_true: Arc::new(RwLock::new(std::collections::HashSet::new())),
            pending_assignments: Arc::new(RwLock::new(Vec::new())),
            flush_interval: Duration::from_secs(10),
            pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
            evaluation_flush_interval: Duration::from_secs(30),
        }
    }

    #[actix_web::test]
    async fn test_map_http_context() {
        let ctx = vec![
            HttpContext {
                key: "user.id".into(),
                value: "u1".into(),
            },
            HttpContext {
                key: "country".into(),
                value: "US".into(),
            },
        ];
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
        assert!(!eng.stages[0].criterias.is_empty());
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
        let req = EvaluateHttpRequest {
            feature_key: "featA".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u1".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
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
        let req = EvaluateHttpRequest {
            feature_key: "featC".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u2".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
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
        let req = EvaluateHttpRequest {
            feature_key: "featB".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u1".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };
        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(!resp.into_inner().enabled);
    }

    #[actix_web::test]
    async fn test_evaluate_handler_cache_miss_feature_not_found() {
        let state = test_state_empty_cache();
        let app_data = web::Data::new(state);
        let req = EvaluateHttpRequest {
            feature_key: "unknown".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext {
                key: "user.id".into(),
                value: "u1".into(),
            }],
            client_id: Some("cid".into()),
            client_secret: Some("sec".into()),
        };
        let resp = evaluate_handler(app_data, web::Json(req))
            .await
            .expect("should not return error");
        assert!(!resp.into_inner().enabled); // Should return false when feature not found
    }

    #[actix_web::test]
    async fn test_fetch_feature_via_grpc_returns_none_on_error() {
        // Test that fetch_feature_via_grpc returns None when gRPC call fails
        let state = test_state_empty_cache();
        let result = fetch_feature_via_grpc(&state, "nonexistent", "client", "secret").await;
        assert!(result.is_none()); // Should return None when feature doesn't exist or gRPC fails
    }

    #[actix_web::test]
    async fn test_health_endpoint() {
        let state = test_state_empty_cache();
        // mark as connected to simulate healthy state
        state
            .connected
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(state.clone()))
                .route("/health", web::get().to(health_handler)),
        )
        .await;
        let req = test::TestRequest::get().uri("/health").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        // now mark disconnected and expect 503
        state
            .connected
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let req2 = test::TestRequest::get().uri("/health").to_request();
        let resp2 = test::call_service(&app, req2).await;
        assert_eq!(resp2.status().as_u16(), 503);
    }

    // Helper function to create a test state with sticky assignments pre-populated
    async fn test_state_with_assignments(
        feature: pb::FeatureFull,
        assignments: Vec<(String, String, String)>,
    ) -> AppState {
        let state = test_state_with_cache(feature).await;
        {
            let mut assigned_true = state.assigned_true.write().await;
            for (user_id, feature_id, env_id) in assignments {
                let key = assignment_key(&user_id, &feature_id, &env_id);
                assigned_true.insert(key);
            }
        }
        state
    }

    // Helper function to make a feature with specific bucketing key
    fn make_feature_with_bucketing(
        key: &str,
        env: &str,
        enabled: bool,
        bucketing_key: &str,
        context_key: &str,
        allowed: &[&str],
        rollout: i32,
    ) -> pb::FeatureFull {
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
                bucketing_key: bucketing_key.into(),
                criterias: vec![pb::StageCriterionFull {
                    id: "crit1".into(),
                    context_key: context_key.into(),
                    context: Some(pb::CriterionContext {
                        key: context_key.into(),
                        entries: allowed.iter().map(|s| s.to_string()).collect(),
                    }),
                    rollout_percentage: rollout,
                }],
            }],
            dependencies: vec![],
        }
    }

    #[actix_web::test]
    async fn test_assignment_key_generation() {
        let key = assignment_key("user123", "feature456", "env789");
        assert_eq!(key, "user123|feature456|env789");
    }

    #[actix_web::test]
    async fn test_evaluate_handler_prior_assignment_true() {
        // Test that cached assignments return prior_assignment=true and skip evaluation
        let feature = make_feature_with_bucketing(
            "featPrior",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let feature_id = feature.id.clone();

        // Pre-populate with a sticky assignment
        let assignments = vec![("u1".to_string(), feature_id, "env1".to_string())];
        let state = test_state_with_assignments(feature, assignments).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featPrior".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u1".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "CA".into(), // Note: different from allowed "US" - should still return true due to cache
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(resp.into_inner().enabled); // Should be true due to cached assignment

        // Verify evaluation event was recorded with prior_assignment=true
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 1);
        assert!(events[0].prior_assignment); // This should be true for cached assignment
        assert_eq!(events[0].feature_key, "featPrior");
        assert_eq!(events[0].user_context, Some("u1".to_string()));
        assert!(events[0].evaluation_result); // Result should be true
    }

    #[actix_web::test]
    async fn test_evaluate_handler_prior_assignment_false() {
        // Test that fresh evaluations return prior_assignment=false
        let feature = make_feature_with_bucketing(
            "featFresh",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featFresh".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u2".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(resp.into_inner().enabled); // Should be true due to matching criteria

        // Verify evaluation event was recorded with prior_assignment=false
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 1);
        assert!(!events[0].prior_assignment); // This should be false for fresh evaluation
        assert_eq!(events[0].feature_key, "featFresh");
        assert_eq!(events[0].user_context, Some("u2".to_string()));
        assert!(events[0].evaluation_result);

        // Verify that assignment was cached for future use
        let assignments = state.assigned_true.read().await;
        let expected_key = assignment_key("u2", "featFresh_id", "env1");
        assert!(assignments.contains(&expected_key));
    }

    #[actix_web::test]
    async fn test_evaluate_handler_fresh_evaluation_false() {
        // Test fresh evaluation that results in false
        let feature = make_feature_with_bucketing(
            "featFalse",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featFalse".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u3".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "CA".into(), // Not in allowed list ["US"]
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(!resp.into_inner().enabled); // Should be false due to criteria mismatch

        // Verify evaluation event was recorded with prior_assignment=false
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 1);
        assert!(!events[0].prior_assignment); // Fresh evaluation
        assert!(!events[0].evaluation_result); // Result is false
        assert_eq!(events[0].user_context, Some("u3".to_string()));

        // Verify no assignment was cached (since result was false)
        let assignments = state.assigned_true.read().await;
        assert!(assignments.is_empty());
    }

    #[actix_web::test]
    async fn test_evaluate_handler_no_user_context() {
        // Test evaluation without user context (no bucketing key match)
        let feature = make_feature_with_bucketing(
            "featNoUser",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featNoUser".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext {
                key: "country".into(), // Missing user.id context but provides country
                value: "US".into(),
            }],
            client_id: None,
            client_secret: None,
        };

        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        let result = resp.into_inner().enabled;

        // Verify evaluation event
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 1);
        assert!(!events[0].prior_assignment); // Fresh evaluation (no cache possible without user context)
        assert_eq!(events[0].user_context, None); // No user context extracted
        assert_eq!(events[0].evaluation_result, result); // Result should match the actual evaluation

        // Verify no assignment was cached (no user context)
        let assignments = state.assigned_true.read().await;
        assert!(assignments.is_empty());
    }

    #[actix_web::test]
    async fn test_evaluate_handler_stage_disabled() {
        // Test evaluation when stage is disabled
        let feature = make_feature_with_bucketing(
            "featDisabled",
            "env1",
            false,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featDisabled".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u4".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(!resp.into_inner().enabled); // Should be false due to disabled stage

        // Verify no evaluation event was recorded (early return due to disabled stage)
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 0);

        // Verify no assignment was cached
        let assignments = state.assigned_true.read().await;
        assert!(assignments.is_empty());
    }

    #[actix_web::test]
    async fn test_evaluate_handler_rollout_percentage() {
        // Test evaluation with partial rollout
        let feature = make_feature_with_bucketing(
            "featRollout",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            0,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featRollout".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u5".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp = evaluate_handler(app_data, web::Json(req)).await.unwrap();
        assert!(!resp.into_inner().enabled); // Should be false due to 0% rollout

        // Verify evaluation event was recorded
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 1);
        assert!(!events[0].prior_assignment);
        assert!(!events[0].evaluation_result);
    }

    #[actix_web::test]
    async fn test_evaluate_handler_multiple_evaluations() {
        // Test that multiple evaluations are tracked correctly
        let feature = make_feature_with_bucketing(
            "featMulti",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        // First evaluation (fresh)
        let req1 = EvaluateHttpRequest {
            feature_key: "featMulti".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u6".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp1 = evaluate_handler(app_data.clone(), web::Json(req1))
            .await
            .unwrap();
        assert!(resp1.into_inner().enabled);

        // Second evaluation (should be cached now)
        let req2 = EvaluateHttpRequest {
            feature_key: "featMulti".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "u6".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "CA".into(), // Different context, but should still return true from cache
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp2 = evaluate_handler(app_data, web::Json(req2)).await.unwrap();
        assert!(resp2.into_inner().enabled);

        // Verify both evaluation events were recorded
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 2);

        // First event should be fresh evaluation
        assert!(!events[0].prior_assignment);
        assert!(events[0].evaluation_result);

        // Second event should be cached assignment
        assert!(events[1].prior_assignment);
        assert!(events[1].evaluation_result);
    }

    #[actix_web::test]
    async fn test_evaluate_handler_different_users_same_feature() {
        // Test that different users are handled independently
        let feature = make_feature_with_bucketing(
            "featUsers",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let feature_id = feature.id.clone();

        // Pre-populate assignment for user1 only
        let assignments = vec![("user1".to_string(), feature_id, "env1".to_string())];
        let state = test_state_with_assignments(feature, assignments).await;
        let app_data = web::Data::new(state.clone());

        // Evaluation for user1 (cached)
        let req1 = EvaluateHttpRequest {
            feature_key: "featUsers".into(),
            environment_id: "env1".into(),
            context: vec![HttpContext {
                key: "user.id".into(),
                value: "user1".into(),
            }],
            client_id: None,
            client_secret: None,
        };

        let resp1 = evaluate_handler(app_data.clone(), web::Json(req1))
            .await
            .unwrap();
        assert!(resp1.into_inner().enabled);

        // Evaluation for user2 (fresh)
        let req2 = EvaluateHttpRequest {
            feature_key: "featUsers".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "user2".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let resp2 = evaluate_handler(app_data, web::Json(req2)).await.unwrap();
        assert!(resp2.into_inner().enabled);

        // Verify evaluation events
        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 2);

        // user1 should have prior_assignment=true
        assert!(events[0].prior_assignment);
        assert_eq!(events[0].user_context, Some("user1".to_string()));

        // user2 should have prior_assignment=false
        assert!(!events[1].prior_assignment);
        assert_eq!(events[1].user_context, Some("user2".to_string()));
    }

    #[actix_web::test]
    async fn test_resolve_credentials() {
        let state = test_state_empty_cache();

        // Test with no overrides
        let req1 = EvaluateHttpRequest {
            feature_key: "test".into(),
            environment_id: "env1".into(),
            context: vec![],
            client_id: None,
            client_secret: None,
        };
        let (id1, secret1) = resolve_credentials(&state, &req1);
        assert_eq!(id1, "c");
        assert_eq!(secret1, "s");

        // Test with overrides
        let req2 = EvaluateHttpRequest {
            feature_key: "test".into(),
            environment_id: "env1".into(),
            context: vec![],
            client_id: Some("override_id".into()),
            client_secret: Some("override_secret".into()),
        };
        let (id2, secret2) = resolve_credentials(&state, &req2);
        assert_eq!(id2, "override_id");
        assert_eq!(secret2, "override_secret");
    }

    #[actix_web::test]
    async fn test_evaluation_event_structure() {
        // Test that evaluation events contain all expected fields
        let feature = make_feature_with_bucketing(
            "featStructure",
            "env1",
            true,
            "user.id",
            "country",
            &["US"],
            100,
        );
        let state = test_state_with_cache(feature).await;
        let app_data = web::Data::new(state.clone());

        let req = EvaluateHttpRequest {
            feature_key: "featStructure".into(),
            environment_id: "env1".into(),
            context: vec![
                HttpContext {
                    key: "user.id".into(),
                    value: "test_user".into(),
                },
                HttpContext {
                    key: "country".into(),
                    value: "US".into(),
                },
            ],
            client_id: None,
            client_secret: None,
        };

        let _resp = evaluate_handler(app_data, web::Json(req.clone()))
            .await
            .unwrap();

        let events = state.pending_evaluation_events.read().await;
        assert_eq!(events.len(), 1);

        let event = &events[0];
        assert_eq!(event.feature_key, "featStructure");
        assert_eq!(event.environment_id, "env1");
        assert_eq!(event.evaluation_result, true);
        assert_eq!(event.evaluation_context, req.context);
        assert_eq!(event.user_context, Some("test_user".to_string()));
        assert!(!event.prior_assignment);
        // evaluated_at should be set to a recent timestamp
        assert!(event.evaluated_at.elapsed().unwrap().as_secs() < 1);
    }
}
