use super::{AppState, backend_client, build_endpoint, pb};
use crate::config::GrpcConfig;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_stream::StreamExt;
use tracing::{debug, error, info, warn};

/// Send the initial subscription payload for a stream connection and return
/// the subscribed feature keys.
///
/// Every connection subscribes with empty keys, which means all of the team's
/// features and a full snapshot. Subscribing with only the cached keys would
/// hide flags created while the edge was disconnected.
pub(crate) async fn send_initial_subscribe(
    tx: &tokio::sync::mpsc::Sender<pb::StreamRequest>,
    app: &AppState,
) -> Vec<String> {
    tracing::info!("Subscribing to all team features (full snapshot)");

    let feature_keys = Vec::new();
    let subscribe = pb::SubscribeRequest {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
        feature_keys: feature_keys.clone(),
        environment_id: "".into(),
    };
    let initial = pb::StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(subscribe)),
    };
    let _ = tx.send(initial).await;
    feature_keys
}

pub(crate) async fn prepare_for_full_resync(app: &AppState) {
    warn!("Clearing local edge state before full snapshot resync");
    app.connected.store(false, Ordering::Relaxed);
    app.mapped_cache.clear_all().await;
    app.purge_all_assignments();
}

/// Spawn a background task to send periodic heartbeats. The task exits once
/// the stream's receiver is dropped.
fn spawn_heartbeat(
    tx: tokio::sync::mpsc::Sender<pb::StreamRequest>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let sent = tx
                .send(pb::StreamRequest {
                    payload: Some(pb::stream_request::Payload::Heartbeat(pb::Heartbeat {
                        ts_unix_ms: ts,
                    })),
                })
                .await;
            // The streaming call owns the receiver; once it is dropped the
            // connection is gone and this task must end with it.
            if sent.is_err() {
                break;
            }
        }
    })
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

/// Learn the configured client's team before consuming the stream, so
/// `handle_feature_update` can drop other teams' features. One attempt per
/// connection; on failure the stream loop retries with its own backoff.
async fn ensure_edge_team_id(
    app: &AppState,
    client: &mut pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
) -> bool {
    if app.edge_team_id().is_some() {
        return true;
    }
    let request = pb::GetClientInfoRequest {
        client_id: app.client_id.clone(),
        client_secret: app.client_secret.clone(),
    };
    match client.get_client_info(tonic::Request::new(request)).await {
        Ok(response) => {
            app.record_edge_team_id(&response.into_inner().team_id);
            if app.edge_team_id().is_none() {
                error!("GetClientInfo returned no team for the configured client");
                return false;
            }
            true
        }
        Err(status) => {
            error!(
                "Failed to resolve the configured client's team: code={:?} msg={}",
                status.code(),
                status.message()
            );
            false
        }
    }
}

/// Apply a single backend stream update to local caches. Returning `true`
/// tells the caller to tear down the stream and reconnect with a full resync.
pub(crate) async fn handle_feature_update(app: &AppState, update: pb::FeatureUpdate) -> bool {
    use pb::feature_update::Action;
    match update.action {
        x if x == Action::Upsert as i32 || x == Action::Snapshot as i32 => {
            if let Some(f) = update.feature {
                // The edge serves one team. Drop other teams' features, and
                // everything while the team is unknown (fail closed).
                let Some(edge_team_id) = app.edge_team_id() else {
                    warn!(
                        "Ignoring update for feature '{}': edge team is not known yet",
                        f.key
                    );
                    return false;
                };
                if f.team_id != edge_team_id {
                    debug!(
                        "Ignoring update for feature '{}' owned by another team",
                        f.key
                    );
                    return false;
                }

                let feature_id = f.id.clone();
                let dependency_ids = f
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.depends_on_id.clone())
                    .collect::<Vec<_>>();

                let engine_feature = std::sync::Arc::new(crate::handlers::map_proto_to_engine(&f));
                let enabled = engine_feature.enabled;
                app.mapped_cache
                    .insert_with_dependencies(&f.team_id, engine_feature, dependency_ids)
                    .await;

                // Cached results always go, so the new config applies. Pending
                // assignments are facts already served to users: keep them while
                // the feature stays enabled (P01 step 3), so a reconnect snapshot
                // or an unrelated edit no longer loses up to one flush interval
                // of them. A disabled feature drops them, as before.
                if enabled {
                    app.clear_cached_assignments_for_feature(&feature_id);
                } else {
                    app.purge_assignments_for_feature(&feature_id).await;
                }
            }
        }
        x if x == Action::Delete as i32 => {
            // `delete_by_key` returns no id when the key was renamed and the
            // feature lives on under its new key. That Upsert already purged
            // the feature's assignments; later ones are still valid.
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

/// Tracks the initial snapshot of one stream connection, so that cached keys
/// the snapshot did not contain can be swept once the backend marks it
/// complete (`SNAPSHOT_COMPLETE`).
///
/// A tracker lives only as long as its connection: if the stream drops before
/// the marker, the partial snapshot is discarded and nothing is swept. A
/// backend that never sends the marker never triggers a sweep either.
#[derive(Debug, Default)]
pub(crate) struct SnapshotSweep {
    /// Keys the subscription asked for. `None` means all team features, so
    /// every cached key of the edge's team is in scope.
    scope: Option<HashSet<String>>,
    /// Own-team keys received in Snapshot messages. `None` once the snapshot
    /// is complete.
    seen: Option<HashSet<String>>,
}

impl SnapshotSweep {
    /// Start tracking the snapshot of a subscription to `feature_keys`.
    pub(crate) fn start(feature_keys: &[String]) -> Self {
        Self {
            scope: (!feature_keys.is_empty()).then(|| feature_keys.iter().cloned().collect()),
            seen: Some(HashSet::new()),
        }
    }

    fn record(&mut self, key: &str) {
        if let Some(seen) = self.seen.as_mut() {
            seen.insert(key.to_string());
        }
    }

    /// Whether a cached key must go: in scope and not in the snapshot.
    /// `None` when no snapshot is in progress.
    fn take_stale_filter(&mut self) -> Option<impl Fn(&str) -> bool + use<>> {
        let seen = self.seen.take()?;
        let scope = self.scope.take();
        Some(move |key: &str| {
            !seen.contains(key) && scope.as_ref().is_none_or(|scope| scope.contains(key))
        })
    }
}

/// Apply one message of a stream connection, tracking its initial snapshot in
/// `sweep`. Returning `true` asks for a reconnect with a full resync.
pub(crate) async fn handle_stream_update(
    app: &AppState,
    sweep: &mut SnapshotSweep,
    update: pb::FeatureUpdate,
) -> bool {
    use pb::feature_update::Action;
    if update.action == Action::SnapshotComplete as i32 {
        sweep_unseen_features(app, sweep).await;
        return false;
    }
    if update.action == Action::Snapshot as i32
        && let (Some(feature), Some(edge_team_id)) = (update.feature.as_ref(), app.edge_team_id())
        && feature.team_id == edge_team_id
    {
        sweep.record(&feature.key);
    }
    handle_feature_update(app, update).await
}

/// Drop cached features of the edge's team that the completed snapshot did
/// not contain (renamed or removed while the edge was disconnected), and
/// purge their assignments like a Delete does.
async fn sweep_unseen_features(app: &AppState, sweep: &mut SnapshotSweep) {
    let Some(is_stale) = sweep.take_stale_filter() else {
        debug!("Ignoring snapshot-complete marker: no snapshot in progress");
        return;
    };
    let Some(edge_team_id) = app.edge_team_id() else {
        return;
    };
    let stale_keys = app
        .mapped_cache
        .keys_for_team(edge_team_id)
        .into_iter()
        .filter(|key| is_stale(key))
        .collect::<Vec<_>>();
    for key in &stale_keys {
        if let Some(feature_id) = app.mapped_cache.delete_by_key(key).await {
            app.purge_assignments_for_feature(&feature_id).await;
        }
    }
    info!(
        "Snapshot complete: removed {} cached feature(s) missing from it",
        stale_keys.len()
    );
}

/// Maintain the long-lived backend update stream. Lag markers force a full
/// snapshot resubscribe so deletes and missed updates converge deterministically.
pub async fn run_stream_task(app: AppState, grpc_addr: String, grpc_config: GrpcConfig) {
    let mut retry_delay = app.retry_config.stream_initial_delay();
    let max_retry_delay = app.retry_config.stream_max_delay();
    let mut force_full_resync = false;

    loop {
        app.connected.store(false, Ordering::Relaxed);

        if force_full_resync {
            prepare_for_full_resync(&app).await;
        }

        let endpoint = build_endpoint(&grpc_addr, &grpc_config);
        match endpoint.connect().await {
            Ok(channel) => {
                let mut client = backend_client(channel, &grpc_config);
                info!("Connected to backend gRPC {}", &grpc_addr);

                retry_delay = app.retry_config.stream_initial_delay();

                if !ensure_edge_team_id(&app, &mut client).await {
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
                    continue;
                }

                let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
                let subscribed_keys = send_initial_subscribe(&tx, &app).await;
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
                // Per connection: a stream that drops before its snapshot
                // completes discards the partial snapshot and sweeps nothing.
                let mut sweep = SnapshotSweep::start(&subscribed_keys);

                while let Some(msg) = inbound.next().await {
                    match msg {
                        Ok(update) => {
                            if handle_stream_update(&app, &mut sweep, update).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn heartbeat_task_exits_after_receiver_is_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
        let handle = spawn_heartbeat(tx);
        drop(rx);

        // Paused clock: the runtime auto-advances through the heartbeat's
        // 30 s timer before this 31 s sleep completes.
        tokio::time::sleep(Duration::from_secs(31)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        assert!(
            handle.is_finished(),
            "heartbeat task must stop once the stream receiver is gone"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_task_keeps_sending_while_receiver_is_alive() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
        let handle = spawn_heartbeat(tx);

        tokio::time::sleep(Duration::from_secs(31)).await;
        let msg = rx.recv().await.expect("heartbeat expected");
        assert!(matches!(
            msg.payload,
            Some(pb::stream_request::Payload::Heartbeat(_))
        ));
        assert!(!handle.is_finished());
        handle.abort();
    }
}
