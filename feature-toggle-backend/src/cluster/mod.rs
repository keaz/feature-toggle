use std::{
    collections::{HashMap, HashSet},
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
    net::{TcpListener, TcpStream, lookup_host},
    select,
    sync::{Mutex, broadcast},
    task::{JoinHandle, JoinSet},
    time::sleep,
};
use uuid::Uuid;

use crate::{grpc::pb, logic::feature_evaluation::FeatureEvaluationEvent};

type WireMessage = Arc<Vec<u8>>;

fn default_refresh_ms() -> u64 {
    2_000
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum DiscoveryConfig {
    Static {
        peers: Vec<String>,
    },
    Dns {
        record: String,
        port: u16,
        #[serde(default = "default_refresh_ms")]
        refresh_ms: u64,
    },
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self::Static { peers: Vec::new() }
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
    /// Backwards-compatible static peer list.
    pub peers: Vec<String>,
    /// Dynamic discovery mechanism.
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
            peers: Vec::new(),
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
}

impl ClusterHandle {
    fn new(tasks: Vec<AbortOnDrop>) -> Self {
        Self { tasks }
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
    feature_updates_tx: broadcast::Sender<pb::FeatureUpdate>,
    evaluation_events_tx: broadcast::Sender<FeatureEvaluationEvent>,
    listen_addr: Option<std::net::SocketAddr>,
    feature_deduper: Arc<Deduper>,
    evaluation_deduper: Arc<Deduper>,
}

impl ClusterState {
    fn new(
        node_id: String,
        wire_tx: broadcast::Sender<WireMessage>,
        feature_updates_tx: broadcast::Sender<pb::FeatureUpdate>,
        evaluation_events_tx: broadcast::Sender<FeatureEvaluationEvent>,
        listen_addr: Option<std::net::SocketAddr>,
    ) -> Self {
        Self {
            node_id,
            wire_tx,
            feature_updates_tx,
            evaluation_events_tx,
            listen_addr,
            feature_deduper: Arc::new(Deduper::new(FEATURE_DEDUP_TTL, DEDUP_MAX_ENTRIES)),
            evaluation_deduper: Arc::new(Deduper::new(EVALUATION_DEDUP_TTL, DEDUP_MAX_ENTRIES)),
        }
    }
}

/// Small helper that drops duplicate message IDs with a TTL window.
struct Deduper {
    ttl: Duration,
    max_entries: usize,
    entries: Mutex<std::collections::HashMap<String, Instant>>,
}

impl Deduper {
    fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries,
            entries: Mutex::new(std::collections::HashMap::new()),
        }
    }

    async fn mark_seen(&self, key: &str) -> bool {
        let mut guard = self.entries.lock().await;
        let now = Instant::now();

        if let Some(ts) = guard.get_mut(key) {
            *ts = now;
            return false;
        }

        guard.insert(key.to_string(), now);
        if guard.len() > self.max_entries {
            guard.retain(|_, ts| now.saturating_duration_since(*ts) < self.ttl);
        }
        true
    }
}

/// Starts the cluster replication tasks and returns a guard to keep them alive.
pub fn start(
    cfg: &ClusterConfig,
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

    let (wire_tx, _) = broadcast::channel::<WireMessage>(WIRE_BUFFER);
    let state = Arc::new(ClusterState::new(
        node_id.clone(),
        wire_tx.clone(),
        feature_updates_tx.clone(),
        evaluation_events_tx.clone(),
        listen_addr,
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

    // Backwards-compatible static peers field.
    if !cfg.peers.is_empty() {
        tasks.extend(spawn_static_connectors(
            state.clone(),
            cfg.peers.clone(),
            reconnect_delay,
        ));
    }

    // Discovery-specific peers.
    match &cfg.discovery {
        DiscoveryConfig::Static { peers } => {
            if !peers.is_empty() {
                tasks.extend(spawn_static_connectors(
                    state.clone(),
                    peers.clone(),
                    reconnect_delay,
                ));
            }
        }
        DiscoveryConfig::Dns {
            record,
            port,
            refresh_ms,
        } => {
            let state_clone = state.clone();
            let record = record.clone();
            let port = *port;
            let refresh = Duration::from_millis((*refresh_ms).max(250));
            let handle = tokio::spawn(async move {
                run_dns_discovery(state_clone, record, port, refresh, reconnect_delay).await;
            });
            tasks.push(AbortOnDrop::new(handle));
        }
    }

    Some(ClusterHandle::new(tasks))
}

async fn run_listener(state: Arc<ClusterState>, listen_addr: String) {
    match TcpListener::bind(&listen_addr).await {
        Ok(listener) => {
            info!(
                "Cluster node {} listening for peers on {}",
                state.node_id, listen_addr
            );
            let mut join_set = JoinSet::new();
            loop {
                select! {
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, addr)) => {
                                let peer_label = format!("inbound:{}", addr);
                                let state_clone = state.clone();
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
                    join_res = join_set.join_next() => {
                        if let Some(Err(err)) = join_res {
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
    loop {
        match TcpStream::connect(&peer).await {
            Ok(stream) => {
                info!("Cluster node {} connected to peer {}", state.node_id, peer);
                if let Err(err) =
                    connection_loop(state.clone(), stream, format!("outbound:{}", peer)).await
                {
                    debug!("Cluster connection to {} closed with error: {}", peer, err);
                }
            }
            Err(err) => {
                debug!(
                    "Cluster node {} failed to connect to {}: {}",
                    state.node_id, peer, err
                );
            }
        }
        sleep(reconnect_delay).await;
    }
}

async fn connection_loop(
    state: Arc<ClusterState>,
    stream: TcpStream,
    label: String,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let mut outbound_rx = state.wire_tx.subscribe();

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
                    debug!(
                        "Cluster node {} received feature update {}",
                        state.node_id, update.message_id
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
    loop {
        match rx.recv().await {
            Ok(update) => {
                if update.message_id.is_empty() {
                    continue;
                }
                if !state.feature_deduper.mark_seen(&update.message_id).await {
                    continue;
                }

                let message = pb::ClusterMessage {
                    node_id: state.node_id.clone(),
                    payload: Some(pb::cluster_message::Payload::FeatureUpdate(update.clone())),
                };
                let bytes = Arc::new(message.encode_to_vec());
                let _ = wire_tx.send(bytes);
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

fn spawn_static_connectors<I>(
    state: Arc<ClusterState>,
    peers: I,
    reconnect_delay: Duration,
) -> Vec<AbortOnDrop>
where
    I: IntoIterator<Item = String>,
{
    let mut handles = Vec::new();
    let mut seen = HashSet::new();
    let self_addr = state.listen_addr;

    for peer in peers.into_iter() {
        if !seen.insert(peer.clone()) {
            continue;
        }
        if should_skip_peer(self_addr, &peer) {
            continue;
        }
        let state_clone = state.clone();
        let peer_clone = peer.clone();
        let handle = tokio::spawn(async move {
            run_peer_connector(state_clone, peer_clone, reconnect_delay).await;
        });
        handles.push(AbortOnDrop::new(handle));
    }

    handles
}

async fn run_dns_discovery(
    state: Arc<ClusterState>,
    record: String,
    port: u16,
    refresh: Duration,
    reconnect_delay: Duration,
) {
    info!(
        "Cluster node {} starting DNS discovery for {}:{}",
        state.node_id, record, port
    );

    let mut connectors: HashMap<String, AbortOnDrop> = HashMap::new();
    loop {
        match lookup_host((record.as_str(), port)).await {
            Ok(iter) => {
                let mut desired: HashSet<String> = HashSet::new();
                for addr in iter {
                    let peer = addr.to_string();
                    if should_skip_peer(state.listen_addr, &peer) {
                        continue;
                    }
                    desired.insert(peer.clone());
                    if connectors.contains_key(&peer) {
                        continue;
                    }
                    let state_clone = state.clone();
                    let peer_clone = peer.clone();
                    let handle = tokio::spawn(async move {
                        run_peer_connector(state_clone, peer_clone, reconnect_delay).await;
                    });
                    connectors.insert(peer, AbortOnDrop::new(handle));
                }

                // Abort obsolete connectors
                let mut removed = Vec::new();
                for key in connectors.keys() {
                    if !desired.contains(key) {
                        removed.push(key.clone());
                    }
                }
                for key in removed {
                    if let Some(handle) = connectors.remove(&key) {
                        // dropping AbortOnDrop triggers abort
                        drop(handle);
                    }
                }
            }
            Err(err) => {
                warn!(
                    "Cluster node {} failed DNS lookup for {}:{} - {}",
                    state.node_id, record, port, err
                );
            }
        }

        sleep(refresh).await;
    }
}

fn should_skip_peer(self_addr: Option<SocketAddr>, peer: &str) -> bool {
    if let Some(self_addr) = self_addr {
        if let Ok(peer_addr) = peer.parse::<SocketAddr>() {
            if self_addr.port() == peer_addr.port() {
                if self_addr.ip().is_unspecified() || self_addr.ip() == peer_addr.ip() {
                    return true;
                }
            }
        }
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
