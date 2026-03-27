mod flush;
mod stream;

use crate::AppState;
use crate::pb;
use std::time::Duration;
use tokio_retry::{Retry, RetryIf};
use tonic::transport::Endpoint;
use tracing::{error, info};

pub use flush::{run_evaluation_flush_task, run_flush_task};
pub use stream::run_stream_task;
#[cfg(test)]
pub(crate) use stream::{handle_feature_update, prepare_for_full_resync, send_initial_subscribe};

#[derive(Clone, Debug)]
pub struct UserAssignment {
    pub user_id: String,
    pub feature_id: String,
    pub environment_id: String,
    pub assigned: bool,
    pub variant: Option<String>,
}

fn should_retry_feature_fetch(status: &tonic::Status) -> bool {
    status.code() != tonic::Code::NotFound
}

/// Fetch a feature by key from the backend via gRPC with retry logic
pub async fn fetch_feature_via_grpc(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<Option<pb::FeatureFull>, tonic::Status> {
    use tokio_retry::strategy::ExponentialBackoff;

    // Retry with exponential backoff using config values
    let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
        .take(app.retry_config.max_attempts);
    let action = || async {
        // Clone the client to allow gRPC channel reconnection on retry
        let mut client = {
            let guard = app.grpc.lock().await;
            guard.clone()
        };
        let request = pb::GetFeatureByKeyRequest {
            feature_key: feature_key.to_string(),
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
        };
        client
            .get_feature_by_key(tonic::Request::new(request))
            .await
    };

    match RetryIf::spawn(retry_strategy, action, should_retry_feature_fetch).await {
        Ok(resp) => {
            let feature = resp.into_inner().feature;
            if feature.is_some() {
                info!("Successfully fetched feature: {}", feature_key);
            }
            Ok(feature)
        }
        Err(status) if status.code() == tonic::Code::NotFound => {
            info!("Feature '{}' not found in backend", feature_key);
            Ok(None)
        }
        Err(status) => {
            error!(
                "gRPC GetFeatureByKey error after retries for feature '{}': code={:?} msg={}",
                feature_key,
                status.code(),
                status.message()
            );
            Err(status)
        }
    }
}

/// Fetch client information from the backend via gRPC with retry logic
/// This is the low-level function that always fetches from backend
async fn fetch_client_info_via_grpc_uncached(
    app: &AppState,
    client_id: &str,
    client_secret: &str,
) -> Option<pb::GetClientInfoResponse> {
    use tokio_retry::strategy::ExponentialBackoff;

    // Retry with exponential backoff using config values
    let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
        .take(app.retry_config.max_attempts);
    let action = || async {
        // Clone the client to allow gRPC channel reconnection on retry
        let mut client = {
            let guard = app.grpc.lock().await;
            guard.clone()
        };
        let request = pb::GetClientInfoRequest {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
        };
        client.get_client_info(tonic::Request::new(request)).await
    };

    match Retry::spawn(retry_strategy, action).await {
        Ok(resp) => {
            let client_info = resp.into_inner();
            info!("Successfully fetched client info for: {}", client_id);
            Some(client_info)
        }
        Err(e) => {
            error!(
                "gRPC GetClientInfo error after retries for client '{}': {}",
                client_id, e
            );
            None
        }
    }
}

/// Get client info from cache or fetch from backend
/// This is the high-level function that uses caching
pub async fn get_or_fetch_client_info(
    app: &AppState,
    client_id: &str,
    client_secret: &str,
) -> Option<pb::GetClientInfoResponse> {
    // Cache entries are scoped by both ID and secret so rotated credentials
    // cannot reuse stale authorization results.
    let cache_key = format!("{client_id}:{client_secret}");

    // Check cache first
    if let Some(cached) = app.client_info_cache.get(&cache_key).await {
        return Some(cached);
    }

    // Cache miss - fetch from backend
    let client_info = fetch_client_info_via_grpc_uncached(app, client_id, client_secret).await?;

    // Store in cache for future requests
    app.client_info_cache
        .insert(cache_key, client_info.clone())
        .await;

    Some(client_info)
}

/// Load user assignments from backend on startup
pub async fn load_user_assignments(app: &AppState) -> Result<usize, tonic::Status> {
    let req = pb::ListUserFlagAssignmentsRequest {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
        environment_id: String::new(),
        feature_id: String::new(),
    };
    let mut client = app.grpc.lock().await.clone();
    let resp = client.list_user_assignments(req).await?.into_inner();
    let mut count = 0usize;
    for a in resp.assignments.into_iter() {
        if a.assigned {
            let key = assignment_key(&a.user_id, &a.feature_id, &a.environment_id);
            app.assigned_cache.insert(
                key,
                crate::CachedAssignment {
                    value: serde_json::json!(true),
                    variant: if a.variant.is_empty() {
                        None
                    } else {
                        Some(a.variant)
                    },
                    // When loading from database, we don't have the original reason
                    // Use TargetingMatch as a reasonable default for assigned users
                    reason: evaluation_engine::EvaluationReason::TargetingMatch,
                },
            );
            count += 1;
        }
    }
    Ok(count)
}

/// Build a gRPC endpoint with standard configuration
pub fn build_endpoint(grpc_addr: &str) -> Endpoint {
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

/// Generate a unique key for user assignment caching
pub fn assignment_key(user_id: &str, feature_id: &str, environment_id: &str) -> String {
    format!("{}|{}|{}", user_id, feature_id, environment_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use feature_toggle_backend::grpc::pb as backend_pb;
    use feature_toggle_backend::grpc::pb::feature_evaluation_server::{
        FeatureEvaluation, FeatureEvaluationServer,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_retry::strategy::ExponentialBackoff;
    use tokio_stream::StreamExt;
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use tonic::transport::{Endpoint, Server};
    use tonic::{Request, Response, Status};

    #[derive(Default)]
    struct MockBackendState {
        assignment_attempts: AtomicUsize,
        evaluation_attempts: AtomicUsize,
    }

    #[derive(Clone)]
    struct MockBackend {
        state: Arc<MockBackendState>,
    }

    #[tonic::async_trait]
    impl FeatureEvaluation for MockBackend {
        type StreamUpdatesStream = ReceiverStream<Result<backend_pb::FeatureUpdate, Status>>;

        async fn evaluate(
            &self,
            _request: Request<backend_pb::EvaluateRequest>,
        ) -> Result<Response<backend_pb::EvaluateResponse>, Status> {
            Err(Status::unimplemented("not used in edge ingestion tests"))
        }

        async fn get_feature_by_key(
            &self,
            _request: Request<backend_pb::GetFeatureByKeyRequest>,
        ) -> Result<Response<backend_pb::GetFeatureByKeyResponse>, Status> {
            Err(Status::unimplemented("not used in edge ingestion tests"))
        }

        async fn get_client_info(
            &self,
            _request: Request<backend_pb::GetClientInfoRequest>,
        ) -> Result<Response<backend_pb::GetClientInfoResponse>, Status> {
            Err(Status::unimplemented("not used in edge ingestion tests"))
        }

        async fn push_user_assignments(
            &self,
            request: Request<tonic::Streaming<backend_pb::UserFlagAssignment>>,
        ) -> Result<Response<backend_pb::Ack>, Status> {
            let attempt = self
                .state
                .assignment_attempts
                .fetch_add(1, Ordering::SeqCst);
            let mut count = 0usize;
            let mut stream = request.into_inner();
            while let Some(msg) = stream.next().await {
                msg.map_err(|e| Status::internal(format!("stream error: {e}")))?;
                count += 1;
            }

            if attempt == 0 {
                return Err(Status::unavailable("transient assignment ingest failure"));
            }

            Ok(Response::new(backend_pb::Ack {
                message_id: format!("assignment-ack-{}", count),
            }))
        }

        async fn list_user_assignments(
            &self,
            _request: Request<backend_pb::ListUserFlagAssignmentsRequest>,
        ) -> Result<Response<backend_pb::ListUserFlagAssignmentsResponse>, Status> {
            Err(Status::unimplemented("not used in edge ingestion tests"))
        }

        async fn stream_updates(
            &self,
            _request: Request<tonic::Streaming<backend_pb::StreamRequest>>,
        ) -> Result<Response<Self::StreamUpdatesStream>, Status> {
            let (_tx, rx) =
                tokio::sync::mpsc::channel::<Result<backend_pb::FeatureUpdate, Status>>(1);
            Ok(Response::new(ReceiverStream::new(rx)))
        }

        async fn push_evaluation_events(
            &self,
            request: Request<backend_pb::PushEvaluationEventsRequest>,
        ) -> Result<Response<backend_pb::PushEvaluationEventsResponse>, Status> {
            let req = request.into_inner();
            let attempt = self
                .state
                .evaluation_attempts
                .fetch_add(1, Ordering::SeqCst);

            if attempt == 0 {
                return Err(Status::unavailable("transient evaluation ingest failure"));
            }

            Ok(Response::new(backend_pb::PushEvaluationEventsResponse {
                message_id: format!("evaluation-ack-{}", req.events.len()),
                processed_count: req.events.len() as i32,
            }))
        }

        async fn track_metrics(
            &self,
            _request: Request<backend_pb::TrackMetricRequest>,
        ) -> Result<Response<backend_pb::TrackMetricResponse>, Status> {
            Err(Status::unimplemented("not used in edge ingestion tests"))
        }
    }

    fn test_app_state_with_endpoint(
        mapped_cache: Arc<crate::MappedFeatureCache>,
        endpoint: &str,
    ) -> crate::AppState {
        let channel = Endpoint::from_shared(endpoint.to_string())
            .expect("valid gRPC endpoint")
            .connect_lazy();
        let grpc_client =
            crate::pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let client_info_cache = Arc::new(crate::ClientInfoCache::new(
            std::time::Duration::from_secs(300),
        ));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(10);

        crate::AppState {
            mapped_cache,
            client_info_cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "test-client-id".to_string(),
            client_secret: "test-secret".to_string(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(dashmap::DashMap::new()),
            pending_assignments: Arc::new(crossbeam::queue::SegQueue::new()),
            flush_interval: std::time::Duration::from_secs(10),
            assignment_flush_batch_size: 1000,
            evaluation_event_tx: event_tx,
            evaluation_flush_interval: std::time::Duration::from_secs(30),
            evaluation_flush_batch_size: 500,
            evaluation_event_queue_capacity: 10,
            evaluation_event_dropped: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            retry_config: crate::config::RetryConfig::default(),
        }
    }

    fn test_app_state(mapped_cache: Arc<crate::MappedFeatureCache>) -> crate::AppState {
        test_app_state_with_endpoint(mapped_cache, "http://127.0.0.1:50051")
    }

    async fn start_mock_backend() -> (String, Arc<MockBackendState>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind mock backend listener");
        let addr = listener.local_addr().expect("listener addr");
        let incoming = TcpListenerStream::new(listener);
        let state = Arc::new(MockBackendState::default());
        let svc = MockBackend {
            state: state.clone(),
        };
        let router = Server::builder().add_service(FeatureEvaluationServer::new(svc));
        let handle = tokio::spawn(async move {
            router
                .serve_with_incoming(incoming)
                .await
                .expect("mock backend should run");
        });

        (format!("http://{}", addr), state, handle)
    }

    #[tokio::test]
    async fn test_send_initial_subscribe_with_cached_keys() {
        // Create a cache and populate it with features
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));

        // Add some features to the cache
        for i in 1..=5 {
            let feature = Arc::new(evaluation_engine::Feature {
                id: format!("id_{}", i),
                key: format!("feature_key_{}", i),
                feature_type: "Simple".to_string(),
                active: true,
                enabled: true,
                dependencies: vec![],
                stages: vec![],
                variants: vec![],
            });
            mapped_cache.insert(feature).await;
        }

        let app_state = test_app_state(mapped_cache);

        // Create a channel to send the subscribe request
        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::pb::StreamRequest>(10);

        // Call send_initial_subscribe
        send_initial_subscribe(&tx, &app_state, false).await;

        // Receive the message
        let received = rx.recv().await;
        assert!(received.is_some());

        let stream_request = received.unwrap();
        assert!(stream_request.payload.is_some());

        // Extract the subscribe request
        match stream_request.payload.unwrap() {
            crate::pb::stream_request::Payload::Subscribe(subscribe) => {
                // Verify client credentials
                assert_eq!(subscribe.client_id, "test-client-id");
                assert_eq!(subscribe.client_secret, "test-secret");

                // Verify that cached feature keys were sent
                assert_eq!(subscribe.feature_keys.len(), 5);

                // Verify all feature keys are present
                for i in 1..=5 {
                    let expected_key = format!("feature_key_{}", i);
                    assert!(
                        subscribe.feature_keys.contains(&expected_key),
                        "Expected to find {} in feature_keys",
                        expected_key
                    );
                }
            }
            _ => panic!("Expected Subscribe payload"),
        }
    }

    #[tokio::test]
    async fn test_send_initial_subscribe_with_empty_cache() {
        // Create an empty cache
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        let app_state = test_app_state(mapped_cache);

        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::pb::StreamRequest>(10);

        send_initial_subscribe(&tx, &app_state, false).await;

        let received = rx.recv().await;
        assert!(received.is_some());

        let stream_request = received.unwrap();
        match stream_request.payload.unwrap() {
            crate::pb::stream_request::Payload::Subscribe(subscribe) => {
                // Empty cache should send empty feature_keys array
                assert_eq!(subscribe.feature_keys.len(), 0);
            }
            _ => panic!("Expected Subscribe payload"),
        }
    }

    #[tokio::test]
    async fn test_send_initial_subscribe_forces_full_snapshot_after_lag() {
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        mapped_cache
            .insert(Arc::new(evaluation_engine::Feature {
                id: "id_1".to_string(),
                key: "feature_key_1".to_string(),
                feature_type: "Simple".to_string(),
                active: true,
                enabled: true,
                dependencies: vec![],
                stages: vec![],
                variants: vec![],
            }))
            .await;
        let app_state = test_app_state(mapped_cache);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::pb::StreamRequest>(10);

        send_initial_subscribe(&tx, &app_state, true).await;

        let received = rx.recv().await.expect("missing subscribe request");
        match received.payload.expect("missing payload") {
            crate::pb::stream_request::Payload::Subscribe(subscribe) => {
                assert!(
                    subscribe.feature_keys.is_empty(),
                    "full resync should request a complete snapshot"
                );
            }
            _ => panic!("Expected Subscribe payload"),
        }
    }

    #[test]
    fn test_assignment_key_format() {
        let key = assignment_key("user-123", "feature-456", "env-789");
        assert_eq!(key, "user-123|feature-456|env-789");
    }

    #[tokio::test]
    async fn test_fetch_feature_retry_skips_not_found() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let action = move || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<tonic::Response<pb::GetFeatureByKeyResponse>, tonic::Status>(
                    tonic::Status::not_found("missing"),
                )
            }
        };

        let result = RetryIf::spawn(
            ExponentialBackoff::from_millis(0).take(3),
            action,
            should_retry_feature_fetch,
        )
        .await;

        assert!(matches!(result, Err(status) if status.code() == tonic::Code::NotFound));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_fetch_feature_retry_retries_transient_statuses() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let action = move || {
            let attempts = attempts_clone.clone();
            async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err::<tonic::Response<pb::GetFeatureByKeyResponse>, tonic::Status>(
                        tonic::Status::unavailable("transient"),
                    )
                } else {
                    Ok(tonic::Response::new(pb::GetFeatureByKeyResponse {
                        feature: Some(pb::FeatureFull {
                            id: "feature-id".to_string(),
                            key: "flag".to_string(),
                            description: String::new(),
                            feature_type: "Simple".to_string(),
                            team_id: "team-1".to_string(),
                            created_at: "2026-03-26T00:00:00Z".to_string(),
                            active: true,
                            kill_switch_enabled: true,
                            kill_switch_activated_at: String::new(),
                            rollback_scheduled_at: String::new(),
                            stages: vec![],
                            dependencies: vec![],
                            variants: vec![],
                        }),
                    }))
                }
            }
        };

        let result = RetryIf::spawn(
            ExponentialBackoff::from_millis(0).take(3),
            action,
            should_retry_feature_fetch,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_handle_feature_update_populates_cache() {
        // Create test AppState
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        let app_state = test_app_state(mapped_cache.clone());

        // Create a test feature
        let test_feature = crate::pb::FeatureFull {
            id: "feature-id-123".to_string(),
            key: "test_feature_key".to_string(),
            description: "Test feature".to_string(),
            feature_type: "Simple".to_string(),
            team_id: "team-1".to_string(),
            created_at: "2024-01-01".to_string(),
            active: true,
            kill_switch_enabled: false,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: String::new(),
            stages: vec![],
            dependencies: vec![],
            variants: vec![],
        };

        // Create an UPSERT update
        let update = crate::pb::FeatureUpdate {
            action: crate::pb::feature_update::Action::Upsert as i32,
            feature: Some(test_feature.clone()),
            feature_key: test_feature.key.clone(),
            error: String::new(),
            message_id: String::new(),
        };

        // Handle the update
        handle_feature_update(&app_state, update).await;

        // Verify mapped cache is populated
        let cached_mapped = mapped_cache.get("test_feature_key").await;
        assert!(
            cached_mapped.is_some(),
            "Mapped cache should contain the feature"
        );
        assert_eq!(cached_mapped.unwrap().id, "feature-id-123");
    }

    #[tokio::test]
    async fn test_lag_recovery_clears_stale_cache_and_requests_full_snapshot() {
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        let app_state = test_app_state(mapped_cache.clone());

        let stale_feature = Arc::new(evaluation_engine::Feature {
            id: "stale-id".to_string(),
            key: "stale-key".to_string(),
            feature_type: "Simple".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        });
        mapped_cache.insert(stale_feature).await;
        mapped_cache.add_negative("stale-miss").await;
        app_state.assigned_cache.insert(
            assignment_key("user-1", "stale-id", "env-1"),
            crate::CachedAssignment {
                value: serde_json::json!(true),
                variant: None,
                reason: evaluation_engine::EvaluationReason::Static,
            },
        );
        app_state.pending_assignments.push(UserAssignment {
            user_id: "user-1".to_string(),
            feature_id: "stale-id".to_string(),
            environment_id: "env-1".to_string(),
            assigned: true,
            variant: None,
        });

        let lagged = pb::FeatureUpdate {
            action: pb::feature_update::Action::Error as i32,
            feature: None,
            feature_key: String::new(),
            error: "lagged".to_string(),
            message_id: "lag-1".to_string(),
        };
        assert!(handle_feature_update(&app_state, lagged).await);

        prepare_for_full_resync(&app_state).await;
        mapped_cache.run_pending_tasks().await;
        assert_eq!(mapped_cache.entry_count(), 0);
        assert!(mapped_cache.get("stale-key").await.is_none());
        assert!(!mapped_cache.is_negative_cached("stale-miss").await);
        assert!(app_state.assigned_cache.is_empty());
        assert!(app_state.pending_assignments.pop().is_none());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::pb::StreamRequest>(10);
        send_initial_subscribe(&tx, &app_state, true).await;
        let subscribe = rx.recv().await.expect("missing subscribe message");
        match subscribe.payload.expect("missing payload") {
            crate::pb::stream_request::Payload::Subscribe(subscribe) => {
                assert!(subscribe.feature_keys.is_empty());
            }
            _ => panic!("Expected Subscribe payload"),
        }

        let snapshot_feature = crate::pb::FeatureFull {
            id: "fresh-id".to_string(),
            key: "fresh-key".to_string(),
            description: "Recovered".to_string(),
            feature_type: "Simple".to_string(),
            team_id: "team-1".to_string(),
            created_at: "2024-01-01".to_string(),
            active: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: String::new(),
            stages: vec![],
            dependencies: vec![],
            variants: vec![],
        };
        let snapshot = crate::pb::FeatureUpdate {
            action: crate::pb::feature_update::Action::Snapshot as i32,
            feature: Some(snapshot_feature.clone()),
            feature_key: snapshot_feature.key.clone(),
            error: String::new(),
            message_id: "snapshot-1".to_string(),
        };

        assert!(!handle_feature_update(&app_state, snapshot).await);
        let recovered = mapped_cache.get("fresh-key").await;
        assert!(
            recovered.is_some(),
            "fresh snapshot should repopulate cache"
        );
        assert_eq!(recovered.unwrap().id, "fresh-id");
        assert!(mapped_cache.get("stale-key").await.is_none());
    }

    #[tokio::test]
    async fn test_run_flush_task_requeues_assignments_after_failure() {
        let (endpoint, state, server_handle) = start_mock_backend().await;
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        let mut app_state = test_app_state_with_endpoint(mapped_cache, &endpoint);
        app_state.flush_interval = std::time::Duration::from_millis(0);
        let user_id = "user-1".to_string();
        let feature_id = "feature-1".to_string();
        let environment_id = "env-1".to_string();
        app_state.pending_assignments.push(UserAssignment {
            user_id: user_id.clone(),
            feature_id: feature_id.clone(),
            environment_id: environment_id.clone(),
            assigned: true,
            variant: Some("variant-a".to_string()),
        });

        let task = tokio::spawn(run_flush_task(app_state.clone()));

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if state.assignment_attempts.load(Ordering::SeqCst) >= 2
                    && app_state.pending_assignments.pop().is_none()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("assignment flush should retry and drain");

        assert_eq!(state.assignment_attempts.load(Ordering::SeqCst), 2);
        assert!(app_state.pending_assignments.pop().is_none());

        task.abort();
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_run_evaluation_flush_task_retries_transient_failure() {
        let (endpoint, state, server_handle) = start_mock_backend().await;
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        let mut app_state = test_app_state_with_endpoint(mapped_cache, &endpoint);
        app_state.evaluation_flush_interval = std::time::Duration::from_millis(0);
        app_state.retry_config.base_delay_ms = 0;
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(10);
        let event = crate::EvaluationEvent {
            feature_key: "feature-a".to_string(),
            environment_id: "env-a".to_string(),
            evaluation_result: true,
            evaluation_context: crate::handlers::EvaluateContext {
                bucketing_key: "user-1".to_string(),
                environment_id: "env-a".to_string(),
                attributes: std::collections::HashMap::new(),
            },
            user_context: Some("user-1".to_string()),
            evaluated_at: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(42),
            prior_assignment: false,
            variant: Some("control".to_string()),
            variant_value: Some(serde_json::json!({"enabled": true})),
        };
        event_tx.send(event).await.expect("queue event");
        drop(event_tx);

        let task = tokio::spawn(run_evaluation_flush_task(app_state.clone(), event_rx));

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if state.evaluation_attempts.load(Ordering::SeqCst) >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("evaluation flush should retry");

        assert_eq!(state.evaluation_attempts.load(Ordering::SeqCst), 2);
        task.abort();
        server_handle.abort();
    }
}
