use crate::AppState;
use crate::pb;
use std::time::Duration;
use tokio_retry::Retry;
use tokio_stream::StreamExt;
use tonic::transport::Endpoint;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub struct UserAssignment {
    pub user_id: String,
    pub feature_id: String,
    pub environment_id: String,
    pub assigned: bool,
    pub variant: Option<String>,
}

/// Fetch a feature by key from the backend via gRPC with retry logic
pub async fn fetch_feature_via_grpc(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<pb::FeatureFull> {
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

    match Retry::spawn(retry_strategy, action).await {
        Ok(resp) => {
            let feature = resp.into_inner().feature;
            if feature.is_some() {
                info!("Successfully fetched feature: {}", feature_key);
            }
            feature
        }
        Err(e) => {
            error!(
                "gRPC GetFeatureByKey error after retries for feature '{}': {}",
                feature_key, e
            );
            None
        }
    }
}

/// Fetch client information from the backend via gRPC with retry logic
pub async fn fetch_client_info_via_grpc(
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
    {
        let mut cache = app.assigned_cache.write().await;
        for a in resp.assignments.into_iter() {
            if a.assigned {
                let key = assignment_key(&a.user_id, &a.feature_id, &a.environment_id);
                cache.insert(
                    key,
                    crate::CachedAssignment {
                        value: serde_json::json!(true),
                        variant: if a.variant.is_empty() {
                            None
                        } else {
                            Some(a.variant)
                        },
                    },
                );
                count += 1;
            }
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

/// Send initial subscribe message to the streaming connection
async fn send_initial_subscribe(tx: &tokio::sync::mpsc::Sender<pb::StreamRequest>, app: &AppState) {
    // Collect all cached feature keys to send to backend
    // This allows backend to rebuild its memory of which features this client is interested in
    let cached_keys = app.cache.get_all_keys().await;

    tracing::info!("Subscribing with {} cached feature keys", cached_keys.len());

    let subscribe = pb::SubscribeRequest {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
        feature_keys: cached_keys,
        environment_id: "".into(),
    };
    let initial = pb::StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(subscribe)),
    };
    let _ = tx.send(initial).await;
}

/// Spawn a background task to send periodic heartbeats
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

/// Open a streaming gRPC call for feature updates
async fn open_streaming_call(
    mut client: pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
    rx: tokio::sync::mpsc::Receiver<pb::StreamRequest>,
) -> Result<tonic::Response<tonic::Streaming<pb::FeatureUpdate>>, tonic::Status> {
    use tokio_stream::wrappers::ReceiverStream;
    let req_stream = ReceiverStream::new(rx);
    client.stream_updates(req_stream).await
}

/// Handle a feature update message from the stream
async fn handle_feature_update(app: &AppState, update: pb::FeatureUpdate) {
    use pb::feature_update::Action;
    match update.action {
        x if x == Action::Upsert as i32 || x == Action::Snapshot as i32 => {
            if let Some(f) = update.feature {
                let feature_id = f.id.clone();
                
                app.cache.upsert(f).await;
                // We are purging assignments for feature in evenry feature update 
                // so that we can make sure 
                app.purge_assignments_for_feature(&feature_id).await;

            }
        }
        x if x == Action::Delete as i32 => {
            if !update.feature_key.is_empty() {
                if let Some(feature_id) = app.cache.delete_by_key(&update.feature_key).await {
                    app.purge_assignments_for_feature(&feature_id).await;
                }
            }
        }
        _ => {}
    }
}

/// Background task to maintain streaming connection with backend
pub async fn run_stream_task(app: AppState, grpc_addr: String) {
    use std::sync::atomic::Ordering;

    // Exponential backoff for stream reconnection using config values
    let mut retry_delay = app.retry_config.stream_initial_delay();
    let max_retry_delay = app.retry_config.stream_max_delay();

    loop {
        // reset status while attempting to connect
        app.connected.store(false, Ordering::Relaxed);

        // Try to connect with retry
        let endpoint = build_endpoint(&grpc_addr);
        match endpoint.connect().await {
            Ok(channel) => {
                let client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
                info!("Connected to backend gRPC {}", &grpc_addr);

                // Reset retry delay on successful connection
                retry_delay = app.retry_config.stream_initial_delay();

                let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);

                // Send initial Subscribe BEFORE opening the streaming call
                send_initial_subscribe(&tx, &app).await;

                // Heartbeats
                spawn_heartbeat(tx.clone());

                let response = match open_streaming_call(client, rx).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Failed to open streaming call: {}", e);
                        app.connected.store(false, Ordering::Relaxed);
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
                        continue;
                    }
                };

                // streaming successfully opened -> mark healthy
                app.connected.store(true, Ordering::Relaxed);
                info!("Stream connection established, receiving updates");
                let mut inbound = response.into_inner();

                // Process updates
                while let Some(msg) = inbound.next().await {
                    match msg {
                        Ok(update) => handle_feature_update(&app, update).await,
                        Err(e) => {
                            error!("Stream error: {}", e);
                            break;
                        }
                    }
                }

                // stream closed -> mark unhealthy
                app.connected.store(false, Ordering::Relaxed);
                warn!("Stream connection closed, will retry in {:?}", retry_delay);
            }
            Err(e) => {
                error!("Failed to connect to backend gRPC {}: {}", &grpc_addr, e);
                app.connected.store(false, Ordering::Relaxed);
                warn!("Retrying connection in {:?}", retry_delay);
            }
        }

        // Wait before reconnecting with exponential backoff
        tokio::time::sleep(retry_delay).await;
        retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
    }
}

/// Background task to periodically flush user assignments to backend
pub async fn run_flush_task(app: AppState) {
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
            variant: to_send[0].variant.clone().unwrap_or_default(),
        };
        // Spawn sender
        tokio::spawn({
            let _app_clone = app.clone();
            let rest = to_send[1..].to_vec();
            let tx_clone = tx.clone();
            async move {
                let _ = tx_clone.send(creds_first).await;
                for a in rest {
                    let assignment = pb::UserFlagAssignment {
                        user_id: a.user_id,
                        feature_id: a.feature_id,
                        environment_id: a.environment_id,
                        assigned: a.assigned,
                        client_id: String::new(),
                        client_secret: String::new(),
                        variant: a.variant.unwrap_or_default(),
                    };
                    let _ = tx_clone.send(assignment).await;
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

        // Note: Streaming calls can't be easily retried as the stream is consumed
        // If this fails, items will be requeued for the next flush cycle
        match client.push_user_assignments(stream).await {
            Ok(_) => {
                info!("Successfully pushed {} user assignments", to_send.len());
            }
            Err(e) => {
                error!("Failed to push user assignments: {}", e);
                warn!(
                    "Will retry on next flush cycle ({}s)",
                    app.flush_interval.as_secs()
                );
                // requeue for next flush attempt
                let mut lock = app.pending_assignments.write().await;
                lock.extend(to_send);
            }
        }
    }
}

/// Background task to periodically flush evaluation events to backend
pub async fn run_evaluation_flush_task(app: AppState) {
    loop {
        tokio::time::sleep(app.evaluation_flush_interval).await;

        // Drain pending evaluation events
        let to_send: Vec<crate::EvaluationEvent> = {
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

            // Convert EvaluateContext to proto Context entries
            let mut proto_context = Vec::new();

            // Add bucketing_key as a context entry
            proto_context.push(pb::Context {
                key: "bucketingKey".to_string(),
                value: event.evaluation_context.bucketing_key.clone(),
            });

            // Add all dynamic attributes as context entries
            for (key, value) in &event.evaluation_context.attributes {
                // Convert JSON value to string
                let value_str = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => value.to_string(),
                };
                proto_context.push(pb::Context {
                    key: key.clone(),
                    value: value_str,
                });
            }

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
                variant: event.variant.clone().unwrap_or_default(),
            });
        }

        // Retry evaluation events push with exponential backoff using config values
        use tokio_retry::strategy::ExponentialBackoff;
        let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
            .take(app.retry_config.max_attempts);
        let action = || async {
            let mut client = {
                let guard = app.grpc.lock().await;
                guard.clone()
            };
            let req = pb::PushEvaluationEventsRequest {
                events: proto_events.clone(),
            };
            client.push_evaluation_events(req).await
        };

        match Retry::spawn(retry_strategy, action).await {
            Ok(response) => {
                let resp = response.into_inner();
                info!(
                    "Successfully pushed {} evaluation events ({} processed)",
                    to_send.len(),
                    resp.processed_count
                );
            }
            Err(e) => {
                error!("Failed to push evaluation events after retries: {}", e);
                warn!(
                    "Will retry on next flush cycle ({}s)",
                    app.evaluation_flush_interval.as_secs()
                );
                // Requeue the events on failure
                let mut lock = app.pending_evaluation_events.write().await;
                lock.extend(to_send);
            }
        }
    }
}

/// Generate a unique key for user assignment caching
pub fn assignment_key(user_id: &str, feature_id: &str, environment_id: &str) -> String {
    format!("{}|{}|{}", user_id, feature_id, environment_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeatureCache;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tonic::transport::Endpoint;

    #[tokio::test]
    async fn test_send_initial_subscribe_with_cached_keys() {
        // Create a cache and populate it with features
        let cache = Arc::new(FeatureCache::new(100));

        // Add some features to the cache
        for i in 1..=5 {
            let feature = crate::pb::FeatureFull {
                id: format!("id_{}", i),
                key: format!("feature_key_{}", i),
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

        // Create AppState with the populated cache
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = crate::pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

        let app_state = crate::AppState {
            cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "test-client-id".to_string(),
            client_secret: "test-secret".to_string(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            pending_assignments: Arc::new(RwLock::new(Vec::new())),
            flush_interval: std::time::Duration::from_secs(10),
            pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
            evaluation_flush_interval: std::time::Duration::from_secs(30),
            retry_config: crate::config::RetryConfig::default(),
        };

        // Create a channel to send the subscribe request
        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::pb::StreamRequest>(10);

        // Call send_initial_subscribe
        send_initial_subscribe(&tx, &app_state).await;

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
        let cache = Arc::new(FeatureCache::new(100));

        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = crate::pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

        let app_state = crate::AppState {
            cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "test-client-id".to_string(),
            client_secret: "test-secret".to_string(),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            pending_assignments: Arc::new(RwLock::new(Vec::new())),
            flush_interval: std::time::Duration::from_secs(10),
            pending_evaluation_events: Arc::new(RwLock::new(Vec::new())),
            evaluation_flush_interval: std::time::Duration::from_secs(30),
            retry_config: crate::config::RetryConfig::default(),
        };

        let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::pb::StreamRequest>(10);

        send_initial_subscribe(&tx, &app_state).await;

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

    #[test]
    fn test_assignment_key_format() {
        let key = assignment_key("user-123", "feature-456", "env-789");
        assert_eq!(key, "user-123|feature-456|env-789");
    }
}
