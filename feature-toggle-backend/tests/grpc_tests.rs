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
use std::collections::HashMap;
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

fn test_client(
    id: Uuid,
    team_id: Uuid,
    environment_id: Uuid,
    secret: &str,
    client_type: db::ClientType,
    enabled: bool,
) -> db::Client {
    db::Client {
        id,
        team_id,
        environment_id,
        name: "Client".into(),
        description: None,
        enabled,
        client_type,
        api_key: secret.to_string(),
        web_origins: None,
    }
}

fn test_feature(
    id: Uuid,
    key: &str,
    team_id: Uuid,
    active: bool,
    kill_switch_enabled: bool,
    dependencies: Vec<db::FeatureDependency>,
) -> db::Feature {
    db::Feature {
        id,
        key: key.to_string(),
        description: Some(String::new()),
        feature_type: db::FeatureType::Simple,
        team_id,
        active,
        created_at: chrono::Utc::now(),
        kill_switch_enabled,
        kill_switch_activated_at: None,
        rollback_scheduled_at: Some(chrono::Utc::now() + chrono::Duration::minutes(30)),
        emergency_override_reason: None,
        emergency_override_expires_at: None,
        emergency_override_actor_id: None,
        emergency_override_applied_at: None,
        lifecycle_stage: "active".to_string(),
        owner: None,
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: vec![],
        archived_at: None,
        deprecated_at: None,
        deprecation_notice: None,
        last_evaluated_at: None,
        evaluation_count_7d: 0,
        evaluation_count_30d: 0,
        evaluation_count_90d: 0,
        dependencies,
    }
}

fn test_stage(feature_id: Uuid, environment_id: Uuid, enabled: bool) -> db::FeaturePipelineStage {
    db::FeaturePipelineStage {
        id: Uuid::new_v4(),
        feature_id,
        environment_id,
        order_index: 0,
        parent_stage_id: None,
        position: "Start".into(),
        enabled,
        status: "NOT_DEPLOYED".into(),
    }
}

/// Builds a `get_feature_stages_batch` mock from a per-feature closure. Like
/// the real repository, features without stages are absent from the map.
fn stages_batch<F>(
    per_feature: F,
) -> impl Fn(&[Uuid]) -> Result<HashMap<Uuid, Vec<db::FeaturePipelineStage>>, Error>
where
    F: Fn(Uuid) -> Result<Vec<db::FeaturePipelineStage>, Error>,
{
    move |feature_ids| batch_by_id(feature_ids, &per_feature)
}

/// Builds a `get_stage_criteria_batch` mock from a per-stage closure.
fn criteria_batch<F>(
    per_stage: F,
) -> impl Fn(Uuid, &[Uuid]) -> Result<HashMap<Uuid, Vec<db::StageCriterion>>, Error>
where
    F: Fn(Uuid) -> Result<Vec<db::StageCriterion>, Error>,
{
    move |_team_id, stage_ids| batch_by_id(stage_ids, &per_stage)
}

/// Builds a `get_feature_variants_batch` mock from a per-feature closure.
fn variants_batch<F>(
    per_feature: F,
) -> impl Fn(&[Uuid]) -> Result<HashMap<Uuid, Vec<db::FeatureVariant>>, Error>
where
    F: Fn(Uuid) -> Result<Vec<db::FeatureVariant>, Error>,
{
    move |feature_ids| batch_by_id(feature_ids, &per_feature)
}

fn batch_by_id<T>(
    ids: &[Uuid],
    per_id: &impl Fn(Uuid) -> Result<Vec<T>, Error>,
) -> Result<HashMap<Uuid, Vec<T>>, Error> {
    let mut out = HashMap::new();
    for id in ids {
        let rows = per_id(*id)?;
        if !rows.is_empty() {
            out.insert(*id, rows);
        }
    }
    Ok(out)
}

/// Builds a `get_feature_by_key` mock from a keyed `get_features` mock closure:
/// the exact-key lookup returns the feature whose key equals the request.
fn exact_key_lookup<F>(features: F) -> impl Fn(Uuid, String) -> Result<Option<db::Feature>, Error>
where
    F: Fn(Uuid, Option<String>, Option<db::FeatureType>) -> Result<Vec<db::Feature>, Error>,
{
    move |team, key| {
        features(team, Some(key.clone()), None)
            .map(|found| found.into_iter().find(|feature| feature.key == key))
    }
}

async fn recv_update_with_timeout(
    stream: &mut tonic::Streaming<pb::FeatureUpdate>,
    timeout: Duration,
) -> Option<pb::FeatureUpdate> {
    match tokio::time::timeout(timeout, stream.message()).await {
        Ok(Ok(Some(update))) => Some(update),
        _ => None,
    }
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
                environment_id: env_id,
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
        .expect_get_feature_stages_batch()
        .returning(stages_batch(move |fid| {
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
        }));
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
                    emergency_override_reason: None,
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: None,
                    lifecycle_stage: "active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    archived_at: None,
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
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

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
                environment_id: env_id,
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
                environment_id: env_id,
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
        .expect_get_feature_stages_batch()
        .returning(stages_batch(move |fid| {
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
        }));
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(move |_team, key, _ftype| {
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
                    emergency_override_reason: None,
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: None,
                    lifecycle_stage: "active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    archived_at: None,
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
        }));
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

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
async fn evaluate_returns_false_for_kill_switched_feature() {
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == valid_client_id {
            Ok(test_client(
                id,
                team_id,
                env_id,
                &sec_clone,
                db::ClientType::Web,
                true,
            ))
        } else {
            Err(Error::NotFound(id))
        }
    });

    let mut feature_mock = MockFeatureRepository::new();
    let feature_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(move |_team, key, _ftype| {
            match key.as_deref() {
                Some("Test Feature") => Ok(vec![test_feature(
                    feature_id,
                    "Test Feature",
                    team_id,
                    true,
                    false,
                    vec![],
                )]),
                _ => Ok(vec![]),
            }
        }));

    let (tx, _rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), tx).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint).await.unwrap();

    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid,
        client_secret: sec,
    };

    let resp = client.evaluate(req).await.unwrap().into_inner();
    assert!(!resp.enabled);
}

#[tokio::test]
async fn evaluate_returns_false_for_stage_disabled_feature() {
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == valid_client_id {
            Ok(test_client(
                id,
                team_id,
                env_id,
                &sec_clone,
                db::ClientType::Web,
                true,
            ))
        } else {
            Err(Error::NotFound(id))
        }
    });

    let mut feature_mock = MockFeatureRepository::new();
    let feature_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(move |_team, key, _ftype| {
            match key.as_deref() {
                Some("Test Feature") => Ok(vec![test_feature(
                    feature_id,
                    "Test Feature",
                    team_id,
                    true,
                    true,
                    vec![],
                )]),
                _ => Ok(vec![]),
            }
        }));
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(move |id| {
            if id == feature_id {
                Ok(vec![test_stage(feature_id, env_id, false)])
            } else {
                Ok(vec![])
            }
        }));
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

    let (tx, _rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), tx).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint).await.unwrap();

    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![],
        feature_id: String::new(),
        client_id: cid,
        client_secret: sec,
    };

    let resp = client.evaluate(req).await.unwrap().into_inner();
    assert!(!resp.enabled);
}

#[tokio::test]
async fn evaluate_returns_false_for_dependency_disabled_by_kill_switch() {
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;
    let root_id = Uuid::new_v4();
    let dependency_id = Uuid::new_v4();

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == valid_client_id {
            Ok(test_client(
                id,
                team_id,
                env_id,
                &sec_clone,
                db::ClientType::Web,
                true,
            ))
        } else {
            Err(Error::NotFound(id))
        }
    });

    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(move |_team, key, _ftype| {
            match key.as_deref() {
                Some("Test Feature") => Ok(vec![test_feature(
                    root_id,
                    "Test Feature",
                    team_id,
                    true,
                    true,
                    vec![db::FeatureDependency {
                        feature_id: root_id,
                        depends_on_id: dependency_id,
                    }],
                )]),
                _ => Ok(vec![]),
            }
        }));
    feature_mock
        .expect_get_feature_by_id()
        .returning(move |id| {
            if id == dependency_id {
                Ok(test_feature(
                    dependency_id,
                    "Dependency Feature",
                    team_id,
                    true,
                    false,
                    vec![],
                ))
            } else {
                Err(Error::NotFound(id))
            }
        });
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(move |id| {
            if id == root_id || id == dependency_id {
                Ok(vec![test_stage(id, env_id, true)])
            } else {
                Ok(vec![])
            }
        }));
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

    let (tx, _rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), tx).await;
    let endpoint = format!("http://{}", addr);
    let mut client = FeatureEvaluationClient::connect(endpoint).await.unwrap();

    let req = EvaluateRequest {
        feature_key: "Test Feature".into(),
        environment_id: valid_env_id(),
        context: vec![pb::Context {
            key: "bucketingKey".into(),
            value: "user-1".into(),
        }],
        feature_id: String::new(),
        client_id: cid,
        client_secret: sec,
    };

    let resp = client.evaluate(req).await.unwrap().into_inner();
    assert!(!resp.enabled);
}

#[tokio::test]
async fn get_feature_by_key_and_stream_branches() {
    use chrono::{Duration as ChronoDuration, Utc};
    // Keep this small enough to induce lag later, but large enough for normal assertions.
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);

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
                environment_id: env_id,
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
        .expect_get_feature_stages_batch()
        .returning(stages_batch(move |fid| {
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
        }));
    let features_by_key =
        move |_team: Uuid, key: Option<String>, _ftype: Option<db::FeatureType>| {
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
                    emergency_override_reason: None,
                    emergency_override_expires_at: None,
                    emergency_override_actor_id: None,
                    emergency_override_applied_at: None,
                    lifecycle_stage: "active".to_string(),
                    owner: None,
                    purpose: None,
                    reference_url: None,
                    expires_at: None,
                    cleanup_reason: None,
                    tags: vec![],
                    archived_at: None,
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
        };
    feature_mock
        .expect_get_features()
        .returning(features_by_key);
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(features_by_key));
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

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

    // Subscribe success with explicit key: should emit snapshot for that key
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
                    feature_keys: vec!["Test Feature".into()],
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
        if let Some(update) =
            recv_update_with_timeout(&mut stream, Duration::from_millis(500)).await
            && update.action == (pb::feature_update::Action::Snapshot as i32)
        {
            assert!(update.feature.as_ref().map(|f| f.key.as_str()) == Some("Test Feature"));
            got_snapshot = true;
            break;
        }
    }
    assert!(got_snapshot, "did not receive snapshot");

    // Now expect a heartbeat update at some point
    let mut got_heartbeat = false;
    for _ in 0..10 {
        if let Some(update) =
            recv_update_with_timeout(&mut stream, Duration::from_millis(500)).await
            && update.action == (pb::feature_update::Action::Heartbeat as i32)
        {
            got_heartbeat = true;
            break;
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
        if let Some(update) =
            recv_update_with_timeout(&mut stream, Duration::from_millis(500)).await
            && update.action == (pb::feature_update::Action::Upsert as i32)
            && update.feature.as_ref().map(|f| f.key.as_str()) == Some("Test Feature")
        {
            got_matching = true;
            break;
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
        if let Some(update) =
            recv_update_with_timeout(&mut stream, Duration::from_millis(250)).await
        {
            if update.action == (pb::feature_update::Action::Error as i32)
                && update.error == "lagged"
            {
                saw_lag_error = true;
                break;
            }
        }
    }
    assert!(saw_lag_error, "did not receive lagged error update");
}

#[tokio::test]
async fn stream_empty_subscription_sends_full_snapshot() {
    use chrono::{Duration as ChronoDuration, Utc};

    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == valid_client_id {
            Ok(db::Client {
                id,
                team_id,
                environment_id: env_id,
                name: "Client".into(),
                description: None,
                enabled: true,
                client_type: db::ClientType::Web,
                api_key: sec_clone.clone(),
                web_origins: None,
            })
        } else {
            Err(Error::NotFound(id))
        }
    });

    let mut feature_mock = MockFeatureRepository::new();
    let feature_a_id = Uuid::new_v4();
    let feature_b_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    feature_mock
        .expect_get_features()
        .returning(move |_team, key, _ftype| {
            let build_feature = |id: Uuid, key: &str| db::Feature {
                id,
                key: key.to_string(),
                description: Some(String::new()),
                feature_type: db::FeatureType::Simple,
                team_id,
                active: true,
                created_at: Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(10)),
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: "active".to_string(),
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: vec![],
                archived_at: None,
                deprecated_at: None,
                deprecation_notice: None,
                last_evaluated_at: None,
                evaluation_count_7d: 0,
                evaluation_count_30d: 0,
                evaluation_count_90d: 0,
                dependencies: vec![],
            };

            match key.as_deref() {
                None => Ok(vec![
                    build_feature(feature_a_id, "feature-A"),
                    build_feature(feature_b_id, "feature-B"),
                ]),
                Some("feature-A") => Ok(vec![build_feature(feature_a_id, "feature-A")]),
                Some("feature-B") => Ok(vec![build_feature(feature_b_id, "feature-B")]),
                _ => Ok(vec![]),
            }
        });
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;
    let endpoint = format!("http://{}", addr);

    let mut raw = FeatureEvaluationClient::connect(endpoint).await.unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    tx.send(StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(
            pb::SubscribeRequest {
                client_id: cid,
                client_secret: sec,
                feature_keys: vec![],
                environment_id: String::new(),
            },
        )),
    })
    .await
    .unwrap();

    let mut stream = raw
        .stream_updates(ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();

    let mut keys = std::collections::HashSet::new();
    for _ in 0..10 {
        if let Some(update) =
            recv_update_with_timeout(&mut stream, Duration::from_millis(400)).await
            && update.action == (pb::feature_update::Action::Snapshot as i32)
            && let Some(feature) = update.feature
        {
            keys.insert(feature.key);
            if keys.contains("feature-A") && keys.contains("feature-B") {
                break;
            }
        }
    }

    assert!(
        keys.contains("feature-A"),
        "missing feature-A from snapshot"
    );
    assert!(
        keys.contains("feature-B"),
        "missing feature-B from snapshot"
    );
}

#[tokio::test]
async fn stream_subscriptions_are_connection_scoped() {
    use chrono::{Duration as ChronoDuration, Utc};

    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(16);
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == valid_client_id {
            Ok(db::Client {
                id,
                team_id,
                environment_id: env_id,
                name: "Client".into(),
                description: None,
                enabled: true,
                client_type: db::ClientType::Web,
                api_key: sec_clone.clone(),
                web_origins: None,
            })
        } else {
            Err(Error::NotFound(id))
        }
    });

    let mut feature_mock = MockFeatureRepository::new();
    let feature_a_id = Uuid::new_v4();
    let feature_b_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    let features_by_key =
        move |_team: Uuid, key: Option<String>, _ftype: Option<db::FeatureType>| match key
            .as_deref()
        {
            Some("feature-A") => Ok(vec![db::Feature {
                id: feature_a_id,
                key: "feature-A".into(),
                description: Some(String::new()),
                feature_type: db::FeatureType::Simple,
                team_id,
                active: true,
                created_at: Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(10)),
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: "active".to_string(),
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: vec![],
                archived_at: None,
                deprecated_at: None,
                deprecation_notice: None,
                last_evaluated_at: None,
                evaluation_count_7d: 0,
                evaluation_count_30d: 0,
                evaluation_count_90d: 0,
                dependencies: vec![],
            }]),
            Some("feature-B") => Ok(vec![db::Feature {
                id: feature_b_id,
                key: "feature-B".into(),
                description: Some(String::new()),
                feature_type: db::FeatureType::Simple,
                team_id,
                active: true,
                created_at: Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(10)),
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: "active".to_string(),
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: vec![],
                archived_at: None,
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
    feature_mock
        .expect_get_features()
        .returning(features_by_key);
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(features_by_key));
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

    let (addr, _server) = start_server_with_repos(
        Box::new(feature_mock),
        Box::new(client_mock),
        updates_tx.clone(),
    )
    .await;
    let endpoint = format!("http://{}", addr);

    // First stream subscribes to feature-A and then disconnects.
    let mut raw_a = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    tx_a.send(StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(
            pb::SubscribeRequest {
                client_id: cid.clone(),
                client_secret: sec.clone(),
                feature_keys: vec!["feature-A".into()],
                environment_id: String::new(),
            },
        )),
    })
    .await
    .unwrap();
    let mut stream_a = raw_a
        .stream_updates(ReceiverStream::new(rx_a))
        .await
        .unwrap()
        .into_inner();
    let _ = tokio::time::timeout(Duration::from_millis(250), stream_a.message()).await;
    drop(stream_a);
    drop(tx_a);
    sleep(Duration::from_millis(50)).await;

    // Second stream for the same client subscribes only to feature-B.
    let mut raw_b = FeatureEvaluationClient::connect(endpoint).await.unwrap();
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    tx_b.send(StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(
            pb::SubscribeRequest {
                client_id: cid,
                client_secret: sec,
                feature_keys: vec!["feature-B".into()],
                environment_id: String::new(),
            },
        )),
    })
    .await
    .unwrap();
    let mut stream_b = raw_b
        .stream_updates(ReceiverStream::new(rx_b))
        .await
        .unwrap()
        .into_inner();
    let _ = tokio::time::timeout(Duration::from_millis(250), stream_b.message()).await;

    // Update for feature-A should not leak into second stream.
    updates_tx
        .send(pb::FeatureUpdate {
            message_id: Uuid::new_v4().to_string(),
            action: pb::feature_update::Action::Upsert as i32,
            feature: Some(pb::FeatureFull {
                id: Uuid::new_v4().to_string(),
                key: "feature-A".into(),
                description: String::new(),
                feature_type: "Simple".into(),
                team_id: team_id.to_string(),
                created_at: Utc::now().to_rfc3339(),
                active: true,
                kill_switch_enabled: true,
                kill_switch_activated_at: String::new(),
                rollback_scheduled_at: Utc::now().to_rfc3339(),
                stages: vec![],
                dependencies: vec![],
                variants: vec![],
            }),
            feature_key: String::new(),
            error: String::new(),
        })
        .unwrap();

    let mut leaked_a = false;
    for _ in 0..3 {
        if let Ok(Ok(Some(update))) =
            tokio::time::timeout(Duration::from_millis(150), stream_b.message()).await
            && update.action == (pb::feature_update::Action::Upsert as i32)
            && update.feature.as_ref().map(|f| f.key.as_str()) == Some("feature-A")
        {
            leaked_a = true;
            break;
        }
    }
    assert!(
        !leaked_a,
        "feature-A update leaked into stream subscribed only to feature-B"
    );

    updates_tx
        .send(pb::FeatureUpdate {
            message_id: Uuid::new_v4().to_string(),
            action: pb::feature_update::Action::Upsert as i32,
            feature: Some(pb::FeatureFull {
                id: Uuid::new_v4().to_string(),
                key: "feature-B".into(),
                description: String::new(),
                feature_type: "Simple".into(),
                team_id: team_id.to_string(),
                created_at: Utc::now().to_rfc3339(),
                active: true,
                kill_switch_enabled: true,
                kill_switch_activated_at: String::new(),
                rollback_scheduled_at: Utc::now().to_rfc3339(),
                stages: vec![],
                dependencies: vec![],
                variants: vec![],
            }),
            feature_key: String::new(),
            error: String::new(),
        })
        .unwrap();

    let mut got_b = false;
    for _ in 0..5 {
        if let Ok(Ok(Some(update))) =
            tokio::time::timeout(Duration::from_millis(200), stream_b.message()).await
            && update.action == (pb::feature_update::Action::Upsert as i32)
            && update.feature.as_ref().map(|f| f.key.as_str()) == Some("feature-B")
        {
            got_b = true;
            break;
        }
    }
    assert!(got_b, "did not receive feature-B update");
}

#[tokio::test]
async fn requested_keys_are_cleared_when_last_stream_disconnects() {
    use chrono::{Duration as ChronoDuration, Utc};

    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(16);
    let (cid, sec) = client_ids();
    let valid_client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let env_id = team_id;

    let mut client_mock = MockClientRepository::new();
    let sec_clone = sec.clone();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == valid_client_id {
            Ok(db::Client {
                id,
                team_id,
                environment_id: env_id,
                name: "Client".into(),
                description: None,
                enabled: true,
                client_type: db::ClientType::Web,
                api_key: sec_clone.clone(),
                web_origins: None,
            })
        } else {
            Err(Error::NotFound(id))
        }
    });

    let mut feature_mock = MockFeatureRepository::new();
    let feature_a_id = Uuid::new_v4();
    let feature_b_id = Uuid::new_v4();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    let features_by_key =
        move |_team: Uuid, key: Option<String>, _ftype: Option<db::FeatureType>| match key
            .as_deref()
        {
            Some("feature-A") => Ok(vec![db::Feature {
                id: feature_a_id,
                key: "feature-A".into(),
                description: Some(String::new()),
                feature_type: db::FeatureType::Simple,
                team_id,
                active: true,
                created_at: Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(10)),
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: "active".to_string(),
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: vec![],
                archived_at: None,
                deprecated_at: None,
                deprecation_notice: None,
                last_evaluated_at: None,
                evaluation_count_7d: 0,
                evaluation_count_30d: 0,
                evaluation_count_90d: 0,
                dependencies: vec![],
            }]),
            Some("feature-B") => Ok(vec![db::Feature {
                id: feature_b_id,
                key: "feature-B".into(),
                description: Some(String::new()),
                feature_type: db::FeatureType::Simple,
                team_id,
                active: true,
                created_at: Utc::now(),
                kill_switch_enabled: true,
                kill_switch_activated_at: None,
                rollback_scheduled_at: Some(Utc::now() + ChronoDuration::minutes(10)),
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: "active".to_string(),
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: vec![],
                archived_at: None,
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
    feature_mock
        .expect_get_features()
        .returning(features_by_key);
    feature_mock
        .expect_get_feature_by_key()
        .returning(exact_key_lookup(features_by_key));
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(|_sid| Ok(Vec::new())));

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;
    let endpoint = format!("http://{}", addr);

    let mut unary = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let unary_response = unary
        .get_feature_by_key(GetFeatureByKeyRequest {
            feature_key: "feature-A".into(),
            client_id: cid.clone(),
            client_secret: sec.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(unary_response.feature.is_some());

    let mut raw_a = FeatureEvaluationClient::connect(endpoint.clone())
        .await
        .unwrap();
    let (tx_a, rx_a) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    tx_a.send(StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(
            pb::SubscribeRequest {
                client_id: cid.clone(),
                client_secret: sec.clone(),
                feature_keys: vec!["feature-B".into()],
                environment_id: String::new(),
            },
        )),
    })
    .await
    .unwrap();
    let mut stream_a = raw_a
        .stream_updates(ReceiverStream::new(rx_a))
        .await
        .unwrap()
        .into_inner();

    let mut first_snapshot_keys = std::collections::HashSet::new();
    for _ in 0..5 {
        if let Some(update) =
            recv_update_with_timeout(&mut stream_a, Duration::from_millis(250)).await
            && update.action == (pb::feature_update::Action::Snapshot as i32)
            && let Some(feature) = update.feature
        {
            first_snapshot_keys.insert(feature.key);
            if first_snapshot_keys.contains("feature-A")
                && first_snapshot_keys.contains("feature-B")
            {
                break;
            }
        }
    }
    assert!(first_snapshot_keys.contains("feature-A"));
    assert!(first_snapshot_keys.contains("feature-B"));

    drop(stream_a);
    drop(tx_a);
    sleep(Duration::from_millis(100)).await;

    let mut raw_b = FeatureEvaluationClient::connect(endpoint).await.unwrap();
    let (tx_b, rx_b) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    tx_b.send(StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(
            pb::SubscribeRequest {
                client_id: cid,
                client_secret: sec,
                feature_keys: vec!["feature-B".into()],
                environment_id: String::new(),
            },
        )),
    })
    .await
    .unwrap();
    let mut stream_b = raw_b
        .stream_updates(ReceiverStream::new(rx_b))
        .await
        .unwrap()
        .into_inner();

    let mut second_snapshot_keys = std::collections::HashSet::new();
    for _ in 0..5 {
        if let Some(update) =
            recv_update_with_timeout(&mut stream_b, Duration::from_millis(250)).await
            && update.action == (pb::feature_update::Action::Snapshot as i32)
            && let Some(feature) = update.feature
        {
            second_snapshot_keys.insert(feature.key);
        }
    }

    assert!(
        !second_snapshot_keys.contains("feature-A"),
        "stale unary requested key leaked into a fresh stream after disconnect"
    );
    assert!(second_snapshot_keys.contains("feature-B"));
}

fn stream_client_mock(client_id: Uuid, team_id: Uuid, secret: String) -> MockClientRepository {
    let mut client_mock = MockClientRepository::new();
    client_mock.expect_get_client_by_id().returning(move |id| {
        if id == client_id {
            Ok(test_client(
                id,
                team_id,
                team_id,
                &secret,
                db::ClientType::Web,
                true,
            ))
        } else {
            Err(Error::NotFound(id))
        }
    });
    client_mock
}

async fn open_update_stream(
    addr: SocketAddr,
    client_id: String,
    client_secret: String,
    feature_keys: Vec<String>,
) -> (
    tonic::Streaming<pb::FeatureUpdate>,
    tokio::sync::mpsc::Sender<pb::StreamRequest>,
) {
    let mut raw = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamRequest>(16);
    tx.send(StreamRequest {
        payload: Some(pb::stream_request::Payload::Subscribe(
            pb::SubscribeRequest {
                client_id,
                client_secret,
                feature_keys,
                environment_id: String::new(),
            },
        )),
    })
    .await
    .unwrap();
    let stream = raw
        .stream_updates(ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    (stream, tx)
}

fn upsert_for(key: &str, team_id: Uuid) -> pb::FeatureUpdate {
    pb::FeatureUpdate {
        message_id: Uuid::new_v4().to_string(),
        action: pb::feature_update::Action::Upsert as i32,
        feature: Some(pb::FeatureFull {
            id: Uuid::new_v4().to_string(),
            key: key.to_string(),
            team_id: team_id.to_string(),
            feature_type: "Simple".into(),
            active: true,
            ..Default::default()
        }),
        feature_key: String::new(),
        error: String::new(),
    }
}

#[tokio::test]
async fn stream_snapshot_larger_than_channel_capacity_completes() {
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::new_v4();
    let client_mock = stream_client_mock(client_id, team_id, sec.clone());

    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    feature_mock
        .expect_get_features()
        .returning(move |_team, _key, _ftype| {
            Ok((0..100)
                .map(|i| {
                    test_feature(
                        Uuid::new_v4(),
                        &format!("bulk-{i:03}"),
                        team_id,
                        true,
                        false,
                        vec![],
                    )
                })
                .collect())
        });

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;

    let (mut stream, _tx) = tokio::time::timeout(
        Duration::from_secs(5),
        open_update_stream(addr, cid, sec, vec![]),
    )
    .await
    .expect("stream_updates did not return within 5s for a 100-feature snapshot");

    let mut keys = std::collections::HashSet::new();
    while keys.len() < 100 {
        let update = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
            .await
            .expect("snapshot stream ended before all 100 features arrived");
        assert_eq!(update.action, pb::feature_update::Action::Snapshot as i32);
        keys.insert(update.feature.unwrap().key);
    }
    assert_eq!(keys.len(), 100);
}

#[tokio::test]
async fn stream_forwards_update_broadcast_during_snapshot_read() {
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(16);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::new_v4();
    let client_mock = stream_client_mock(client_id, team_id, sec.clone());

    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    let tx_clone = updates_tx.clone();
    feature_mock
        .expect_get_features()
        .returning(move |_team, _key, _ftype| {
            // An operator change lands while the snapshot is being read.
            let _ = tx_clone.send(upsert_for("x", team_id));
            Ok(vec![test_feature(
                Uuid::new_v4(),
                "snapshot-feature",
                team_id,
                true,
                false,
                vec![],
            )])
        });

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;

    let (mut stream, _tx) = tokio::time::timeout(
        Duration::from_secs(5),
        open_update_stream(addr, cid, sec, vec![]),
    )
    .await
    .expect("stream_updates did not return");

    let mut saw_upsert_x = false;
    for _ in 0..5 {
        let Some(update) = recv_update_with_timeout(&mut stream, Duration::from_millis(500)).await
        else {
            break;
        };
        if update.action == pb::feature_update::Action::Upsert as i32
            && update.feature.as_ref().map(|f| f.key.as_str()) == Some("x")
        {
            saw_upsert_x = true;
            break;
        }
    }
    assert!(
        saw_upsert_x,
        "update broadcast during the snapshot read was lost"
    );
}

#[tokio::test]
async fn stream_drops_upserts_from_other_teams() {
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(16);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let team_a = Uuid::new_v4();
    let team_b = Uuid::new_v4();
    let client_mock = stream_client_mock(client_id, team_a, sec.clone());

    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    feature_mock
        .expect_get_features()
        .returning(move |_team, _key, _ftype| {
            Ok(vec![test_feature(
                Uuid::new_v4(),
                "shared",
                team_a,
                true,
                false,
                vec![],
            )])
        });

    let (addr, _server) = start_server_with_repos(
        Box::new(feature_mock),
        Box::new(client_mock),
        updates_tx.clone(),
    )
    .await;

    // Subscribe as Team A for all features and drain the snapshot.
    let (mut stream, _tx) = open_update_stream(addr, cid, sec, vec![]).await;
    let snapshot = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
        .await
        .expect("missing snapshot");
    assert_eq!(snapshot.action, pb::feature_update::Action::Snapshot as i32);
    assert_eq!(snapshot.feature.unwrap().team_id, team_a.to_string());

    // Team B changes its flag with the same key: Team A's stream must not see it.
    updates_tx.send(upsert_for("shared", team_b)).unwrap();
    let leaked = recv_update_with_timeout(&mut stream, Duration::from_millis(300)).await;
    assert!(
        leaked.is_none(),
        "Team A stream received Team B's update: {leaked:?}"
    );

    // Control: Team A's own update still arrives.
    updates_tx.send(upsert_for("shared", team_a)).unwrap();
    let own = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
        .await
        .expect("Team A update was not forwarded");
    assert_eq!(own.action, pb::feature_update::Action::Upsert as i32);
    let feature = own.feature.unwrap();
    assert_eq!(feature.key, "shared");
    assert_eq!(feature.team_id, team_a.to_string());
}

#[tokio::test]
async fn get_feature_by_key_returns_exact_key_match() {
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::new_v4();
    let client_mock = stream_client_mock(client_id, team_id, sec.clone());

    let checkout_id = Uuid::new_v4();
    let checkout_v2_id = Uuid::new_v4();
    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    // The list/search query matches substrings, ordered by key.
    feature_mock
        .expect_get_features()
        .returning(move |_team, _key, _ftype| {
            Ok(vec![
                test_feature(checkout_id, "checkout", team_id, true, true, vec![]),
                test_feature(checkout_v2_id, "checkout-v2", team_id, true, true, vec![]),
            ])
        });
    feature_mock
        .expect_get_feature_by_key()
        .returning(move |team, key| {
            assert_eq!(team, team_id);
            Ok((key == "checkout")
                .then(|| test_feature(checkout_id, "checkout", team_id, true, true, vec![])))
        });

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;
    let mut client = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .unwrap();

    let response = client
        .get_feature_by_key(GetFeatureByKeyRequest {
            client_id: cid,
            client_secret: sec,
            feature_key: "checkout".into(),
        })
        .await
        .unwrap()
        .into_inner();

    let feature = response.feature.expect("checkout should be found");
    assert_eq!(feature.key, "checkout");
    assert_eq!(feature.id, checkout_id.to_string());
}

#[tokio::test]
async fn stream_keys_snapshot_sends_delete_for_missing_key() {
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let team_id = Uuid::new_v4();
    let client_mock = stream_client_mock(client_id, team_id, sec.clone());

    let present_id = Uuid::new_v4();
    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    feature_mock
        .expect_get_feature_by_key()
        .returning(move |_team, key| match key.as_str() {
            "present" => Ok(Some(test_feature(
                present_id,
                "present",
                team_id,
                true,
                false,
                vec![],
            ))),
            _ => Ok(None),
        });

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;

    // An edge reconnecting with cached keys, one of which no longer exists.
    let (mut stream, _tx) = open_update_stream(
        addr,
        cid,
        sec,
        vec!["gone".to_string(), "present".to_string()],
    )
    .await;

    let mut received = Vec::new();
    while let Some(update) = recv_update_with_timeout(&mut stream, Duration::from_millis(500)).await
    {
        received.push(update);
    }

    let delete_pos = received
        .iter()
        .position(|u| {
            u.action == pb::feature_update::Action::Delete as i32 && u.feature_key == "gone"
        })
        .unwrap_or_else(|| panic!("no Delete for missing key 'gone' in {received:?}"));
    assert!(
        received[delete_pos].feature.is_none(),
        "Delete must reach the edge without a feature payload"
    );
    let snapshot_pos = received
        .iter()
        .position(|u| {
            u.action == pb::feature_update::Action::Snapshot as i32
                && u.feature.as_ref().map(|f| f.key.as_str()) == Some("present")
        })
        .expect("existing key should still arrive as a Snapshot");
    // Deletes go first: after a rename, a later Delete for the old key would
    // otherwise drop the edge's id index entry for the renamed feature.
    assert!(delete_pos < snapshot_pos);
    assert!(
        !received.iter().any(|u| {
            u.action == pb::feature_update::Action::Delete as i32 && u.feature_key == "present"
        }),
        "existing key must not be deleted"
    );
}

#[tokio::test]
async fn stream_forwards_deletes_only_to_owning_team() {
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(16);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let team_a = Uuid::new_v4();
    let team_b = Uuid::new_v4();
    let client_mock = stream_client_mock(client_id, team_a, sec.clone());

    let mut feature_mock = MockFeatureRepository::new();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(|_fid| Ok(Vec::new())));
    feature_mock
        .expect_get_features()
        .returning(move |_team, _key, _ftype| {
            Ok(vec![test_feature(
                Uuid::new_v4(),
                "shared",
                team_a,
                true,
                false,
                vec![],
            )])
        });

    let (addr, _server) = start_server_with_repos(
        Box::new(feature_mock),
        Box::new(client_mock),
        updates_tx.clone(),
    )
    .await;

    // Subscribe as Team A for all features and drain the snapshot.
    let (mut stream, _tx) = open_update_stream(addr, cid, sec, vec![]).await;
    let snapshot = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
        .await
        .expect("missing snapshot");
    assert_eq!(snapshot.action, pb::feature_update::Action::Snapshot as i32);

    // Team B renames its own "shared": Team A must keep its flag.
    updates_tx
        .send(feature_toggle_backend::grpc::team_scoped_delete(
            team_b, "shared",
        ))
        .unwrap();
    // A Delete with no owning team cannot be scoped, so it is dropped.
    updates_tx
        .send(pb::FeatureUpdate {
            message_id: Uuid::new_v4().to_string(),
            action: pb::feature_update::Action::Delete as i32,
            feature: None,
            feature_key: "shared".to_string(),
            error: String::new(),
        })
        .unwrap();
    let leaked = recv_update_with_timeout(&mut stream, Duration::from_millis(300)).await;
    assert!(
        leaked.is_none(),
        "Team A stream received a Delete it does not own: {leaked:?}"
    );

    // Control: Team A's own Delete arrives as a plain proto Delete.
    updates_tx
        .send(feature_toggle_backend::grpc::team_scoped_delete(
            team_a, "shared",
        ))
        .unwrap();
    let own = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
        .await
        .expect("Team A Delete was not forwarded");
    assert_eq!(own.action, pb::feature_update::Action::Delete as i32);
    assert_eq!(own.feature_key, "shared");
    assert!(
        own.feature.is_none(),
        "internal team tag must not reach the edge"
    );
}

/// Feature data for the mapping pin test: three features (one Contextual,
/// one depending on another), several stages, criteria with rule groups,
/// conditions, allocations and variants.
#[derive(Clone)]
struct MappingFixture {
    team_id: Uuid,
    env_1: Uuid,
    env_2: Uuid,
    features: Vec<db::Feature>,
    stages: std::collections::HashMap<Uuid, Vec<db::FeaturePipelineStage>>,
    criteria: std::collections::HashMap<Uuid, Vec<db::StageCriterion>>,
    variants: std::collections::HashMap<Uuid, Vec<db::FeatureVariant>>,
}

fn fixed_uuid(n: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | n)
}

fn mapping_fixture() -> MappingFixture {
    use chrono::TimeZone;
    let team_id = fixed_uuid(1);
    let env_1 = fixed_uuid(2);
    let env_2 = fixed_uuid(3);
    let created_at = chrono::Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();

    let feature_a = fixed_uuid(10);
    let feature_b = fixed_uuid(11);
    let feature_d = fixed_uuid(12);
    let build = |id: Uuid, key: &str, feature_type: db::FeatureType, deps: Vec<Uuid>| {
        let mut feature = test_feature(
            id,
            key,
            team_id,
            true,
            true,
            deps.into_iter()
                .map(|depends_on_id| db::FeatureDependency {
                    feature_id: id,
                    depends_on_id,
                })
                .collect(),
        );
        feature.feature_type = feature_type;
        feature.created_at = created_at;
        feature.rollback_scheduled_at = None;
        feature.description = Some(format!("{key} description"));
        feature
    };
    let features = vec![
        build(feature_a, "pin-a", db::FeatureType::Simple, vec![]),
        build(feature_b, "pin-b", db::FeatureType::Contextual, vec![]),
        build(feature_d, "pin-d", db::FeatureType::Simple, vec![feature_a]),
    ];

    let stage = |id: Uuid, feature_id: Uuid, environment_id: Uuid, order_index: i32, enabled| {
        db::FeaturePipelineStage {
            id,
            feature_id,
            environment_id,
            order_index,
            parent_stage_id: None,
            position: format!("{{\"x\":{order_index},\"y\":0}}"),
            enabled,
            status: if enabled { "DEPLOYED" } else { "NOT_DEPLOYED" }.into(),
        }
    };
    let stage_a1 = fixed_uuid(20);
    let stage_a2 = fixed_uuid(21);
    let stage_b1 = fixed_uuid(22);
    let stage_d1 = fixed_uuid(23);
    let stages = std::collections::HashMap::from([
        (
            feature_a,
            vec![
                stage(stage_a1, feature_a, env_1, 0, true),
                stage(stage_a2, feature_a, env_2, 1, false),
            ],
        ),
        (feature_b, vec![stage(stage_b1, feature_b, env_1, 0, true)]),
        (feature_d, vec![stage(stage_d1, feature_d, env_1, 0, true)]),
    ]);

    let condition =
        |n: u128, key: &str, operator: &str, value, order_index| db::CompoundRuleCondition {
            id: fixed_uuid(n),
            context_key: key.into(),
            operator: operator.into(),
            value,
            order_index,
        };
    let criteria = std::collections::HashMap::from([
        (
            stage_a1,
            vec![
                db::StageCriterion {
                    id: fixed_uuid(30),
                    stage_id: stage_a1,
                    priority: 0,
                    rule_groups: vec![
                        db::CompoundRuleGroup {
                            id: fixed_uuid(40),
                            logic_operator: db::LogicOperator::And,
                            conditions: vec![
                                condition(50, "country", "EQUALS", serde_json::json!("US"), 0),
                                condition(51, "plan", "IN", serde_json::json!(["pro", "team"]), 1),
                            ],
                        },
                        db::CompoundRuleGroup {
                            id: fixed_uuid(41),
                            logic_operator: db::LogicOperator::Or,
                            conditions: vec![condition(
                                52,
                                "age",
                                "GREATER_THAN",
                                serde_json::json!(18),
                                0,
                            )],
                        },
                    ],
                    variant_allocations: vec![
                        db::VariantAllocationSimple {
                            variant_control: "off".into(),
                            weight: 40,
                        },
                        db::VariantAllocationSimple {
                            variant_control: "on".into(),
                            weight: 60,
                        },
                    ],
                    variant_selection_mode: db::VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
                },
                db::StageCriterion {
                    id: fixed_uuid(31),
                    stage_id: stage_a1,
                    priority: 1,
                    rule_groups: vec![],
                    variant_allocations: vec![],
                    variant_selection_mode: db::VariantSelectionMode::SpecificVariant,
                    selected_variant_control: Some("on".into()),
                },
            ],
        ),
        (
            stage_b1,
            vec![db::StageCriterion {
                id: fixed_uuid(32),
                stage_id: stage_b1,
                priority: 0,
                rule_groups: vec![db::CompoundRuleGroup {
                    id: fixed_uuid(42),
                    logic_operator: db::LogicOperator::And,
                    conditions: vec![condition(
                        53,
                        "country",
                        "EQUALS",
                        serde_json::json!("DE"),
                        0,
                    )],
                }],
                variant_allocations: vec![],
                variant_selection_mode: db::VariantSelectionMode::SpecificVariant,
                selected_variant_control: Some("blue".into()),
            }],
        ),
    ]);

    let variant = |n: u128, control: &str, value: serde_json::Value| db::FeatureVariant {
        id: fixed_uuid(n),
        feature_id: feature_b,
        control: control.into(),
        value,
        value_type: db::VariantValueType::Json,
        description: None,
        created_at,
        updated_at: created_at,
    };
    let variants = std::collections::HashMap::from([(
        feature_b,
        vec![
            variant(60, "red", serde_json::json!({"r": 1})),
            variant(61, "blue", serde_json::json!("#00f")),
        ],
    )]);

    MappingFixture {
        team_id,
        env_1,
        env_2,
        features,
        stages,
        criteria,
        variants,
    }
}

/// Wires the feature, stage, criteria and variant loaders of the mock to the
/// fixture.
fn mapping_repo_mock(fixture: &MappingFixture) -> MockFeatureRepository {
    let mut feature_mock = MockFeatureRepository::new();

    let features = fixture.features.clone();
    let team_id = fixture.team_id;
    feature_mock
        .expect_get_features()
        .returning(move |team, key, _ftype| {
            assert_eq!(team, team_id);
            assert!(key.is_none());
            Ok(features.clone())
        });
    let features = fixture.features.clone();
    feature_mock
        .expect_get_feature_by_key()
        .returning(move |_team, key| Ok(features.iter().find(|f| f.key == key).cloned()));
    let features = fixture.features.clone();
    feature_mock
        .expect_get_feature_by_id()
        .returning(move |id| {
            features
                .iter()
                .find(|f| f.id == id)
                .cloned()
                .ok_or(Error::NotFound(id))
        });

    let stages = fixture.stages.clone();
    feature_mock
        .expect_get_feature_stages_batch()
        .returning(stages_batch(move |feature_id| {
            Ok(stages.get(&feature_id).cloned().unwrap_or_default())
        }));
    let criteria = fixture.criteria.clone();
    feature_mock
        .expect_get_stage_criteria_batch()
        .returning(criteria_batch(move |stage_id| {
            Ok(criteria.get(&stage_id).cloned().unwrap_or_default())
        }));
    let variants = fixture.variants.clone();
    feature_mock
        .expect_get_feature_variants_batch()
        .returning(variants_batch(move |feature_id| {
            Ok(variants.get(&feature_id).cloned().unwrap_or_default())
        }));

    feature_mock
}

fn expected_mapping_messages(fixture: &MappingFixture) -> Vec<pb::FeatureFull> {
    let team_id = fixture.team_id.to_string();
    let created_at = "2026-01-02T03:04:05+00:00".to_string();
    let condition =
        |n: u128, key: &str, operator: &str, value: &str, order_index| pb::RuleCondition {
            id: fixed_uuid(n).to_string(),
            context_key: key.into(),
            operator: operator.into(),
            value: value.into(),
            order_index,
        };
    let stage = |n: u128, env: Uuid, order_index: i32, enabled, criterias| pb::FeatureStageFull {
        id: fixed_uuid(n).to_string(),
        environment_id: env.to_string(),
        order_index,
        position: format!("{{\"x\":{order_index},\"y\":0}}"),
        enabled,
        criterias,
    };
    let feature =
        |n: u128, key: &str, feature_type: &str, stages, dependencies, variants| pb::FeatureFull {
            id: fixed_uuid(n).to_string(),
            key: key.into(),
            description: format!("{key} description"),
            feature_type: feature_type.into(),
            team_id: team_id.clone(),
            created_at: created_at.clone(),
            kill_switch_enabled: true,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: String::new(),
            stages,
            dependencies,
            active: true,
            variants,
        };

    vec![
        feature(
            10,
            "pin-a",
            "Simple",
            vec![
                stage(
                    20,
                    fixture.env_1,
                    0,
                    true,
                    vec![
                        pb::StageCriterionFull {
                            id: fixed_uuid(30).to_string(),
                            stage_id: fixed_uuid(20).to_string(),
                            priority: 0,
                            rule_groups: vec![
                                pb::RuleGroup {
                                    id: fixed_uuid(40).to_string(),
                                    logic_operator: "AND".into(),
                                    conditions: vec![
                                        condition(50, "country", "EQUALS", "\"US\"", 0),
                                        condition(51, "plan", "IN", "[\"pro\",\"team\"]", 1),
                                    ],
                                },
                                pb::RuleGroup {
                                    id: fixed_uuid(41).to_string(),
                                    logic_operator: "OR".into(),
                                    conditions: vec![condition(52, "age", "GREATER_THAN", "18", 0)],
                                },
                            ],
                            variant_allocations: vec![
                                pb::VariantAllocation {
                                    variant_control: "off".into(),
                                    weight: 40,
                                },
                                pb::VariantAllocation {
                                    variant_control: "on".into(),
                                    weight: 60,
                                },
                            ],
                            variant_selection_mode: "WEIGHTED_SPLIT".into(),
                            selected_variant_control: String::new(),
                        },
                        pb::StageCriterionFull {
                            id: fixed_uuid(31).to_string(),
                            stage_id: fixed_uuid(20).to_string(),
                            priority: 1,
                            rule_groups: vec![],
                            variant_allocations: vec![],
                            variant_selection_mode: "SPECIFIC_VARIANT".into(),
                            selected_variant_control: "on".into(),
                        },
                    ],
                ),
                stage(21, fixture.env_2, 1, false, vec![]),
            ],
            vec![],
            vec![],
        ),
        feature(
            11,
            "pin-b",
            "Contextual",
            vec![stage(
                22,
                fixture.env_1,
                0,
                true,
                vec![pb::StageCriterionFull {
                    id: fixed_uuid(32).to_string(),
                    stage_id: fixed_uuid(22).to_string(),
                    priority: 0,
                    rule_groups: vec![pb::RuleGroup {
                        id: fixed_uuid(42).to_string(),
                        logic_operator: "AND".into(),
                        conditions: vec![condition(53, "country", "EQUALS", "\"DE\"", 0)],
                    }],
                    variant_allocations: vec![],
                    variant_selection_mode: "SPECIFIC_VARIANT".into(),
                    selected_variant_control: "blue".into(),
                }],
            )],
            vec![],
            vec![
                pb::FeatureVariant {
                    control: "red".into(),
                    value: "{\"r\":1}".into(),
                },
                pb::FeatureVariant {
                    control: "blue".into(),
                    value: "\"#00f\"".into(),
                },
            ],
        ),
        feature(
            12,
            "pin-d",
            "Simple",
            vec![stage(23, fixture.env_1, 0, true, vec![])],
            vec![pb::FeatureDependencyFull {
                feature_id: fixed_uuid(12).to_string(),
                depends_on_id: fixed_uuid(10).to_string(),
            }],
            vec![],
        ),
    ]
}

/// Pins the exact snapshot, GetFeatureByKey and Evaluate output of the
/// feature mapping, so batching the child-row loads (P02) cannot change it.
#[tokio::test]
async fn feature_mapping_output_is_pinned() {
    let fixture = mapping_fixture();
    let expected = expected_mapping_messages(&fixture);
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let client_mock = stream_client_mock(client_id, fixture.team_id, sec.clone());

    let (addr, _server) = start_server_with_repos(
        Box::new(mapping_repo_mock(&fixture)),
        Box::new(client_mock),
        updates_tx,
    )
    .await;

    // Full snapshot
    let (mut stream, _tx) = open_update_stream(addr, cid.clone(), sec.clone(), vec![]).await;
    let mut snapshot = Vec::new();
    while snapshot.len() < expected.len() {
        let update = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
            .await
            .expect("snapshot ended early");
        assert_eq!(update.action, pb::feature_update::Action::Snapshot as i32);
        snapshot.push(update.feature.expect("snapshot carries a feature"));
    }
    assert_eq!(snapshot, expected);

    // GetFeatureByKey
    let mut client = FeatureEvaluationClient::connect(format!("http://{}", addr))
        .await
        .unwrap();
    for expected_feature in &expected {
        let response = client
            .get_feature_by_key(GetFeatureByKeyRequest {
                client_id: cid.clone(),
                client_secret: sec.clone(),
                feature_key: expected_feature.key.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.feature.as_ref(), Some(expected_feature));
    }

    // Evaluate, including the dependency graph of pin-d
    let context = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(key, value)| pb::Context {
                key: (*key).into(),
                value: (*value).into(),
            })
            .collect::<Vec<_>>()
    };
    let cases = [
        (
            "pin-a",
            fixture.env_1,
            context(&[("bucketingKey", "u1"), ("country", "US"), ("plan", "pro")]),
        ),
        (
            "pin-a",
            fixture.env_1,
            context(&[("bucketingKey", "u2"), ("country", "FR")]),
        ),
        (
            "pin-a",
            fixture.env_2,
            context(&[("bucketingKey", "u1"), ("country", "US"), ("plan", "pro")]),
        ),
        (
            "pin-b",
            fixture.env_1,
            context(&[("bucketingKey", "u1"), ("country", "DE")]),
        ),
        (
            "pin-b",
            fixture.env_1,
            context(&[("bucketingKey", "u1"), ("country", "US")]),
        ),
        (
            "pin-d",
            fixture.env_1,
            context(&[("bucketingKey", "u1"), ("country", "US"), ("plan", "pro")]),
        ),
        ("pin-d", fixture.env_2, context(&[("bucketingKey", "u1")])),
    ];
    let mut results = Vec::new();
    for (key, env, context) in cases {
        let response = client
            .evaluate(EvaluateRequest {
                client_id: cid.clone(),
                client_secret: sec.clone(),
                feature_key: key.into(),
                environment_id: env.to_string(),
                context,
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        results.push(response.enabled);
    }
    // Values recorded from the per-stage implementation before P02.
    assert_eq!(
        results,
        vec![true, true, false, false, false, true, false],
        "evaluate results"
    );
}

/// A snapshot loads stages, criteria and variants with one batched call each,
/// however many features and stages it covers. The per-id loaders have no
/// expectation, so calling them would fail the stream.
#[tokio::test]
async fn stream_snapshot_loads_child_rows_with_one_call_per_kind() {
    let fixture = mapping_fixture();
    let expected = expected_mapping_messages(&fixture);
    let (updates_tx, _updates_rx) = broadcast::channel::<pb::FeatureUpdate>(8);
    let (cid, sec) = client_ids();
    let client_id = Uuid::parse_str(&cid).unwrap();
    let client_mock = stream_client_mock(client_id, fixture.team_id, sec.clone());

    let mut feature_mock = MockFeatureRepository::new();
    let features = fixture.features.clone();
    feature_mock
        .expect_get_features()
        .returning(move |_team, _key, _ftype| Ok(features.clone()));

    let all_feature_ids: std::collections::BTreeSet<Uuid> =
        fixture.features.iter().map(|f| f.id).collect();
    let all_stage_ids: std::collections::BTreeSet<Uuid> = fixture
        .stages
        .values()
        .flatten()
        .map(|stage| stage.id)
        .collect();
    let contextual_ids: std::collections::BTreeSet<Uuid> = fixture
        .features
        .iter()
        .filter(|f| matches!(f.feature_type, db::FeatureType::Contextual))
        .map(|f| f.id)
        .collect();
    let team_id = fixture.team_id;

    let stages = fixture.stages.clone();
    feature_mock
        .expect_get_feature_stages_batch()
        .withf(move |ids| {
            ids.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                == all_feature_ids
        })
        .times(1)
        .returning(stages_batch(move |feature_id| {
            Ok(stages.get(&feature_id).cloned().unwrap_or_default())
        }));
    let criteria = fixture.criteria.clone();
    feature_mock
        .expect_get_stage_criteria_batch()
        .withf(move |team, ids| {
            *team == team_id
                && ids
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
                    == all_stage_ids
        })
        .times(1)
        .returning(criteria_batch(move |stage_id| {
            Ok(criteria.get(&stage_id).cloned().unwrap_or_default())
        }));
    let variants = fixture.variants.clone();
    feature_mock
        .expect_get_feature_variants_batch()
        .withf(move |ids| {
            ids.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                == contextual_ids
        })
        .times(1)
        .returning(variants_batch(move |feature_id| {
            Ok(variants.get(&feature_id).cloned().unwrap_or_default())
        }));

    let (addr, _server) =
        start_server_with_repos(Box::new(feature_mock), Box::new(client_mock), updates_tx).await;
    let (mut stream, _tx) = open_update_stream(addr, cid, sec, vec![]).await;
    let mut snapshot = Vec::new();
    while snapshot.len() < expected.len() {
        let update = recv_update_with_timeout(&mut stream, Duration::from_secs(2))
            .await
            .expect("snapshot ended early");
        snapshot.push(update.feature.expect("snapshot carries a feature"));
    }
    assert_eq!(snapshot, expected);
}
