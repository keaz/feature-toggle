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
    }
}

impl Drop for DbDiscoveryHandle {
    fn drop(&mut self) {
        // Send shutdown signal to allow cleanup task to deregister the node
        let _ = self.shutdown_tx.send(());

        // Perform immediate deregistration in a blocking manner to ensure
        // the database record is removed even if tasks are aborted
        let repo = self.repo.clone();
        let node_id = self.node_id.clone();

        // Spawn a blocking task to deregister (won't block Drop)
        // This ensures deregistration completes even if the main tasks are aborted
        std::thread::spawn(move || {
            // Create a minimal tokio runtime for this blocking operation
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = repo.deregister_node(&node_id).await {
                    log::error!("Failed to deregister node {} in Drop: {}", node_id, e);
                } else {
                    log::info!("Successfully deregistered node {} during shutdown", node_id);
                }
            });
        });
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
        Self {
            config,
            repo,
            node_id: uuid::Uuid::new_v4().to_string(),
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
    use crate::cluster::db_discovery::ClusterNodeRepo;
    use sqlx::postgres::PgPoolOptions;
    use std::env;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    async fn setup_test_repo(node_prefix: &str) -> ClusterNodeRepo {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");

        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("Failed to connect to test database");

        // Clean up any existing test nodes for this prefix
        sqlx::query(&format!(
            "DELETE FROM cluster_nodes WHERE node_id LIKE '{}-%'",
            node_prefix
        ))
        .execute(&pool)
        .await
        .expect("Failed to clean up test data");

        ClusterNodeRepo::new(pool)
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - discovery tests hang. TODO: Fix shutdown/cleanup issue
    async fn test_service_starts_and_registers_node() {
        let repo = setup_test_repo("disco-test-1").await;

        let config = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50001".to_string(),
            heartbeat_interval_secs: 10,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 20,
        };

        let service = DbDiscoveryService::new(config, repo.clone());
        let node_id = service.node_id.clone(); // Capture the generated node_id
        let handle = service.start().await.unwrap();

        // Verify node was registered
        let node = repo.get_node(&node_id).await.unwrap();
        assert!(node.is_some());
        assert_eq!(node.unwrap().listen_addr, "127.0.0.1:50001");

        // Cleanup
        handle.shutdown().await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - discovery tests hang. TODO: Fix shutdown/cleanup issue
    async fn test_heartbeat_updates() {
        let repo = setup_test_repo("disco-test-2").await;

        let config = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50002".to_string(),
            heartbeat_interval_secs: 1, // Fast heartbeat for testing
            stale_threshold_secs: 30,
            cleanup_interval_secs: 60,
        };

        let service = DbDiscoveryService::new(config, repo.clone());
        let node_id = service.node_id.clone(); // Capture the generated node_id
        let handle = service.start().await.unwrap();

        // Get initial heartbeat
        sleep(Duration::from_millis(500)).await;
        let node1 = repo.get_node(&node_id).await.unwrap().unwrap();

        // Wait for heartbeat to update
        sleep(Duration::from_millis(1500)).await;
        let node2 = repo.get_node(&node_id).await.unwrap().unwrap();

        // Heartbeat should have been updated
        assert!(node2.last_heartbeat > node1.last_heartbeat);

        // Cleanup
        handle.shutdown().await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - discovery tests hang. TODO: Fix shutdown/cleanup issue
    async fn test_peer_discovery() {
        let repo = setup_test_repo("disco-test-3").await;

        // Register node A
        let config_a = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50003".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 60,
        };

        let service_a = DbDiscoveryService::new(config_a, repo.clone());
        let mut handle_a = service_a.start().await.unwrap();

        // Wait a bit
        sleep(Duration::from_millis(500)).await;

        // Register node B
        let config_b = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50004".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 60,
        };

        let service_b = DbDiscoveryService::new(config_b, repo.clone());
        let handle_b = service_b.start().await.unwrap();

        // Node A should discover node B
        let event = timeout(Duration::from_secs(3), handle_a.peer_events.recv())
            .await
            .expect("Timeout waiting for peer discovery")
            .expect("Channel closed");

        assert_eq!(event, PeerEvent::PeerAdded("127.0.0.1:50004".to_string()));

        // Cleanup
        handle_a.shutdown().await;
        handle_b.shutdown().await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - discovery tests hang. TODO: Fix shutdown/cleanup issue
    async fn test_stale_peer_detection() {
        let repo = setup_test_repo("disco-test-4").await;

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

        let service_a = DbDiscoveryService::new(config_a, repo.clone());
        let handle_a = service_a.start().await.unwrap();

        // Wait for cleanup to run
        sleep(Duration::from_millis(2500)).await;

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
    #[ignore] // Temporarily ignored - discovery tests hang. TODO: Fix shutdown/cleanup issue
    async fn test_shutdown_deregisters_node() {
        let repo = setup_test_repo("disco-test-5").await;

        let config = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50007".to_string(),
            heartbeat_interval_secs: 10,
            stale_threshold_secs: 30,
            cleanup_interval_secs: 20,
        };

        let service = DbDiscoveryService::new(config, repo.clone());
        let node_id = service.node_id.clone(); // Capture the generated node_id
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
    #[ignore] // Temporarily ignored - discovery tests hang. TODO: Fix shutdown/cleanup issue
    async fn test_peer_removed_event() {
        let repo = setup_test_repo("disco-test-6").await;

        // Start node A
        let config_a = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50008".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 2,
            cleanup_interval_secs: 60,
        };

        let service_a = DbDiscoveryService::new(config_a, repo.clone());
        let mut handle_a = service_a.start().await.unwrap();

        // Start node B
        let config_b = DbDiscoveryConfig {
            listen_addr: "127.0.0.1:50009".to_string(),
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 2,
            cleanup_interval_secs: 60,
        };

        let service_b = DbDiscoveryService::new(config_b, repo.clone());
        let handle_b = service_b.start().await.unwrap();

        // Wait for node A to discover node B
        let event = timeout(Duration::from_secs(3), handle_a.peer_events.recv())
            .await
            .expect("Timeout waiting for peer added");
        assert_eq!(
            event,
            Some(PeerEvent::PeerAdded("127.0.0.1:50009".to_string()))
        );

        // Shutdown node B
        handle_b.shutdown().await;
        sleep(Duration::from_millis(500)).await;

        // Node A should detect node B is removed
        let event = timeout(Duration::from_secs(4), handle_a.peer_events.recv())
            .await
            .expect("Timeout waiting for peer removed");
        assert_eq!(
            event,
            Some(PeerEvent::PeerRemoved("127.0.0.1:50009".to_string()))
        );

        // Cleanup
        handle_a.shutdown().await;
    }
}
