//! Database-backed cluster discovery service
//!
//! This module provides the discovery service that uses the database
//! to discover peers, maintain heartbeats, and detect stale nodes.

use super::db_discovery::ClusterNodeRepo;
use log::{debug, error, info};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::interval;

/// Peer event notifications
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    /// A new peer was discovered
    PeerAdded(String),
    /// A peer was removed (stale or deregistered)
    PeerRemoved(String),
}

/// Configuration for database-backed discovery
#[derive(Debug, Clone)]
pub struct DbDiscoveryConfig {
    /// Address where this node listens for cluster connections
    pub listen_addr: String,
    /// Interval between heartbeat updates (seconds)
    pub heartbeat_interval_secs: u64,
    /// Threshold for considering a node stale (seconds)
    pub stale_threshold_secs: u64,
    /// Interval between cleanup runs (seconds)
    pub cleanup_interval_secs: u64,
}

impl Default for DbDiscoveryConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:50052".to_string(),
            heartbeat_interval_secs: 30,
            stale_threshold_secs: 90,
            cleanup_interval_secs: 60,
        }
    }
}

/// Handle to the running discovery service
pub struct DbDiscoveryHandle {
    /// Channel to receive peer events (new/removed peers)
    pub peer_events: mpsc::Receiver<PeerEvent>,
    /// Handle to the background tasks
    tasks: Vec<JoinHandle<()>>,
    /// Channel to signal shutdown
    shutdown_tx: broadcast::Sender<()>,
    /// Repository for fallback deregistration
    repo: ClusterNodeRepo,
    /// Node ID for deregistration
    node_id: String,
}

impl DbDiscoveryHandle {
    /// Shutdown the discovery service gracefully
    pub async fn shutdown(mut self) {
        // Signal shutdown to all tasks
        let _ = self.shutdown_tx.send(());

        // Wait for all tasks to complete
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }

        // Best-effort fallback in case cleanup task exited before deregistration.
        if let Err(err) = self.repo.deregister_node(&self.node_id).await {
            debug!(
                "Discovery fallback deregistration failed for node {}: {}",
                self.node_id, err
            );
        }
    }
}

impl Drop for DbDiscoveryHandle {
    fn drop(&mut self) {
        // Signal shutdown so background tasks can exit.
        let _ = self.shutdown_tx.send(());

        // Ensure tasks do not outlive the owner when the handle is dropped abruptly.
        for task in self.tasks.drain(..) {
            task.abort();
        }

        // Best-effort immediate cleanup for abrupt drops where graceful shutdown wasn't awaited.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let repo = self.repo.clone();
            let node_id = self.node_id.clone();
            runtime.spawn(async move {
                let _ = repo.deregister_node(&node_id).await;
            });
        }
    }
}

/// Database-backed discovery service
pub struct DbDiscoveryService {
    config: DbDiscoveryConfig,
    repo: ClusterNodeRepo,
    node_id: String,
}

impl DbDiscoveryService {
    /// Create a new discovery service with a dynamically generated node ID
    pub fn new(config: DbDiscoveryConfig, repo: ClusterNodeRepo) -> Self {
        Self::with_node_id(config, repo, uuid::Uuid::new_v4().to_string())
    }

    /// Create a new discovery service with a fixed node ID.
    pub fn with_node_id(config: DbDiscoveryConfig, repo: ClusterNodeRepo, node_id: String) -> Self {
        Self {
            config,
            repo,
            node_id,
        }
    }

    /// Start the discovery service
    ///
    /// This will:
    /// 1. Register this node in the database
    /// 2. Start heartbeat loop
    /// 3. Start peer discovery loop
    /// 4. Start cleanup loop
    ///
    /// Returns a handle to control the service and receive peer events.
    pub async fn start(self) -> Result<DbDiscoveryHandle, super::db_discovery::ClusterDbError> {
        let node_id = self.node_id.clone();
        let listen_addr = self.config.listen_addr.clone();

        // Register this node
        info!("Registering cluster node {} at {}", node_id, listen_addr);
        self.repo.register_node(&node_id, &listen_addr).await?;

        let (peer_event_tx, peer_event_rx) = mpsc::channel::<PeerEvent>(100);
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        let mut tasks = Vec::new();

        // Heartbeat task
        {
            let repo = self.repo.clone();
            let node_id = node_id.clone();
            let interval_secs = self.config.heartbeat_interval_secs;
            let mut shutdown_rx = shutdown_tx.subscribe();

            let handle = tokio::spawn(async move {
                let mut ticker = interval(Duration::from_secs(interval_secs));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            debug!("Sending heartbeat for node {}", node_id);
                            if let Err(e) = repo.heartbeat(&node_id).await {
                                error!("Failed to send heartbeat for node {}: {}", node_id, e);
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Heartbeat task shutting down for node {}", node_id);
                            break;
                        }
                    }
                }
            });
            tasks.push(handle);
        }

        // Peer discovery task
        {
            let repo = self.repo.clone();
            let node_id = node_id.clone();
            let heartbeat_interval = self.config.heartbeat_interval_secs;
            let stale_threshold = self.config.stale_threshold_secs;
            let peer_tx = peer_event_tx.clone();
            let mut shutdown_rx = shutdown_tx.subscribe();

            let handle = tokio::spawn(async move {
                let mut known_peers: HashSet<String> = HashSet::new();
                let mut ticker = interval(Duration::from_secs(heartbeat_interval));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            match repo.get_active_peers(&node_id, stale_threshold).await {
                                Ok(current_peers) => {
                                    let current_set: HashSet<String> = current_peers.into_iter().collect();

                                    // Find new peers
                                    for peer in current_set.iter() {
                                        if !known_peers.contains(peer) {
                                            info!("Discovered new peer: {}", peer);
                                            known_peers.insert(peer.clone());
                                            let _ = peer_tx.send(PeerEvent::PeerAdded(peer.clone())).await;
                                        }
                                    }

                                    // Find removed peers
                                    let removed: Vec<String> = known_peers
                                        .difference(&current_set)
                                        .cloned()
                                        .collect();

                                    for peer in removed {
                                        info!("Peer removed: {}", peer);
                                        known_peers.remove(&peer);
                                        let _ = peer_tx.send(PeerEvent::PeerRemoved(peer.clone())).await;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to discover peers for node {}: {}", node_id, e);
                                }
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Peer discovery task shutting down for node {}", node_id);
                            break;
                        }
                    }
                }
            });
            tasks.push(handle);
        }

        // Cleanup task
        {
            let repo = self.repo.clone();
            let node_id = node_id.clone();
            let cleanup_interval = self.config.cleanup_interval_secs;
            let stale_threshold = self.config.stale_threshold_secs;
            let mut shutdown_rx = shutdown_tx.subscribe();

            let handle = tokio::spawn(async move {
                let mut ticker = interval(Duration::from_secs(cleanup_interval));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            debug!("Running cleanup for stale nodes");
                            match repo.cleanup_stale_nodes(stale_threshold).await {
                                Ok(removed) => {
                                    if removed > 0 {
                                        info!("Cleaned up {} stale node(s)", removed);
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to cleanup stale nodes: {}", e);
                                }
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Cleanup task shutting down");
                            // Deregister this node on shutdown
                            if let Err(e) = repo.deregister_node(&node_id).await {
                                error!("Failed to deregister node {} on shutdown: {}", node_id, e);
                            } else {
                                info!("Deregistered node {} on shutdown", node_id);
                            }
                            break;
                        }
                    }
                }
            });
            tasks.push(handle);
        }

        Ok(DbDiscoveryHandle {
            peer_events: peer_event_rx,
            tasks,
            shutdown_tx,
            repo: self.repo.clone(),
            node_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::db_discovery::{ClusterNode, ClusterNodeRepo};
    use sqlx::postgres::PgPoolOptions;
    use std::env;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    async fn wait_for_node(
        repo: &ClusterNodeRepo,
        node_id: &str,
        timeout_secs: u64,
    ) -> Option<ClusterNode> {
        timeout(Duration::from_secs(timeout_secs), async {
            loop {
                if let Some(node) = repo.get_node(node_id).await.expect("node lookup failed") {
                    break Some(node);
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .ok()
        .flatten()
    }

    async fn wait_for_active_peer(
        repo: &ClusterNodeRepo,
        node_id: &str,
        peer_addr: &str,
        timeout_secs: u64,
    ) -> bool {
        timeout(Duration::from_secs(timeout_secs), async {
            loop {
                let peers = repo
                    .get_active_peers(node_id, 30)
                    .await
                    .expect("peer query failed");
                if peers.iter().any(|peer| peer == peer_addr) {
                    break true;
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap_or(false)
    }

    async fn wait_for_peer_event(
        rx: &mut tokio::sync::mpsc::Receiver<PeerEvent>,
        expected: PeerEvent,
        timeout_secs: u64,
    ) -> Option<PeerEvent> {
        timeout(Duration::from_secs(timeout_secs), async {
            loop {
                match rx.recv().await {
                    Some(event) if event == expected => break Some(event),
                    Some(_) => continue,
                    None => break None,
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    async fn setup_test_repo() -> ClusterNodeRepo {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");

        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("Failed to connect to test database");

        // Isolate tests from any previously registered cluster nodes.
        sqlx::query("DELETE FROM cluster_nodes")
            .execute(&pool)
            .await
            .expect("Failed to clean up test data");

        ClusterNodeRepo::new(pool)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_service_starts_and_registers_node() {
        let repo = setup_test_repo().await;

        let config = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50001".to_string(),
            heartbeat_interval_secs: 10,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 20,
        };

        let service =
            DbDiscoveryService::with_node_id(config, repo.clone(), "disco-test-1-node".to_string());
        let node_id = service.node_id.clone(); // Capture the configured node_id
        let handle = service.start().await.unwrap();

        // Verify node was registered
        let node = repo.get_node(&node_id).await.unwrap();
        assert!(node.is_some());
        assert_eq!(node.unwrap().listen_addr, "127.0.0.1:50001");

        // Cleanup
        handle.shutdown().await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_heartbeat_updates() {
        let repo = setup_test_repo().await;

        let config = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50002".to_string(),
            heartbeat_interval_secs: 1, // Fast heartbeat for testing
            stale_threshold_secs: 30,
            cleanup_interval_secs: 60,
        };

        let service =
            DbDiscoveryService::with_node_id(config, repo.clone(), "disco-test-2-node".to_string());
        let node_id = service.node_id.clone(); // Capture the configured node_id
        let handle = service.start().await.unwrap();

        // Wait until the node is definitely registered before sampling heartbeat values.
        let node1 = wait_for_node(&repo, &node_id, 3)
            .await
            .expect("node should register before heartbeat sampling");

        // Wait for heartbeat to update
        let node2 = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(node) = repo.get_node(&node_id).await.unwrap() {
                    if node.last_heartbeat > node1.last_heartbeat {
                        break node;
                    }
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("heartbeat should update within timeout");

        // Heartbeat should have been updated
        assert!(node2.last_heartbeat > node1.last_heartbeat);

        // Cleanup
        handle.shutdown().await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_peer_discovery() {
        let repo = setup_test_repo().await;

        // Register node A
        let config_a = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50003".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 60,
        };

        let service_a = DbDiscoveryService::with_node_id(
            config_a,
            repo.clone(),
            "disco-test-3-node-a".to_string(),
        );
        let handle_a = service_a.start().await.unwrap();

        // Wait a bit
        sleep(Duration::from_millis(250)).await;

        // Register node B
        let config_b = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50004".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 60,
        };

        let service_b = DbDiscoveryService::with_node_id(
            config_b,
            repo.clone(),
            "disco-test-3-node-b".to_string(),
        );
        let handle_b = service_b.start().await.unwrap();

        // Node A should discover node B in the active peer view.
        assert!(
            wait_for_active_peer(&repo, "disco-test-3-node-a", "127.0.0.1:50004", 5).await,
            "Timeout waiting for peer discovery"
        );

        // Cleanup
        handle_a.shutdown().await;
        handle_b.shutdown().await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_stale_peer_detection() {
        let repo = setup_test_repo().await;

        // Manually register a stale node
        repo.register_node("disco-test-4-stale", "127.0.0.1:50005")
            .await
            .unwrap();

        // Make it stale by updating heartbeat to past
        sqlx::query(
            "UPDATE cluster_nodes SET last_heartbeat = NOW() - INTERVAL '5 seconds' WHERE node_id = $1"
        )
        .bind("disco-test-4-stale")
        .execute(repo.pool())
        .await
        .unwrap();

        // Start node A with short stale threshold
        let config_a = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50006".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 3,  // 3 seconds threshold
            cleanup_interval_secs: 2, // Cleanup every 2 seconds
        };

        let service_a = DbDiscoveryService::with_node_id(
            config_a,
            repo.clone(),
            "disco-test-4-node-a".to_string(),
        );
        let handle_a = service_a.start().await.unwrap();

        // Wait for cleanup to run
        sleep(Duration::from_millis(3000)).await;

        // Stale node should be removed
        let stale_node = repo.get_node("disco-test-4-stale").await.unwrap();
        assert!(
            stale_node.is_none(),
            "Stale node should have been cleaned up"
        );

        // Cleanup
        handle_a.shutdown().await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_shutdown_deregisters_node() {
        let repo = setup_test_repo().await;

        let config = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50007".to_string(),
            heartbeat_interval_secs: 10,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 20,
        };

        let service =
            DbDiscoveryService::with_node_id(config, repo.clone(), "disco-test-5-node".to_string());
        let node_id = service.node_id.clone(); // Capture the configured node_id
        let handle = service.start().await.unwrap();

        // Verify node exists
        let node = repo.get_node(&node_id).await.unwrap();
        assert!(node.is_some());

        // Shutdown
        handle.shutdown().await;
        sleep(Duration::from_millis(200)).await;

        // Node should be deregistered
        let node = repo.get_node(&node_id).await.unwrap();
        assert!(node.is_none(), "Node should be deregistered after shutdown");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_peer_removed_event() {
        let repo = setup_test_repo().await;

        // Start node A
        let config_a = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50008".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 2,
            cleanup_interval_secs: 60,
        };

        let service_a = DbDiscoveryService::with_node_id(
            config_a,
            repo.clone(),
            "disco-test-6-node-a".to_string(),
        );
        let mut handle_a = service_a.start().await.unwrap();

        // Start node B
        let config_b = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50009".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 2,
            cleanup_interval_secs: 60,
        };

        let service_b = DbDiscoveryService::with_node_id(
            config_b,
            repo.clone(),
            "disco-test-6-node-b".to_string(),
        );
        let handle_b = service_b.start().await.unwrap();

        // Wait for node A to discover node B
        let event = wait_for_peer_event(
            &mut handle_a.peer_events,
            PeerEvent::PeerAdded("127.0.0.1:50009".to_string()),
            5,
        )
        .await;
        assert_eq!(
            event,
            Some(PeerEvent::PeerAdded("127.0.0.1:50009".to_string()))
        );

        // Shutdown node B
        handle_b.shutdown().await;
        sleep(Duration::from_millis(500)).await;

        // Node A should detect node B is removed
        let event = wait_for_peer_event(
            &mut handle_a.peer_events,
            PeerEvent::PeerRemoved("127.0.0.1:50009".to_string()),
            5,
        )
        .await;
        assert_eq!(
            event,
            Some(PeerEvent::PeerRemoved("127.0.0.1:50009".to_string()))
        );

        // Cleanup
        handle_a.shutdown().await;
    }
}
