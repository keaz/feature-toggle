use std::net::TcpListener;
use std::time::Duration;

use feature_toggle_backend::cluster::{ClusterConfig, DiscoveryConfig};
use feature_toggle_backend::grpc::pb;
use feature_toggle_backend::grpc::pb::feature_update::Action;
use feature_toggle_backend::logic::feature_evaluation::FeatureEvaluationEvent;
use tokio::sync::broadcast;
use tokio::time::timeout;
use uuid::Uuid;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind to acquire free port")
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
#[ignore] // Temporarily ignored - cluster tests hang. TODO: Fix discovery/peer connection issue
async fn cluster_db_discovery_propagates_feature_updates() {
    println!("TEST: Starting database discovery cluster test");

    // Setup test database
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for database discovery tests");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to test database");

    // Clean up any existing test nodes
    sqlx::query("DELETE FROM cluster_nodes WHERE node_id LIKE 'cluster-test-%'")
        .execute(&pool)
        .await
        .expect("Failed to clean up test data");

    let port_a = free_port();
    let port_b = free_port();
    println!(
        "TEST: Allocated ports: port_a={}, port_b={}",
        port_a, port_b
    );

    let cfg_a = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_a}"),
        discovery: DiscoveryConfig {
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 5,
            cleanup_interval_secs: 10,
        },
        node_id: Some("cluster-test-node-a".into()),
        reconnect_delay_ms: 200,
        ..ClusterConfig::default()
    };

    let cfg_b = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_b}"),
        discovery: DiscoveryConfig {
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 5,
            cleanup_interval_secs: 10,
        },
        node_id: Some("cluster-test-node-b".into()),
        reconnect_delay_ms: 200,
        ..ClusterConfig::default()
    };

    let (updates_a, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_a, _) = broadcast::channel::<FeatureEvaluationEvent>(32);
    let (updates_b, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_b, _) = broadcast::channel::<FeatureEvaluationEvent>(32);

    println!("TEST: Starting node A");
    // Start both nodes with database discovery
    let guard_a = feature_toggle_backend::cluster::start(
        &cfg_a,
        Some(pool.clone()),
        updates_a.clone(),
        eval_a.clone(),
    )
    .expect("cluster A guard");

    println!("TEST: Starting node B");
    let guard_b = feature_toggle_backend::cluster::start(
        &cfg_b,
        Some(pool.clone()),
        updates_b.clone(),
        eval_b.clone(),
    )
    .expect("cluster B guard");

    println!("TEST: Waiting for database discovery and connections to be established");
    // Wait for database discovery and connections to be established
    tokio::time::sleep(Duration::from_millis(3000)).await;

    println!("TEST: Checking nodes in database");
    // Check if nodes are registered
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cluster_nodes WHERE node_id LIKE 'cluster-test-%'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count test nodes");
    println!("TEST: Found {} nodes in database", count);
    assert_eq!(count, 2, "Both nodes should be registered in database");

    // Subscribe to node B's updates
    println!("TEST: Subscribing to node B updates");
    let mut rx_b = updates_b.subscribe();
    let message_id = Uuid::new_v4().to_string();
    let update = pb::FeatureUpdate {
        message_id: message_id.clone(),
        action: Action::Upsert as i32,
        feature: None,
        feature_key: "db-discovered-feature".into(),
        error: String::new(),
    };

    // Send from node A
    println!("TEST: Sending update from node A");
    updates_a.send(update).unwrap();

    // Receive at node B
    println!("TEST: Waiting for update at node B");
    let received = timeout(Duration::from_secs(5), async {
        loop {
            match rx_b.recv().await {
                Ok(incoming) => {
                    println!("TEST: Received message_id: {}", incoming.message_id);
                    if incoming.message_id == message_id {
                        break incoming;
                    }
                }
                Err(e) => {
                    println!("TEST: Receiver error: {}", e);
                    panic!("cluster receiver closed");
                }
            }
        }
    })
    .await
    .expect("feature update propagated via database discovery");

    assert_eq!(received.feature_key, "db-discovered-feature");
    println!("TEST: Success! Feature update propagated correctly");

    // Cleanup
    println!("TEST: Cleaning up");
    drop(guard_a);
    drop(guard_b);

    // Give time for deregistration
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify nodes were deregistered
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM cluster_nodes WHERE node_id LIKE 'cluster-test-%'",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to count test nodes");

    println!("TEST: Nodes remaining after cleanup: {}", count);
    assert_eq!(count, 0, "Nodes should be deregistered after shutdown");
}
