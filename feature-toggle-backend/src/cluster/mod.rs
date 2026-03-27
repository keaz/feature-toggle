use std::{
    collections::HashMap,
    io::Cursor,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::TimeZone;
use log::{debug, error, info, warn};
use prost::Message;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    select,
    sync::{Mutex, broadcast},
    task::{JoinHandle, JoinSet},
    time::sleep,
};
use uuid::Uuid;

use crate::{grpc::pb, logic::feature_evaluation::FeatureEvaluationEvent};

pub mod db_discovery;
pub mod discovery;

type WireMessage = Arc<Vec<u8>>;

/// Database-backed cluster discovery configuration
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,
    #[serde(default = "default_stale_threshold")]
    pub stale_threshold_secs: u64,
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,
}

fn default_heartbeat_interval() -> u64 {
    30
}

fn default_stale_threshold() -> u64 {
    90
}

fn default_cleanup_interval() -> u64 {
    60
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_secs: 30,
            stale_threshold_secs: 90,
            cleanup_interval_secs: 60,
        }
    }
}

struct AbortOnDrop {
    handle: Option<JoinHandle<()>>,
}

impl AbortOnDrop {
    fn new(handle: JoinHandle<()>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn abort_and_wait(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Configuration for the intra-cluster replication channel.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ClusterConfig {
    /// Enables the cluster replication subsystem.
    pub enabled: bool,
    /// Address the node listens on for peer connections, e.g. "0.0.0.0:6000".
    pub listen_addr: String,
    /// Database-backed discovery configuration.
    pub discovery: DiscoveryConfig,
    /// Optional static identifier for the node. Defaults to a random UUID.
    pub node_id: Option<String>,
    /// Delay in milliseconds between reconnect attempts to peers.
    pub reconnect_delay_ms: u64,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: "0.0.0.0:6000".to_string(),
            discovery: DiscoveryConfig::default(),
            node_id: None,
            reconnect_delay_ms: 2_000,
        }
    }
}

const FEATURE_DEDUP_TTL: Duration = Duration::from_secs(600);
const EVALUATION_DEDUP_TTL: Duration = Duration::from_secs(600);
const DEDUP_MAX_ENTRIES: usize = 16_384;
const WIRE_BUFFER: usize = 1024;

/// Guard that keeps background tasks alive for the cluster runtime.
pub struct ClusterHandle {
    tasks: Vec<AbortOnDrop>,
    connection_ready_rx: tokio::sync::watch::Receiver<bool>,
}

impl ClusterHandle {
    fn new(
        tasks: Vec<AbortOnDrop>,
        connection_ready_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        Self {
            tasks,
            connection_ready_rx,
        }
    }

    /// Wait for at least one peer connection to be established.
    /// Useful in tests to ensure the cluster is ready before sending messages.
    pub async fn wait_for_connection_ready(&self) {
        let mut rx = self.connection_ready_rx.clone();
        // Wait until the value becomes true, with a timeout
        let timeout_duration = tokio::time::Duration::from_secs(10);
        if tokio::time::timeout(timeout_duration, async {
            rx.wait_for(|&ready| ready).await.ok();
        })
        .await
        .is_err()
        {
            warn!("Cluster connection did not become ready within 10 seconds");
        }
    }

    /// Stop cluster background tasks and wait for cancellation.
    pub async fn shutdown(mut self) {
        for task in self.tasks.drain(..) {
            task.abort_and_wait().await;
        }
    }
}

impl Drop for ClusterHandle {
    fn drop(&mut self) {
        self.tasks.clear();
    }
}

struct ClusterState {
    node_id: String,
    wire_tx: broadcast::Sender<WireMessage>,
    _wire_rx_keepalive: broadcast::Receiver<WireMessage>,
    feature_updates_tx: broadcast::Sender<pb::FeatureUpdate>,
    evaluation_events_tx: broadcast::Sender<FeatureEvaluationEvent>,
    listen_addr: Option<std::net::SocketAddr>,
    feature_deduper: Arc<Deduper>,
    evaluation_deduper: Arc<Deduper>,
    /// Tracks if at least one peer connection has been established
    connection_ready_tx: tokio::sync::watch::Sender<bool>,
}

impl ClusterState {
    fn new(
        node_id: String,
        wire_tx: broadcast::Sender<WireMessage>,
        wire_rx_keepalive: broadcast::Receiver<WireMessage>,
        feature_updates_tx: broadcast::Sender<pb::FeatureUpdate>,
        evaluation_events_tx: broadcast::Sender<FeatureEvaluationEvent>,
        listen_addr: Option<std::net::SocketAddr>,
        connection_ready_tx: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        Self {
            node_id,
            wire_tx,
            _wire_rx_keepalive: wire_rx_keepalive,
            feature_updates_tx,
            evaluation_events_tx,
            listen_addr,
            feature_deduper: Arc::new(Deduper::new(FEATURE_DEDUP_TTL, DEDUP_MAX_ENTRIES)),
            evaluation_deduper: Arc::new(Deduper::new(EVALUATION_DEDUP_TTL, DEDUP_MAX_ENTRIES)),
            connection_ready_tx,
        }
    }
}

/// Small helper that drops duplicate message IDs with a TTL window.
struct Deduper {
    ttl: Duration,
    max_entries: usize,
    entries: Mutex<DeduperEntries>,
}

struct DeduperEntries {
    by_key: std::collections::HashMap<String, Instant>,
    order: std::collections::VecDeque<(String, Instant)>,
}

impl DeduperEntries {
    fn new() -> Self {
        Self {
            by_key: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    fn prune_expired(&mut self, now: Instant, ttl: Duration) {
        loop {
            let Some((key, seen_at)) = self.order.front() else {
                break;
            };

            let is_current_entry =
                matches!(self.by_key.get(key), Some(current) if *current == *seen_at);
            if !is_current_entry {
                self.order.pop_front();
                continue;
            }

            if now.saturating_duration_since(*seen_at) < ttl {
                break;
            }

            let stale_key = key.clone();
            self.by_key.remove(&stale_key);
            self.order.pop_front();
        }
    }
}

impl Deduper {
    fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries,
            entries: Mutex::new(DeduperEntries::new()),
        }
    }

    async fn mark_seen(&self, key: &str) -> bool {
        let mut guard = self.entries.lock().await;
        let now = Instant::now();
        guard.prune_expired(now, self.ttl);

        if let Some(ts) = guard.by_key.get_mut(key) {
            *ts = now;
            guard.order.push_back((key.to_string(), now));
            return false;
        }

        let key_owned = key.to_string();
        guard.by_key.insert(key_owned.clone(), now);
        guard.order.push_back((key_owned, now));
        if guard.by_key.len() > self.max_entries {
            guard.prune_expired(now, self.ttl);
        }
        true
    }
}

/// Starts the cluster replication tasks and returns a guard to keep them alive.
///
/// # Arguments
/// * `cfg` - Cluster configuration
/// * `db_pool` - Optional database pool (required for Database discovery mode)
/// * `feature_updates_tx` - Channel for broadcasting feature updates
/// * `evaluation_events_tx` - Channel for broadcasting evaluation events
pub fn start(
    cfg: &ClusterConfig,
    db_pool: Option<sqlx::PgPool>,
    feature_updates_tx: broadcast::Sender<pb::FeatureUpdate>,
    evaluation_events_tx: broadcast::Sender<FeatureEvaluationEvent>,
) -> Option<ClusterHandle> {
    if !cfg.enabled {
        info!("Cluster replication disabled via configuration.");
        return None;
    }

    let node_id = cfg
        .node_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let reconnect_delay = Duration::from_millis(cfg.reconnect_delay_ms.max(250));
    let listen_addr = cfg.listen_addr.parse::<SocketAddr>().ok();

    let (wire_tx, wire_rx_keepalive) = broadcast::channel::<WireMessage>(WIRE_BUFFER);
    let (connection_ready_tx, connection_ready_rx) = tokio::sync::watch::channel(false);
    let state = Arc::new(ClusterState::new(
        node_id.clone(),
        wire_tx.clone(),
        wire_rx_keepalive,
        feature_updates_tx.clone(),
        evaluation_events_tx.clone(),
        listen_addr,
        connection_ready_tx,
    ));

    info!(
        "Starting cluster replication node {} listening on {}",
        node_id, cfg.listen_addr
    );

    let mut tasks: Vec<AbortOnDrop> = Vec::new();

    // Listener accepting inbound connections.
    {
        let state = state.clone();
        let listen_addr = cfg.listen_addr.clone();
        let handle = tokio::spawn(async move {
            run_listener(state, listen_addr).await;
        });
        tasks.push(AbortOnDrop::new(handle));
    }

    let wire_tx_for_features = wire_tx.clone();
    let wire_tx_for_evaluations = wire_tx.clone();

    // Local fan-out for feature updates.
    {
        let state = state.clone();
        let handle = tokio::spawn(async move {
            forward_feature_updates(state, wire_tx_for_features).await;
        });
        tasks.push(AbortOnDrop::new(handle));
    }

    // Local fan-out for evaluation events.
    {
        let state = state.clone();
        let handle = tokio::spawn(async move {
            forward_evaluation_events(state, wire_tx_for_evaluations).await;
        });
        tasks.push(AbortOnDrop::new(handle));
    }

    // Database-backed discovery
    let Some(pool) = db_pool else {
        error!("Database pool required for cluster discovery");
        return None;
    };
    let repo = db_discovery::ClusterNodeRepo::new(pool);

    let discovery_config = discovery::DbDiscoveryConfig {
        listen_addr: cfg.listen_addr.clone(),
        heartbeat_interval_secs: cfg.discovery.heartbeat_interval_secs,
        stale_threshold_secs: cfg.discovery.stale_threshold_secs,
        cleanup_interval_secs: cfg.discovery.cleanup_interval_secs,
    };

    let service =
        discovery::DbDiscoveryService::with_node_id(discovery_config, repo, node_id.clone());

    // Spawn task to start discovery service and handle peer events
    let state_clone = state.clone();
    let self_addr = state.listen_addr;
    let discovery_handle = tokio::spawn(async move {
        match service.start().await {
            Ok(mut handle) => {
                info!("Database discovery service started for node {}", node_id);

                // Track active peer connectors
                let mut peer_connectors: HashMap<String, AbortOnDrop> = HashMap::new();

                // Handle peer events
                while let Some(event) = handle.peer_events.recv().await {
                    match event {
                        discovery::PeerEvent::PeerAdded(peer_addr) => {
                            if should_skip_peer(self_addr, &peer_addr) {
                                info!("Skipping self peer {}", peer_addr);
                                continue;
                            }

                            if let std::collections::hash_map::Entry::Vacant(e) =
                                peer_connectors.entry(peer_addr.clone())
                            {
                                info!("Connecting to discovered peer: {}", peer_addr);
                                let state_for_peer = state_clone.clone();
                                let peer_clone = peer_addr;
                                let connector_handle = tokio::spawn(async move {
                                    run_peer_connector(state_for_peer, peer_clone, reconnect_delay)
                                        .await;
                                });
                                e.insert(AbortOnDrop::new(connector_handle));
                            }
                        }
                        discovery::PeerEvent::PeerRemoved(peer_addr) => {
                            if let Some(connector) = peer_connectors.remove(&peer_addr) {
                                info!("Removing connector for peer: {}", peer_addr);
                                drop(connector); // Abort the connector task
                            }
                        }
                    }
                }
                info!("Database discovery peer event handler shutting down");
            }
            Err(e) => {
                error!("Failed to start database discovery service: {}", e);
            }
        }
    });

    tasks.push(AbortOnDrop::new(discovery_handle));

    Some(ClusterHandle::new(tasks, connection_ready_rx))
}

async fn run_listener(state: Arc<ClusterState>, listen_addr: String) {
    match TcpListener::bind(&listen_addr).await {
        Ok(listener) => {
            info!(
                "Cluster node {} listening for peers on {}",
                state.node_id, listen_addr
            );
            let mut join_set = JoinSet::new();
            let mut has_notified = false;
            loop {
                if join_set.is_empty() {
                    match listener.accept().await {
                        Ok((stream, addr)) => {
                            let peer_label = format!("inbound:{}", addr);
                            let state_clone = state.clone();

                            // Notify that at least one connection is ready (do this once)
                            if !has_notified {
                                let _ = state.connection_ready_tx.send(true);
                                has_notified = true;
                            }

                            join_set.spawn(async move {
                                if let Err(err) =
                                    connection_loop(state_clone, stream, peer_label).await
                                {
                                    debug!("Inbound cluster connection ended: {}", err);
                                }
                            });
                        }
                        Err(err) => {
                            warn!("Cluster listener accept error: {}", err);
                            break;
                        }
                    }
                    continue;
                }

                select! {
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, addr)) => {
                                let peer_label = format!("inbound:{}", addr);
                                let state_clone = state.clone();

                                // Notify that at least one connection is ready (do this once)
                                if !has_notified {
                                    let _ = state.connection_ready_tx.send(true);
                                    has_notified = true;
                                }

                                join_set.spawn(async move {
                                    if let Err(err) = connection_loop(state_clone, stream, peer_label).await {
                                        debug!("Inbound cluster connection ended: {}", err);
                                    }
                                });
                            }
                            Err(err) => {
                                warn!("Cluster listener accept error: {}", err);
                                break;
                            }
                        }
                    }
                    Some(join_res) = join_set.join_next() => {
                        if let Err(err) = join_res {
                            debug!("Cluster connection task aborted: {}", err);
                        }
                    }
                }
            }
            join_set.abort_all();
            while let Some(res) = join_set.join_next().await {
                if let Err(err) = res {
                    debug!("Cluster connection task aborted during shutdown: {}", err);
                }
            }
        }
        Err(err) => {
            error!(
                "Cluster node {} failed to bind {}: {}",
                state.node_id, listen_addr, err
            );
        }
    }
}

async fn run_peer_connector(state: Arc<ClusterState>, peer: String, reconnect_delay: Duration) {
    // Subscribe ONCE before entering the loop to ensure we're ready to receive messages
    // as soon as forward_feature_updates starts sending them
    let mut wire_rx = state.wire_tx.subscribe();
    let mut has_notified = false;

    info!(
        "Cluster node {} starting peer connector for {}",
        state.node_id, peer
    );
    loop {
        debug!(
            "Cluster node {} attempting to connect to peer {}...",
            state.node_id, peer
        );
        match TcpStream::connect(&peer).await {
            Ok(stream) => {
                info!(
                    "Cluster node {} successfully connected to peer {}",
                    state.node_id, peer
                );

                // Notify that at least one connection is ready (do this once)
                if !has_notified {
                    info!("Cluster node {} notifying connection ready", state.node_id);
                    let _ = state.connection_ready_tx.send(true);
                    has_notified = true;
                }

                if let Err(err) = connection_loop_with_rx(
                    state.clone(),
                    stream,
                    format!("outbound:{}", peer),
                    &mut wire_rx,
                )
                .await
                {
                    info!("Cluster connection to {} closed with error: {}", peer, err);
                }
            }
            Err(err) => {
                warn!(
                    "Cluster node {} failed to connect to {}: {}",
                    state.node_id, peer, err
                );
            }
        }
        debug!(
            "Cluster node {} sleeping {}ms before reconnect to {}",
            state.node_id,
            reconnect_delay.as_millis(),
            peer
        );
        sleep(reconnect_delay).await;

        // After sleep, drain any messages that arrived while disconnected to prevent lag errors
        while wire_rx.try_recv().is_ok() {
            // Discard buffered messages from disconnect period
        }
    }
}

async fn connection_loop(
    state: Arc<ClusterState>,
    stream: TcpStream,
    label: String,
) -> std::io::Result<()> {
    // For inbound connections, create a new subscription
    let mut outbound_rx = state.wire_tx.subscribe();
    connection_loop_with_rx(state, stream, label, &mut outbound_rx).await
}

async fn connection_loop_with_rx(
    state: Arc<ClusterState>,
    stream: TcpStream,
    label: String,
    outbound_rx: &mut broadcast::Receiver<WireMessage>,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();

    loop {
        select! {
            read_res = read_frame(&mut reader) => {
                match read_res {
                    Ok(Some(bytes)) => {
                        let arc = Arc::new(bytes);
                        handle_incoming(state.clone(), arc.clone()).await;
                    }
                    Ok(None) => {
                        debug!("Cluster connection {} closed by peer", label);
                        break;
                    }
                    Err(err) => {
                        debug!("Cluster read error on {}: {}", label, err);
                        break;
                    }
                }
            }
            outbound = outbound_rx.recv() => {
                match outbound {
                    Ok(msg) => {
                        if let Err(err) = write_frame(&mut writer, &msg).await {
                            debug!("Cluster write error on {}: {}", label, err);
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        debug!("Cluster connection {} lagged {} messages", label, skipped);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_incoming(state: Arc<ClusterState>, bytes: WireMessage) {
    match decode_cluster_message(&bytes) {
        Ok(message) => match message.payload {
            Some(pb::cluster_message::Payload::FeatureUpdate(update)) => {
                if update.message_id.is_empty() {
                    return;
                }
                if state.feature_deduper.mark_seen(&update.message_id).await {
                    info!(
                        "Cluster node {} received feature update {} for key '{}', broadcasting to {} local subscribers",
                        state.node_id,
                        update.message_id,
                        update
                            .feature
                            .as_ref()
                            .map(|f| f.key.as_str())
                            .unwrap_or(&update.feature_key),
                        state.feature_updates_tx.receiver_count()
                    );
                    let _ = state.feature_updates_tx.send(update.clone());
                    // Relay to other peers to ensure propagation in sparse topologies.
                    let _ = state.wire_tx.send(bytes);
                }
            }
            Some(pb::cluster_message::Payload::EvaluationEvent(event)) => {
                if event.event_id.is_empty() {
                    return;
                }
                if state.evaluation_deduper.mark_seen(&event.event_id).await {
                    if let Some(local_event) = to_logic_event(event.clone()) {
                        debug!(
                            "Cluster node {} received evaluation event {}",
                            state.node_id, event.event_id
                        );
                        let _ = state.evaluation_events_tx.send(local_event);
                        let _ = state.wire_tx.send(bytes);
                    } else {
                        warn!(
                            "Cluster node {} dropped malformed evaluation event {}",
                            state.node_id, event.event_id
                        );
                    }
                }
            }
            None => {}
        },
        Err(err) => warn!(
            "Cluster node {} failed to decode message: {}",
            state.node_id, err
        ),
    }
}

async fn forward_feature_updates(
    state: Arc<ClusterState>,
    wire_tx: broadcast::Sender<WireMessage>,
) {
    let mut rx = state.feature_updates_tx.subscribe();
    info!(
        "Cluster node {} starting forward_feature_updates task",
        state.node_id
    );
    loop {
        match rx.recv().await {
            Ok(update) => {
                debug!(
                    "Cluster node {} received feature update message_id={}",
                    state.node_id, update.message_id
                );
                if update.message_id.is_empty() {
                    debug!(
                        "Cluster node {} skipping update with empty message_id",
                        state.node_id
                    );
                    continue;
                }
                if !state.feature_deduper.mark_seen(&update.message_id).await {
                    debug!(
                        "Cluster node {} skipping duplicate update message_id={}",
                        state.node_id, update.message_id
                    );
                    continue;
                }

                info!(
                    "Cluster node {} forwarding feature update message_id={} to {} peers",
                    state.node_id,
                    update.message_id,
                    wire_tx.receiver_count()
                );
                let message = pb::ClusterMessage {
                    node_id: state.node_id.clone(),
                    payload: Some(pb::cluster_message::Payload::FeatureUpdate(update.clone())),
                };
                let bytes = Arc::new(message.encode_to_vec());
                match wire_tx.send(bytes) {
                    Ok(count) => {
                        info!("Cluster node {} sent to {} receivers", state.node_id, count)
                    }
                    Err(_) => warn!(
                        "Cluster node {} failed to send (no receivers)",
                        state.node_id
                    ),
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    "Cluster feature forwarder lagged {} messages; consider increasing buffer.",
                    skipped
                );
            }
        }
    }
}

async fn forward_evaluation_events(
    state: Arc<ClusterState>,
    wire_tx: broadcast::Sender<WireMessage>,
) {
    let mut rx = state.evaluation_events_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let event_id = event.event_id.to_string();
                if !state.evaluation_deduper.mark_seen(&event_id).await {
                    continue;
                }

                let cluster_event = pb::ClusterEvaluationEvent {
                    event_id,
                    feature_key: event.feature_key.clone(),
                    environment_id: event.environment_id.clone(),
                    client_id: event.client_id.to_string(),
                    evaluated_at_unix_ms: event.evaluated_at.timestamp_millis(),
                    evaluation_result: event.evaluation_result,
                    prior_assignment: event.prior_assignment,
                    user_context: event.user_context.clone().unwrap_or_default(),
                };

                let message = pb::ClusterMessage {
                    node_id: state.node_id.clone(),
                    payload: Some(pb::cluster_message::Payload::EvaluationEvent(cluster_event)),
                };
                let bytes = Arc::new(message.encode_to_vec());
                let _ = wire_tx.send(bytes);
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    "Cluster evaluation forwarder lagged {} messages; consider increasing buffer.",
                    skipped
                );
            }
        }
    }
}

fn to_logic_event(event: pb::ClusterEvaluationEvent) -> Option<FeatureEvaluationEvent> {
    let event_id = Uuid::parse_str(&event.event_id).ok()?;
    let client_id = Uuid::parse_str(&event.client_id).ok()?;
    let evaluated_at = chrono::Utc
        .timestamp_millis_opt(event.evaluated_at_unix_ms)
        .single()?;

    Some(FeatureEvaluationEvent {
        event_id,
        feature_key: event.feature_key,
        environment_id: event.environment_id,
        client_id,
        evaluated_at,
        evaluation_result: event.evaluation_result,
        prior_assignment: event.prior_assignment,
        user_context: if event.user_context.is_empty() {
            None
        } else {
            Some(event.user_context)
        },
    })
}

fn decode_cluster_message(bytes: &WireMessage) -> Result<pb::ClusterMessage, prost::DecodeError> {
    let slice: &[u8] = bytes.as_ref();
    let mut cursor = Cursor::new(slice);
    pb::ClusterMessage::decode(&mut cursor)
}

fn should_skip_peer(self_addr: Option<SocketAddr>, peer: &str) -> bool {
    if let Some(self_addr) = self_addr
        && let Ok(peer_addr) = peer.parse::<SocketAddr>()
        && self_addr.port() == peer_addr.port()
        && (self_addr.ip().is_unspecified() || self_addr.ip() == peer_addr.ip())
    {
        return true;
    }
    false
}

async fn read_frame(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

async fn write_frame(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    payload: &WireMessage,
) -> std::io::Result<()> {
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload.as_ref()).await?;
    writer.flush().await
}
