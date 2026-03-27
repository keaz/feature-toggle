use super::{AppState, build_endpoint, pb};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_stream::StreamExt;
use tracing::{error, info, warn};

/// Send the initial subscription payload for a stream connection.
pub(crate) async fn send_initial_subscribe(
    tx: &tokio::sync::mpsc::Sender<pb::StreamRequest>,
    app: &AppState,
    force_full_snapshot: bool,
) {
    // Under normal reconnects, we resubscribe to the cached key set so the
    // backend can continue sending the subset we already care about. After a
    // lag signal, we intentionally request a full snapshot to converge stale
    // local state, including deletes that may have been missed.
    let cached_keys = if force_full_snapshot {
        tracing::info!("Subscribing with full snapshot resync after lag");
        Vec::new()
    } else {
        let keys = app.mapped_cache.get_all_keys().await;
        tracing::info!("Subscribing with {} cached feature keys", keys.len());
        keys
    };

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

pub(crate) async fn prepare_for_full_resync(app: &AppState) {
    warn!("Clearing local edge state before full snapshot resync");
    app.connected.store(false, Ordering::Relaxed);
    app.mapped_cache.clear_all().await;
    app.purge_all_assignments();
}

/// Spawn a background task to send periodic heartbeats.
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

/// Open a streaming gRPC call for feature updates.
async fn open_streaming_call(
    mut client: pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
    rx: tokio::sync::mpsc::Receiver<pb::StreamRequest>,
) -> Result<tonic::Response<tonic::Streaming<pb::FeatureUpdate>>, tonic::Status> {
    use tokio_stream::wrappers::ReceiverStream;
    let req_stream = ReceiverStream::new(rx);
    client.stream_updates(req_stream).await
}

/// Apply a single backend stream update to local caches. Returning `true`
/// tells the caller to tear down the stream and reconnect with a full resync.
pub(crate) async fn handle_feature_update(app: &AppState, update: pb::FeatureUpdate) -> bool {
    use pb::feature_update::Action;
    match update.action {
        x if x == Action::Upsert as i32 || x == Action::Snapshot as i32 => {
            if let Some(f) = update.feature {
                let feature_id = f.id.clone();
                let dependency_ids = f
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.depends_on_id.clone())
                    .collect::<Vec<_>>();

                let engine_feature = std::sync::Arc::new(crate::handlers::map_proto_to_engine(&f));
                app.mapped_cache
                    .insert_with_dependencies(engine_feature, dependency_ids)
                    .await;

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
        x if x == Action::Error as i32 => {
            if update.error == "lagged" {
                warn!("Received lagged marker from backend stream; forcing reconnect");
                return true;
            }
            if !update.error.is_empty() {
                warn!("Received backend stream error marker: {}", update.error);
            }
        }
        _ => {}
    }
    false
}

/// Maintain the long-lived backend update stream. Lag markers force a full
/// snapshot resubscribe so deletes and missed updates converge deterministically.
pub async fn run_stream_task(app: AppState, grpc_addr: String) {
    let mut retry_delay = app.retry_config.stream_initial_delay();
    let max_retry_delay = app.retry_config.stream_max_delay();
    let mut force_full_resync = false;

    loop {
        app.connected.store(false, Ordering::Relaxed);

        if force_full_resync {
            prepare_for_full_resync(&app).await;
        }

        let endpoint = build_endpoint(&grpc_addr);
        match endpoint.connect().await {
            Ok(channel) => {
                let client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
                info!("Connected to backend gRPC {}", &grpc_addr);

                retry_delay = app.retry_config.stream_initial_delay();

                let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
                send_initial_subscribe(&tx, &app, force_full_resync).await;
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
                force_full_resync = false;

                app.connected.store(true, Ordering::Relaxed);
                info!("Stream connection established, receiving updates");
                let mut inbound = response.into_inner();

                while let Some(msg) = inbound.next().await {
                    match msg {
                        Ok(update) => {
                            if handle_feature_update(&app, update).await {
                                force_full_resync = true;
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Stream error: {}", e);
                            break;
                        }
                    }
                }

                app.connected.store(false, Ordering::Relaxed);
                warn!("Stream connection closed, will retry in {:?}", retry_delay);
            }
            Err(e) => {
                error!("Failed to connect to backend gRPC {}: {}", &grpc_addr, e);
                app.connected.store(false, Ordering::Relaxed);
                warn!("Retrying connection in {:?}", retry_delay);
            }
        }

        tokio::time::sleep(retry_delay).await;
        retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
    }
}
