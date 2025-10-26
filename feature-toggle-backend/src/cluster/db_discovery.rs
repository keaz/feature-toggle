//! Database-backed cluster node discovery
//!
//! This module provides database operations for cluster node registration,
//! heartbeat updates, and peer discovery. Similar to JGroups JDBC_PING,
//! nodes register themselves in the database and discover peers by querying
//! the cluster_nodes table.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Result type for cluster database operations
pub type Result<T> = std::result::Result<T, ClusterDbError>;

/// Errors that can occur during cluster database operations
#[derive(Debug, thiserror::Error)]
pub enum ClusterDbError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Node {0} not found")]
    NodeNotFound(String),

    #[error("Invalid node ID: {0}")]
    InvalidNodeId(String),
}

/// Represents a cluster node record in the database
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ClusterNode {
    pub node_id: String,
    pub listen_addr: String,
    pub last_heartbeat: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Repository for cluster node database operations
#[derive(Clone)]
pub struct ClusterNodeRepo {
    pool: PgPool,
}

impl ClusterNodeRepo {
    /// Create a new cluster node repository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying pool (for testing)
    #[cfg(test)]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Register this node in the database
    ///
    /// This uses an UPSERT (INSERT ... ON CONFLICT) to handle the case where
    /// the node is already registered (e.g., due to a previous crash without cleanup).
    ///
    /// # Arguments
    /// * `node_id` - Unique identifier for this node
    /// * `listen_addr` - Address where this node listens for cluster connections
    ///
    /// # Returns
    /// * `Ok(())` if registration successful
    /// * `Err(ClusterDbError)` if database operation fails
    pub async fn register_node(&self, node_id: &str, listen_addr: &str) -> Result<()> {
        if node_id.is_empty() {
            return Err(ClusterDbError::InvalidNodeId(
                "node_id cannot be empty".into(),
            ));
        }

        sqlx::query(
            r#"
            INSERT INTO cluster_nodes (node_id, listen_addr, last_heartbeat, created_at)
            VALUES ($1, $2, NOW(), NOW())
            ON CONFLICT (node_id)
            DO UPDATE SET
                listen_addr = EXCLUDED.listen_addr,
                last_heartbeat = NOW()
            "#,
        )
        .bind(node_id)
        .bind(listen_addr)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update the heartbeat timestamp for this node
    ///
    /// Should be called periodically to indicate this node is still alive.
    ///
    /// # Arguments
    /// * `node_id` - Unique identifier for this node
    ///
    /// # Returns
    /// * `Ok(())` if heartbeat updated successfully
    /// * `Err(ClusterDbError::NodeNotFound)` if node is not registered
    /// * `Err(ClusterDbError)` for other database errors
    pub async fn heartbeat(&self, node_id: &str) -> Result<()> {
        let result = sqlx::query(
            r#"
            UPDATE cluster_nodes
            SET last_heartbeat = NOW()
            WHERE node_id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ClusterDbError::NodeNotFound(node_id.to_string()));
        }

        Ok(())
    }

    /// Get all active peers (excluding self)
    ///
    /// Returns addresses of nodes that have sent a heartbeat within the timeout period.
    ///
    /// # Arguments
    /// * `node_id` - ID of the current node (will be excluded from results)
    /// * `timeout_secs` - Maximum age of last heartbeat in seconds
    ///
    /// # Returns
    /// * `Ok(Vec<String>)` - List of listen addresses for active peers
    /// * `Err(ClusterDbError)` if database operation fails
    pub async fn get_active_peers(&self, node_id: &str, timeout_secs: u64) -> Result<Vec<String>> {
        let peers = sqlx::query_as::<_, (String,)>(
            r#"
            SELECT listen_addr
            FROM cluster_nodes
            WHERE node_id != $1
              AND last_heartbeat > NOW() - INTERVAL '1 second' * $2
            ORDER BY created_at ASC
            "#,
        )
        .bind(node_id)
        .bind(timeout_secs as i64)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|(addr,)| addr)
        .collect();

        Ok(peers)
    }

    /// Get a specific cluster node by ID
    ///
    /// # Arguments
    /// * `node_id` - ID of the node to retrieve
    ///
    /// # Returns
    /// * `Ok(Some(ClusterNode))` if node found
    /// * `Ok(None)` if node not found
    /// * `Err(ClusterDbError)` if database operation fails
    pub async fn get_node(&self, node_id: &str) -> Result<Option<ClusterNode>> {
        let node = sqlx::query_as::<_, ClusterNode>(
            r#"
            SELECT node_id, listen_addr, last_heartbeat, created_at
            FROM cluster_nodes
            WHERE node_id = $1
            "#,
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(node)
    }

    /// Remove this node from the cluster
    ///
    /// Should be called during graceful shutdown.
    ///
    /// # Arguments
    /// * `node_id` - Unique identifier for this node
    ///
    /// # Returns
    /// * `Ok(())` if deregistration successful (or node doesn't exist)
    /// * `Err(ClusterDbError)` if database operation fails
    pub async fn deregister_node(&self, node_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM cluster_nodes
            WHERE node_id = $1
            "#,
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Clean up stale nodes from the database
    ///
    /// Removes nodes that haven't sent a heartbeat within the timeout period.
    /// This is a maintenance operation that can be run periodically.
    ///
    /// # Arguments
    /// * `timeout_secs` - Maximum age of last heartbeat in seconds
    ///
    /// # Returns
    /// * `Ok(u64)` - Number of stale nodes removed
    /// * `Err(ClusterDbError)` if database operation fails
    pub async fn cleanup_stale_nodes(&self, timeout_secs: u64) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM cluster_nodes
            WHERE last_heartbeat < NOW() - INTERVAL '1 second' * $1
            "#,
        )
        .bind(timeout_secs as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Get all cluster nodes (for debugging/monitoring)
    ///
    /// # Returns
    /// * `Ok(Vec<ClusterNode>)` - List of all registered nodes
    /// * `Err(ClusterDbError)` if database operation fails
    pub async fn get_all_nodes(&self) -> Result<Vec<ClusterNode>> {
        let nodes = sqlx::query_as::<_, ClusterNode>(
            r#"
            SELECT node_id, listen_addr, last_heartbeat, created_at
            FROM cluster_nodes
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use std::env;
    use std::time::Duration;

    async fn setup_test_db() -> PgPool {
        let database_url = env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for tests");

        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("Failed to connect to test database");

        // Clean up any existing test nodes
        sqlx::query("DELETE FROM cluster_nodes WHERE node_id LIKE 'test-%'")
            .execute(&pool)
            .await
            .expect("Failed to clean up test data");

        pool
    }

    async fn cleanup_test_db(pool: &PgPool) {
        sqlx::query("DELETE FROM cluster_nodes WHERE node_id LIKE 'test-%'")
            .execute(pool)
            .await
            .expect("Failed to clean up test data");
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_register_node() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        let result = repo.register_node("test-node-1", "127.0.0.1:50051").await;
        assert!(result.is_ok(), "Failed to register node: {:?}", result);

        // Verify node exists
        let node = repo.get_node("test-node-1").await.unwrap();
        assert!(node.is_some());
        let node = node.unwrap();
        assert_eq!(node.node_id, "test-node-1");
        assert_eq!(node.listen_addr, "127.0.0.1:50051");

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_register_node_upsert() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register node first time
        repo.register_node("test-node-2", "127.0.0.1:50051").await.unwrap();

        // Register same node with different address (should update)
        repo.register_node("test-node-2", "127.0.0.1:50052").await.unwrap();

        // Verify address was updated
        let node = repo.get_node("test-node-2").await.unwrap().unwrap();
        assert_eq!(node.listen_addr, "127.0.0.1:50052");

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_register_node_empty_id() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        let result = repo.register_node("", "127.0.0.1:50051").await;
        assert!(matches!(result, Err(ClusterDbError::InvalidNodeId(_))));

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_heartbeat() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register node first
        repo.register_node("test-node-3", "127.0.0.1:50051").await.unwrap();

        let node_before = repo.get_node("test-node-3").await.unwrap().unwrap();

        // Wait a bit and send heartbeat
        tokio::time::sleep(Duration::from_millis(100)).await;
        repo.heartbeat("test-node-3").await.unwrap();

        let node_after = repo.get_node("test-node-3").await.unwrap().unwrap();

        // Heartbeat should be updated
        assert!(node_after.last_heartbeat > node_before.last_heartbeat);

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_heartbeat_nonexistent_node() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        let result = repo.heartbeat("test-nonexistent").await;
        assert!(matches!(result, Err(ClusterDbError::NodeNotFound(_))));

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_get_active_peers() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register multiple nodes
        repo.register_node("test-node-4", "127.0.0.1:50051").await.unwrap();
        repo.register_node("test-node-5", "127.0.0.1:50052").await.unwrap();
        repo.register_node("test-node-6", "127.0.0.1:50053").await.unwrap();

        // Get peers for node-4 (should see node-5 and node-6)
        let peers = repo.get_active_peers("test-node-4", 60).await.unwrap();
        assert_eq!(peers.len(), 2);
        assert!(peers.contains(&"127.0.0.1:50052".to_string()));
        assert!(peers.contains(&"127.0.0.1:50053".to_string()));
        assert!(!peers.contains(&"127.0.0.1:50051".to_string())); // Should not include self

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_get_active_peers_excludes_stale() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register nodes
        repo.register_node("test-node-7", "127.0.0.1:50051").await.unwrap();
        repo.register_node("test-node-8", "127.0.0.1:50052").await.unwrap();

        // Make node-8 stale by manually updating its heartbeat to the past
        sqlx::query(
            "UPDATE cluster_nodes SET last_heartbeat = NOW() - INTERVAL '120 seconds' WHERE node_id = $1"
        )
        .bind("test-node-8")
        .execute(&pool)
        .await
        .unwrap();

        // Get active peers with 60s timeout (should not include node-8)
        let peers = repo.get_active_peers("test-node-7", 60).await.unwrap();
        assert_eq!(peers.len(), 0); // node-8 is stale

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_deregister_node() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register and then deregister
        repo.register_node("test-node-9", "127.0.0.1:50051").await.unwrap();

        let node = repo.get_node("test-node-9").await.unwrap();
        assert!(node.is_some());

        repo.deregister_node("test-node-9").await.unwrap();

        let node = repo.get_node("test-node-9").await.unwrap();
        assert!(node.is_none());

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_deregister_nonexistent_node() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Deregistering nonexistent node should not error
        let result = repo.deregister_node("test-nonexistent").await;
        assert!(result.is_ok());

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_cleanup_stale_nodes() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register multiple nodes
        repo.register_node("test-node-10", "127.0.0.1:50051").await.unwrap();
        repo.register_node("test-node-11", "127.0.0.1:50052").await.unwrap();
        repo.register_node("test-node-12", "127.0.0.1:50053").await.unwrap();

        // Make some nodes stale
        sqlx::query(
            "UPDATE cluster_nodes SET last_heartbeat = NOW() - INTERVAL '120 seconds' WHERE node_id IN ($1, $2)"
        )
        .bind("test-node-10")
        .bind("test-node-11")
        .execute(&pool)
        .await
        .unwrap();

        // Cleanup stale nodes (>60s old)
        let removed = repo.cleanup_stale_nodes(60).await.unwrap();
        assert_eq!(removed, 2);

        // Verify only active node remains
        let all_nodes = repo.get_all_nodes().await.unwrap();
        let test_nodes: Vec<_> = all_nodes.iter()
            .filter(|n| n.node_id.starts_with("test-"))
            .collect();
        assert_eq!(test_nodes.len(), 1);
        assert_eq!(test_nodes[0].node_id, "test-node-12");

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_get_all_nodes() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Register multiple nodes
        repo.register_node("test-node-13", "127.0.0.1:50051").await.unwrap();
        repo.register_node("test-node-14", "127.0.0.1:50052").await.unwrap();

        let all_nodes = repo.get_all_nodes().await.unwrap();
        let test_nodes: Vec<_> = all_nodes.iter()
            .filter(|n| n.node_id.starts_with("test-"))
            .collect();

        assert!(test_nodes.len() >= 2);
        assert!(test_nodes.iter().any(|n| n.node_id == "test-node-13"));
        assert!(test_nodes.iter().any(|n| n.node_id == "test-node-14"));

        cleanup_test_db(&pool).await;
    }

    #[tokio::test]
    #[ignore] // Temporarily ignored - database repository tests may hang. TODO: Fix potential async issues
    async fn test_concurrent_registration() {
        let pool = setup_test_db().await;
        let repo = ClusterNodeRepo::new(pool.clone());

        // Simulate concurrent registration of same node
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let repo = repo.clone();
                tokio::spawn(async move {
                    repo.register_node(
                        "test-node-concurrent",
                        &format!("127.0.0.1:5005{}", i)
                    ).await
                })
            })
            .collect();

        // All should succeed (UPSERT handles concurrency)
        for handle in handles {
            handle.await.unwrap().unwrap();
        }

        // Verify node exists (address will be from one of the registrations)
        let node = repo.get_node("test-node-concurrent").await.unwrap();
        assert!(node.is_some());

        cleanup_test_db(&pool).await;
    }
}
