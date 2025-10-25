use std::net::TcpListener;
use std::time::Duration;

use chrono::Utc;
use feature_toggle_backend::cluster::{ClusterConfig, DiscoveryConfig};
use feature_toggle_backend::grpc::pb;
use feature_toggle_backend::grpc::pb::feature_update::Action;
use feature_toggle_backend::logic::feature_evaluation::FeatureEvaluationEvent;
use tokio::sync::broadcast;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind to acquire free port")
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn cluster_propagates_feature_updates_between_nodes() {
    let port_a = free_port();
    let port_b = free_port();

    let cfg_a = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_a}"),
        discovery: DiscoveryConfig::Static {
            peers: vec![format!("127.0.0.1:{port_b}")],
        },
        node_id: Some("node-a".into()),
        reconnect_delay_ms: 200,
        ..ClusterConfig::default()
    };

    let cfg_b = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_b}"),
        discovery: DiscoveryConfig::Static {
            peers: vec![format!("127.0.0.1:{port_a}")],
        },
        node_id: Some("node-b".into()),
        reconnect_delay_ms: 200,
        ..ClusterConfig::default()
    };

    let (updates_a, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_a, _) = broadcast::channel::<FeatureEvaluationEvent>(32);
    let (updates_b, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_b, _) = broadcast::channel::<FeatureEvaluationEvent>(32);

    let guard_a = feature_toggle_backend::cluster::start(&cfg_a, updates_a.clone(), eval_a.clone())
        .expect("cluster A guard");
    let guard_b = feature_toggle_backend::cluster::start(&cfg_b, updates_b.clone(), eval_b.clone())
        .expect("cluster B guard");

    // Give peers a moment to connect.
    sleep(Duration::from_millis(400)).await;

    let mut rx_b = updates_b.subscribe();
    let message_id = Uuid::new_v4().to_string();
    let update = pb::FeatureUpdate {
        message_id: message_id.clone(),
        action: Action::Upsert as i32,
        feature: None,
        feature_key: "migrated-feature".into(),
        error: String::new(),
    };

    updates_a.send(update).unwrap();

    let received = timeout(Duration::from_secs(2), async {
        loop {
            match rx_b.recv().await {
                Ok(incoming) if incoming.message_id == message_id => break incoming,
                Ok(_) => continue,
                Err(_) => break panic!("cluster receiver closed"),
            }
        }
    })
    .await
    .expect("feature update propagated to node B");

    assert_eq!(received.feature_key, "migrated-feature");

    drop(guard_a);
    drop(guard_b);
}

#[tokio::test]
async fn cluster_propagates_evaluation_events_between_nodes() {
    let port_a = free_port();
    let port_b = free_port();

    let cfg_a = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_a}"),
        discovery: DiscoveryConfig::Static {
            peers: vec![format!("127.0.0.1:{port_b}")],
        },
        node_id: Some("node-a-eval".into()),
        reconnect_delay_ms: 200,
        ..ClusterConfig::default()
    };

    let cfg_b = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_b}"),
        discovery: DiscoveryConfig::Static {
            peers: vec![format!("127.0.0.1:{port_a}")],
        },
        node_id: Some("node-b-eval".into()),
        reconnect_delay_ms: 200,
        ..ClusterConfig::default()
    };

    let (updates_a, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_a, _) = broadcast::channel::<FeatureEvaluationEvent>(32);
    let (updates_b, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_b, _) = broadcast::channel::<FeatureEvaluationEvent>(32);

    let guard_a = feature_toggle_backend::cluster::start(&cfg_a, updates_a.clone(), eval_a.clone())
        .expect("cluster A guard");
    let guard_b = feature_toggle_backend::cluster::start(&cfg_b, updates_b.clone(), eval_b.clone())
        .expect("cluster B guard");

    sleep(Duration::from_millis(400)).await;

    let mut eval_rx_b = eval_b.subscribe();
    let event = FeatureEvaluationEvent {
        event_id: Uuid::new_v4(),
        feature_key: "remote-rollout".into(),
        environment_id: "env-123".into(),
        client_id: Uuid::new_v4(),
        evaluated_at: Utc::now(),
        evaluation_result: true,
        prior_assignment: false,
        user_context: Some("user-42".into()),
    };

    eval_a.send(event.clone()).unwrap();

    let received = timeout(Duration::from_secs(2), async {
        loop {
            match eval_rx_b.recv().await {
                Ok(incoming) if incoming.event_id == event.event_id => break incoming,
                Ok(_) => continue,
                Err(_) => break panic!("evaluation receiver closed"),
            }
        }
    })
    .await
    .expect("evaluation event propagated to node B");

    assert_eq!(received.feature_key, "remote-rollout");
    assert_eq!(received.user_context.as_deref(), Some("user-42"));

    drop(guard_a);
    drop(guard_b);
}

#[tokio::test]
async fn cluster_dns_discovery_picks_up_peer() {
    let port_a = free_port();
    let port_b = free_port();

    let cfg_a = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_a}"),
        discovery: DiscoveryConfig::Dns {
            record: "127.0.0.1".into(),
            port: port_b,
            refresh_ms: 200,
        },
        node_id: Some("node-a-dns".into()),
        reconnect_delay_ms: 100,
        ..ClusterConfig::default()
    };

    let cfg_b = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port_b}"),
        discovery: DiscoveryConfig::Static {
            peers: vec![format!("127.0.0.1:{port_a}")],
        },
        node_id: Some("node-b-dns".into()),
        reconnect_delay_ms: 100,
        ..ClusterConfig::default()
    };

    let (updates_a, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_a, _) = broadcast::channel::<FeatureEvaluationEvent>(32);
    let (updates_b, _) = broadcast::channel::<pb::FeatureUpdate>(32);
    let (eval_b, _) = broadcast::channel::<FeatureEvaluationEvent>(32);

    let guard_a = feature_toggle_backend::cluster::start(&cfg_a, updates_a.clone(), eval_a)
        .expect("cluster A guard");
    let guard_b = feature_toggle_backend::cluster::start(&cfg_b, updates_b.clone(), eval_b)
        .expect("cluster B guard");

    // Allow DNS discovery loop to resolve and connect.
    sleep(Duration::from_millis(600)).await;

    let mut rx_a = updates_a.subscribe();
    let message_id = Uuid::new_v4().to_string();
    updates_b
        .send(pb::FeatureUpdate {
            message_id: message_id.clone(),
            action: Action::Upsert as i32,
            feature: None,
            feature_key: "dns-feature".into(),
            error: String::new(),
        })
        .unwrap();

    timeout(Duration::from_secs(3), async {
        loop {
            match rx_a.recv().await {
                Ok(incoming) if incoming.message_id == message_id => break,
                Ok(_) => continue,
                Err(err) => panic!("receiver closed: {err}"),
            }
        }
    })
    .await
    .expect("dns discovery propagated feature update");

    drop(guard_a);
    drop(guard_b);
}
