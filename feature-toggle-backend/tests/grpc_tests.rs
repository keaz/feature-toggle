use feature_toggle_backend::Error;
use feature_toggle_backend::database::client::MockClientRepository;
use feature_toggle_backend::database::entity as db;
use feature_toggle_backend::database::feature::MockFeatureRepository;
use feature_toggle_backend::grpc::pb;
use feature_toggle_backend::grpc::pb::feature_evaluation_client::FeatureEvaluationClient;
use feature_toggle_backend::grpc::pb::{EvaluateRequest, GetFeatureByKeyRequest, StreamRequest};
use feature_toggle_backend::grpc::{
    FeatureEvaluationSvc, feature_evaluation_server::FeatureEvaluationServer,
};
use std::net::SocketAddr;
use tokio::sync::broadcast;
use tokio::time::{Duration, sleep};
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::Server;
use uuid::Uuid;

async fn start_server_with_repos(
    feature_repo: Box<dyn feature_toggle_backend::database::feature::FeatureRepository>,
    client_repo: Box<dyn feature_toggle_backend::database::client::ClientRepository>,
    updates_tx: broadcast::Sender<pb::FeatureUpdate>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = TcpListenerStream::new(listener);
    let (evaluation_events_tx, _) = broadcast::channel(32);
    let svc = FeatureEvaluationSvc::new_with_repos(
        feature_repo,
        client_repo,
        updates_tx,
        evaluation_events_tx,
    );
    let router = Server::builder().add_service(FeatureEvaluationServer::new(svc));
    let handle = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, handle)
}

fn client_ids() -> (String, String) {
    // Seeded in init.sql
    (
        "a1b2c3d4-0000-4000-8000-000000000001".to_string(),
        "TEST_WEB_KEY_1".to_string(),
    )
}

fn valid_env_id() -> String {
    "51ecc366-f1cd-4d3d-ab73-fa60bad98f27".to_string()
}

#[tokio::test]
async fn evaluate_validation_errors() {
    use chrono::{Duration as ChronoDuration, Utc};
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id; // reuse constant string

    // Mock client repository
    let mut client_mock = MockClientRepository::new();
    client_mock.expect_get_client_by_id().returning(move |id| {
        let out: Result<db::Client, Error> = if id == valid_client_id {
            Ok(db::Client {
                id,
                team_id,
                name: "Client".into(),
                description: None,
                enabled: true,
                client_type: db::ClientType::Web,
                api_key: sec.clone(),
                web_origins: None,
            })
        } else {
            Err(Error::NotFound(id))
        };
        out
    });

    // Mock feature repository: minimal behavior
    let mut feature_mock = MockFeatureRepository::new();
    let feature_id = Uuid::new_v4();
    let stage_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_stages()
        .returning(move |fid| {
            if fid == feature_id {
                Ok(vec![db::FeaturePipelineStage {
                    id: stage_id,
                    feature_id,
                    environment_id: env_id,
                    order_index: 0,
                    parent_stage_id: None,
                    position: "Start".into(),
                    enabled: true,
                    status: "NOT_DEPLOYED".into(),
                }])
            } else {
                Ok(vec![])
            }
        });
    feature_mock
        .expect_get_features()
        .returning(move |_team, key, _ftype| {
            let res: Result<Vec<db::Feature>, Error> = match key.as_deref() {
                Some("Test Feature") => Ok(vec![db::Feature {
                    id: feature_id,
                    key: "Test Feature".into(),
                    description: Some(String::new()),
                    feature_type: db::FeatureType::Simple,
                    team_id,
                    active: true,
                    created_at: Utc::now(),
                    kill_switch_enabled: true,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(30)),
                    lifecycle_stage: "active".to_string(),
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                }]),
                _ => Ok(vec![]),
            };
            res
        });
    feature_mock
        .expect_get_stage_criteria()
        .returning(|_sid| Ok(Vec::new()));

    let (tx, _rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), tx).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint).await.unwrap();

    // missing client_id
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: String::new(),
        client_secret: "x".into(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // missing client_secret
    let (cid, _sec) = client_ids();
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid.clone(),
        client_secret: String::new(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // missing feature_key
    let req = EvaluateRequest {
        feature_key: String::new(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid.clone(),
        client_secret: "x".into(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // invalid uuid
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: "not-a-uuid".into(),
        client_secret: "x".into(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // client not found
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: uuid::Uuid::new_v4().to_string(),
        client_secret: "x".into(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn evaluate_auth_and_success() {
    use chrono::{Duration as ChronoDuration, Utc};
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let disabled_id = Uuid::new_v4();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    // Client mock with disabled and enabled clients
    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        let out: Result<db::Client, Error> = if id == valid_client_id {
            Ok(db::Client {
                id,
                team_id,
                name: "Client".into(),
                description: None,
                enabled: true,
                client_type: db::ClientType::Web,
                api_key: sec_clone.clone(),
                web_origins: None,
            })
        } else if id == disabled_id {
            Ok(db::Client {
                id,
                team_id,
                name: "Disabled".into(),
                description: None,
                enabled: false,
                client_type: db::ClientType::Backend,
                api_key: "DISABLED_KEY".into(),
                web_origins: None,
            })
        } else {
            Err(Error::NotFound(id))
        };
        out
    });

    // Feature mock
    let mut feature_mock = MockFeatureRepository::new();
    let feature_id = Uuid::new_v4();
    let stage_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_stages()
        .returning(move |fid| {
            if fid == feature_id {
                Ok(vec![db::FeaturePipelineStage {
                    id: stage_id,
                    feature_id,
                    environment_id: env_id,
                    order_index: 0,
                    parent_stage_id: None,
                    position: "Start".into(),
                    enabled: true,
                    status: "NOT_DEPLOYED".into(),
                }])
            } else {
                Ok(vec![])
            }
        });
    feature_mock
        .expect_get_features()
        .returning(move |_team, key, _ftype| {
            let res: Result<Vec<db::Feature>, Error> = match key.as_deref() {
                Some("Test Feature") => Ok(vec![db::Feature {
                    id: feature_id,
                    key: "Test Feature".into(),
                    description: Some(String::new()),
                    feature_type: db::FeatureType::Simple,
                    team_id,
                    active: true,
                    created_at: Utc::now(),
                    kill_switch_enabled: true,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(45)),
                    lifecycle_stage: "active".to_string(),
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                }]),
                _ => Ok(vec![]),
            };
            res
        });
    feature_mock
        .expect_get_stage_criteria()
        .returning(|_sid| Ok(Vec::new()));

    let (tx, _rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), tx).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint).await.unwrap();

    // disabled client
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: disabled_id.to_string(),
        client_secret: "DISABLED_KEY".into(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    // wrong secret
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid.clone(),
        client_secret: "WRONG".into(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // feature not found
    let req = EvaluateRequest {
        feature_key: "NoSuchKey".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid.clone(),
        client_secret: sec.clone(),
    };
    let err = client.evaluate(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // happy path
    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid.clone(),
        client_secret: sec.clone(),
    };
    let resp = client.evaluate(req).await.unwrap().into_inner();
    assert!(resp.enabled);
}

#[tokio::test]
async fn get_feature_by_key_and_stream_branches() {
    use chrono::{Duration as ChronoDuration, Utc};
    // Create a tiny buffer to induce lag
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(1);

    // Build mocks
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        let out: Result<db::Client, Error> = if id == valid_client_id {
            Ok(db::Client {
                id,
                team_id,
                name: "Client".into(),
                description: None,
                enabled: true,
                client_type: db::ClientType::Web,
                api_key: sec_clone.clone(),
                web_origins: None,
            })
        } else {
            Err(Error::NotFound(id))
        };
        out
    });

    let mut feature_mock = MockFeatureRepository::new();
    let feature_id = Uuid::new_v4();
    let stage_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_stages()
        .returning(move |fid| {
            if fid == feature_id {
                Ok(vec![db::FeaturePipelineStage {
                    id: stage_id,
                    feature_id,
                    environment_id: env_id,
                    order_index: 0,
                    parent_stage_id: None,
                    position: "Start".into(),
                    enabled: true,
                    status: "NOT_DEPLOYED".into(),
                }])
            } else {
                Ok(vec![])
            }
        });
    feature_mock
        .expect_get_features()
        .returning(move |_team, key, _ftype| {
            let res: Result<Vec<db::Feature>, Error> = match key.as_deref() {
                Some("Test Feature") => Ok(vec![db::Feature {
                    id: feature_id,
                    key: "Test Feature".into(),
                    description: Some(String::new()),
                    feature_type: db::FeatureType::Simple,
                    team_id,
                    active: true,
                    created_at: Utc::now(),
                    kill_switch_enabled: true,
                    kill_switch_activated_at: None,
                    rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(15)),
                    lifecycle_stage: "active".to_string(),
                    deprecated_at: None,
                    deprecation_notice: None,
                    last_evaluated_at: None,
                    evaluation_count_7d: 0,
                    evaluation_count_30d: 0,
                    evaluation_count_90d: 0,
                    dependencies: vec![],
                }]),
                _ => Ok(vec![]),
            };
            res
        });
    feature_mock
        .expect_get_stage_criteria()
        .returning(|_sid| Ok(Vec::new()));

    let (addr, _server) = start_server_with_repos(
        Box::new(feature_mock),
        Box::new(client_mock),
        updates_tx.clone(),
    )
    .await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();

    // get_feature_by_key validations
    // missing fields
    let err = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let err = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "x".into(),
            client_id: String::new(),
            client_secret: "y".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let (cid, sec) = client_ids();
    let err = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "x".into(),
            client_id: cid.clone(),
            client_secret: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // invalid uuid
    let err = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "x".into(),
            client_id: "not-a-uuid".into(),
            client_secret: "y".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // not found client
    let err = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "x".into(),
            client_id: uuid::Uuid::new_v4().to_string(),
            client_secret: "y".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // wrong secret
    let err = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "x".into(),
            client_id: cid.clone(),
            client_secret: "WRONG".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // feature not found - should return None, not error
    let resp = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "NoSuchKey".into(),
            client_id: cid.clone(),
            client_secret: sec.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(
        resp.feature.is_none(),
        "Expected None when feature not found"
    );

    // success and track requested_keys
    let resp = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "Test Feature".into(),
            client_id: cid.clone(),
            client_secret: sec.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(resp.feature.is_some());

    // Now connect to stream without sending subscribe -> expect invalid argument
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(4);
    // immediately drop without sending first message
    drop(tx_in);
    let res = raw.stream_updates(ReceiverStream::new(rx_out)).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Send non-subscribe as first message
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(4);
    let _ = tx_in
        .send(StreamRequest {
            payload: Some(pb::stream_request::Payload::Heartbeat(pb::Heartbeat {
                ts_unix_ms: 0,
            })),
        })
        .await;
    let res = raw.stream_updates(ReceiverStream::new(rx_out)).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Proper subscribe but missing creds
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    let _ = tx_in
        .send(StreamRequest {
            payload: Some(pb::stream_request::Payload::Subscribe(
                pb::SubscribeRequest {
                    client_id: String::new(),
                    client_secret: String::new(),
                    feature_keys: vec![],
                    environment_id: String::new(),
                },
            )),
        })
        .await;
    let res = raw.stream_updates(ReceiverStream::new(rx_out)).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Subscribe with invalid uuid
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    let _ = tx_in
        .send(StreamRequest {
            payload: Some(pb::stream_request::Payload::Subscribe(
                pb::SubscribeRequest {
                    client_id: "not-a-uuid".into(),
                    client_secret: "x".into(),
                    feature_keys: vec![],
                    environment_id: String::new(),
                },
            )),
        })
        .await;
    let res = raw.stream_updates(ReceiverStream::new(rx_out)).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code(), tonic::Code::InvalidArgument);

    // Subscribe not found client
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    let _ = tx_in
        .send(StreamRequest {
            payload: Some(pb::stream_request::Payload::Subscribe(
                pb::SubscribeRequest {
                    client_id: uuid::Uuid::new_v4().to_string(),
                    client_secret: "x".into(),
                    feature_keys: vec![],
                    environment_id: String::new(),
                },
            )),
        })
        .await;
    let res = raw.stream_updates(ReceiverStream::new(rx_out)).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code(), tonic::Code::NotFound);

    // Subscribe wrong secret
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    let _ = tx_in
        .send(StreamRequest {
            payload: Some(pb::stream_request::Payload::Subscribe(
                pb::SubscribeRequest {
                    client_id: cid.clone(),
                    client_secret: "WRONG".into(),
                    feature_keys: vec![],
                    environment_id: String::new(),
                },
            )),
        })
        .await;
    let res = raw.stream_updates(ReceiverStream::new(rx_out)).await;
    assert!(res.is_err());
    assert_eq!(res.unwrap_err().code(), tonic::Code::Unauthenticated);

    // Subscribe success: should emit snapshot for previously requested key
    let mut raw = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_in, rx_out) = tokio::sync::mpsc::channel::<pb::StreamRequest>(32);
    let _ = tx_in
        .send(StreamRequest {
            payload: Some(pb::stream_request::Payload::Subscribe(
                pb::SubscribeRequest {
                    client_id: cid.clone(),
                    client_secret: sec.clone(),
                    feature_keys: vec![],
                    environment_id: String::new(),
                },
            )),
        })
        .await;
    let mut stream = raw
        .stream_updates(ReceiverStream::new(rx_out))
        .await
        .unwrap()
        .into_inner();

    // Expect first a snapshot FeatureUpdate for "Test Feature"
    let mut got_snapshot = false;
    // Also test heartbeat handling: send a heartbeat in parallel and expect a HEARTBEAT action
    let tx_in_clone = tx_in.clone();
    tokio::spawn(async move {
        let _ = tx_in_clone
            .send(StreamRequest {
                payload: Some(pb::stream_request::Payload::Heartbeat(pb::Heartbeat {
                    ts_unix_ms: 123,
                })),
            })
            .await;
    });

    for _ in 0..5 {
        if let Some(Ok(update)) = stream.message().await.transpose() {
            if update.action == (pb::feature_update::Action::Snapshot as i32) {
                assert!(update.feature.as_ref().map(|f| f.key.as_str()) == Some("Test Feature"));
                got_snapshot = true;
                break;
            }
        }
    }
    assert!(got_snapshot, "did not receive snapshot");

    // Now expect a heartbeat update at some point
    let mut got_heartbeat = false;
    for _ in 0..10 {
        if let Some(Ok(update)) = stream.message().await.transpose() {
            if update.action == (pb::feature_update::Action::Heartbeat as i32) {
                got_heartbeat = true;
                break;
            }
        }
    }
    assert!(got_heartbeat, "did not receive heartbeat");

    // Live update filtering: send an UPSERT for the requested key and for a different key
    // First, different key should be ignored
    let other = pb::FeatureUpdate {
        message_id: uuid::Uuid::new_v4().to_string(),
        action: pb::feature_update::Action::Upsert as i32,
        feature: Some(pb::FeatureFull {
            id: uuid::Uuid::new_v4().to_string(),
            key: "Another feature".into(),
            description: String::new(),
            feature_type: "Simple".into(),
            team_id: "51ecc366-f1cd-4d3d-ab73-fa60bad98f27".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            active: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: chrono::Utc::now().to_rfc3339(),
            stages: vec![],
            dependencies: vec![],
            variants: vec![],
        }),
        feature_key: String::new(),
        error: String::new(),
    };
    updates_tx.send(other).unwrap();

    // Then, send a matching key
    let matching = pb::FeatureUpdate {
        message_id: uuid::Uuid::new_v4().to_string(),
        action: pb::feature_update::Action::Upsert as i32,
        feature: Some(pb::FeatureFull {
            id: uuid::Uuid::new_v4().to_string(),
            key: "Test Feature".into(),
            description: String::new(),
            feature_type: "Simple".into(),
            team_id: "51ecc366-f1cd-4d3d-ab73-fa60bad98f27".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            active: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: chrono::Utc::now().to_rfc3339(),
            stages: vec![],
            dependencies: vec![],
            variants: vec![],
        }),
        feature_key: String::new(),
        error: String::new(),
    };
    updates_tx.send(matching.clone()).unwrap();

    // Expect to receive the matching update soon, and not necessarily the other one
    let mut got_matching = false;
    for _ in 0..10 {
        if let Some(Ok(update)) = stream.message().await.transpose() {
            if update.action == (pb::feature_update::Action::Upsert as i32)
                && update.feature.as_ref().map(|f| f.key.as_str()) == Some("Test Feature")
            {
                got_matching = true;
                break;
            }
        }
    }
    assert!(got_matching, "did not receive matching UPSERT");

    // Induce lag: send many messages quickly to overflow the broadcast buffer while we don't read
    for i in 0..20 {
        let _ = updates_tx.send(pb::FeatureUpdate {
            message_id: format!("{i}"),
            action: pb::feature_update::Action::Upsert as i32,
            feature: Some(pb::FeatureFull {
                id: uuid::Uuid::new_v4().to_string(),
                key: "Test Feature".into(),
                description: String::new(),
                feature_type: "Simple".into(),
                team_id: "51ecc366-f1cd-4d3d-ab73-fa60bad98f27".into(),
                created_at: chrono::Utc::now().to_rfc3339(),
                active: true,
                kill_switch_enabled: true,
                kill_switch_activated_at: String::new(),
                rollback_scheduled_at: chrono::Utc::now().to_rfc3339(),
                stages: vec![],
                dependencies: vec![],
                variants: vec![],
            }),
            feature_key: String::new(),
            error: String::new(),
        });
    }
    // Wait a bit to ensure lag is detected in spawned task
    sleep(Duration::from_millis(50)).await;

    // Now read until we see an ERROR with "lagged"
    let mut saw_lag_error = false;
    for _ in 0..50 {
        if let Some(Ok(update)) = stream.message().await.transpose() {
            if update.action == (pb::feature_update::Action::Error as i32)
                && update.error == "lagged"
            {
                saw_lag_error = true;
                break;
            }
        } else {
            break;
        }
    }
    assert!(saw_lag_error, "did not receive lagged error update");
}
