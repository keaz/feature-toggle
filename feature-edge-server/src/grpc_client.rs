use crate::AppState;
use crate::pb;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
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
    // Check cache first
    if let Some(cached) = app.client_info_cache.get(client_id).await {
        return Some(cached);
    }

    // Cache miss - fetch from backend
    let client_info = fetch_client_info_via_grpc_uncached(app, client_id, client_secret).await?;

    // Store in cache for future requests
    app.client_info_cache
        .insert(client_id.to_string(), client_info.clone())
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

/// Send initial subscribe message to the streaming connection
async fn send_initial_subscribe(tx: &tokio::sync::mpsc::Sender<pb::StreamRequest>, app: &AppState) {
    // Collect all cached feature keys to send to backend
    // This allows backend to rebuild its memory of which features this client is interested in
    let cached_keys = app.mapped_cache.get_all_keys().await;

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

                // Map protobuf to engine format and cache
                let engine_feature = std::sync::Arc::new(crate::handlers::map_proto_to_engine(&f));
                app.mapped_cache.insert(engine_feature).await;

                // Purge assignments for feature on every feature update
                app.purge_assignments_for_feature(&feature_id).await;
            }
        }
        x if x == Action::Delete as i32 => {
            if !update.feature_key.is_empty()
                && let Some(feature_id) = app.mapped_cache.delete_by_key(&update.feature_key).await
            {
                app.purge_assignments_for_feature(&feature_id).await;
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
    let batch_size = app.assignment_flush_batch_size.max(1);
    loop {
        tokio::time::sleep(app.flush_interval).await;

        let mut total_unique = 0usize;
        let mut total_drained = 0usize;
        let mut batches = 0usize;
        let mut failed = false;

        loop {
            let mut drained = 0usize;
            let mut dedup: HashMap<String, UserAssignment> = HashMap::new();

            while drained < batch_size {
                match app.pending_assignments.pop() {
                    Some(assignment) => {
                        drained += 1;
                        let key = assignment_key(
                            &assignment.user_id,
                            &assignment.feature_id,
                            &assignment.environment_id,
                        );
                        dedup.insert(key, assignment);
                    }
                    None => break,
                }
            }

            if dedup.is_empty() {
                break;
            }

            total_drained += drained;
            let assignment_count = dedup.len();

            let client_id = app.client_id.clone();
            let client_secret = app.client_secret.clone();
            let stream =
                tokio_stream::iter(dedup.into_values().enumerate().map(move |(idx, a)| {
                    pb::UserFlagAssignment {
                        user_id: a.user_id,
                        feature_id: a.feature_id,
                        environment_id: a.environment_id,
                        assigned: a.assigned,
                        // Only send credentials on first message
                        client_id: if idx == 0 {
                            client_id.clone()
                        } else {
                            String::new()
                        },
                        client_secret: if idx == 0 {
                            client_secret.clone()
                        } else {
                            String::new()
                        },
                        variant: a.variant.unwrap_or_default(),
                    }
                }));

            // Use a cloned client to avoid holding the lock
            let mut client = {
                let guard = app.grpc.lock().await;
                guard.clone()
            };

            // Note: Streaming calls can't be easily retried as the stream is consumed
            match client.push_user_assignments(stream).await {
                Ok(_) => {
                    total_unique += assignment_count;
                    batches += 1;
                }
                Err(e) => {
                    error!("Failed to push user assignments: {}", e);
                    warn!(
                        "Will retry on next flush cycle ({}s)",
                        app.flush_interval.as_secs()
                    );
                    failed = true;
                    break;
                }
            }
        }

        if !failed && total_unique > 0 {
            info!(
                "Successfully pushed {} user assignments in {} batch(es) ({} drained)",
                total_unique, batches, total_drained
            );
        }
    }
}

/// Background task to periodically flush evaluation events to backend
pub async fn run_evaluation_flush_task(
    app: AppState,
    mut event_rx: tokio::sync::mpsc::Receiver<crate::EvaluationEvent>,
) {
    let mut buffer = Vec::new();
    let flush_interval = app.evaluation_flush_interval;
    let max_buffered = app.evaluation_event_queue_capacity.max(1);
    let batch_size = app.evaluation_flush_batch_size.max(1);

    loop {
        tokio::time::sleep(flush_interval).await;

        let dropped = app.evaluation_event_dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            warn!(
                "Dropped {} evaluation events due to full queue (capacity={})",
                dropped, max_buffered
            );
        }

        // Drain events up to the buffer limit
        while buffer.len() < max_buffered {
            match event_rx.try_recv() {
                Ok(event) => buffer.push(event),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        // Drop any additional events still waiting in the channel once the buffer is full
        let mut dropped_in_flush = 0u64;
        if buffer.len() >= max_buffered {
            loop {
                match event_rx.try_recv() {
                    Ok(_) => dropped_in_flush += 1,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
        }
        if dropped_in_flush > 0 {
            warn!(
                "Dropped {} evaluation events while draining (buffer full, capacity={})",
                dropped_in_flush, max_buffered
            );
        }

        if buffer.is_empty() {
            continue;
        }

        let mut to_send = std::mem::take(&mut buffer);
        let mut total_sent = 0usize;
        let mut total_processed = 0usize;
        let mut batches = 0usize;
        let mut failed = false;

        while !to_send.is_empty() {
            let chunk = if to_send.len() > batch_size {
                let rest = to_send.split_off(batch_size);
                let chunk = to_send;
                to_send = rest;
                chunk
            } else {
                let chunk = to_send;
                to_send = Vec::new();
                chunk
            };

            // Convert to proto format - pre-allocate capacity
            let mut proto_events = Vec::with_capacity(chunk.len());
            for event in chunk.iter() {
                let evaluated_at_unix_ms = event
                    .evaluated_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                // Convert EvaluateContext to proto Context entries
                // Pre-allocate: 1 for bucketingKey + number of attributes
                let mut proto_context =
                    Vec::with_capacity(1 + event.evaluation_context.attributes.len());

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
                        serde_json::Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
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
                    variant_value: event
                        .variant_value
                        .as_ref()
                        .map(|v| serde_json::to_string(v).unwrap_or_default())
                        .unwrap_or_default(),
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
                    total_sent += chunk.len();
                    total_processed += resp.processed_count as usize;
                    batches += 1;
                }
                Err(e) => {
                    error!("Failed to push evaluation events after retries: {}", e);
                    warn!(
                        "Will retry on next flush cycle ({}s)",
                        flush_interval.as_secs()
                    );
                    let mut requeue = chunk;
                    requeue.extend(to_send);
                    if requeue.len() > max_buffered {
                        let drop_count = requeue.len() - max_buffered;
                        buffer.extend(requeue.into_iter().skip(drop_count));
                        warn!(
                            "Dropped {} evaluation events while requeueing (buffer limit={})",
                            drop_count, max_buffered
                        );
                    } else {
                        buffer.extend(requeue);
                    }
                    failed = true;
                    break;
                }
            }
        }

        if !failed && total_sent > 0 {
            info!(
                "Successfully pushed {} evaluation events in {} batch(es) ({} processed)",
                total_sent, batches, total_processed
            );
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
    use std::sync::Arc;
    use tonic::transport::Endpoint;

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

        // Create AppState with the populated cache
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client =
            crate::pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

        let client_info_cache = Arc::new(crate::ClientInfoCache::new(
            std::time::Duration::from_secs(300),
        ));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(10);

        let app_state = crate::AppState {
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
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));

        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client =
            crate::pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

        let client_info_cache = Arc::new(crate::ClientInfoCache::new(
            std::time::Duration::from_secs(300),
        ));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(10);

        let app_state = crate::AppState {
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

    #[tokio::test]
    async fn test_handle_feature_update_populates_cache() {
        // Create test AppState
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(100));
        let client_info_cache = Arc::new(crate::ClientInfoCache::new(
            std::time::Duration::from_secs(300),
        ));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(10);
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client =
            crate::pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);

        let app_state = crate::AppState {
            mapped_cache: mapped_cache.clone(),
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
        };

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
}
