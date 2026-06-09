use feature_toggle_backend::Error;
use feature_toggle_backend::database::client::{
    ClientRepositoryTx, CreateClient, UpdateClient, client_repository, client_repository_tx,
};
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::entity::{ClientType, VariantValueType};
use feature_toggle_backend::database::environment::{
    CreateEnvironment, EnvironmentRepositoryTx, UpdateEnvironment, environment_repository,
    environment_repository_tx,
};
use feature_toggle_backend::database::feature::{
    CreateFeature, CreateFeatureStage, FeatureRepositoryTx, UpdateFeature, feature_repository,
    feature_repository_tx,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::pipeline::{
    CreatePipeline, CreateStage, PipelineRepositoryTx, UpdatePipeline, pipeline_repository,
    pipeline_repository_tx,
};
use uuid::Uuid;

fn seeded_team_id() -> Uuid {
    Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").expect("valid team uuid")
}

fn seeded_environment_id() -> Uuid {
    Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").expect("valid environment uuid")
}

#[tokio::test]
async fn client_tx_methods_read_uncommitted_writes_and_rollback_cleanly() {
    let pool = init_pg_pool().await;
    let tx_repo = client_repository_tx(pool.clone());
    let read_repo = client_repository(pool.clone());
    let mut tx = pool.begin().await.expect("tx begins");
    let name = format!("tx-client-{}", Uuid::new_v4());

    let created = tx_repo
        .create_client_tx(
            &mut tx,
            seeded_team_id(),
            CreateClient {
                name: name.clone(),
                description: Some("tx create".into()),
                enabled: true,
                client_type: ClientType::Web,
                web_origins: Some(vec!["https://tx.example".into()]),
                environment_id: seeded_environment_id(),
            },
        )
        .await
        .expect("create_client_tx sees transaction state");

    let updated = tx_repo
        .update_client_tx(
            &mut tx,
            created.id,
            UpdateClient {
                name: Some(format!("{name}-updated")),
                description: Some("tx update".into()),
                enabled: Some(false),
                client_type: Some(ClientType::Web),
                web_origins: Some(vec!["https://updated.example".into()]),
            },
        )
        .await
        .expect("update_client_tx can read newly created client inside tx");

    assert_eq!(updated.name, format!("{name}-updated"));
    assert!(!updated.enabled);
    assert_eq!(
        updated.web_origins.as_ref().expect("web origins in tx"),
        &vec!["https://updated.example".to_string()]
    );

    tx_repo
        .delete_client_tx(&mut tx, created.id)
        .await
        .expect("delete_client_tx can delete newly created client inside tx");

    tx.rollback().await.expect("rollback succeeds");

    let lookup = read_repo.get_client_by_id(created.id).await;
    assert!(matches!(lookup, Err(Error::NotFound(id)) if id == created.id));
}

#[tokio::test]
async fn environment_tx_methods_read_uncommitted_writes_and_rollback_cleanly() {
    let pool = init_pg_pool().await;
    let tx_repo = environment_repository_tx(pool.clone());
    let read_repo = environment_repository(pool.clone());
    let mut tx = pool.begin().await.expect("tx begins");
    let name = format!("tx-environment-{}", Uuid::new_v4());

    let created = tx_repo
        .create_environment_tx(
            &mut tx,
            seeded_team_id(),
            CreateEnvironment {
                name: name.clone(),
                active: true,
                environment_type: Some("Preview".into()),
            },
        )
        .await
        .expect("create_environment_tx sees transaction state");

    let updated = tx_repo
        .update_environment_tx(
            &mut tx,
            created.id,
            UpdateEnvironment {
                name: Some(format!("{name}-updated")),
                active: Some(false),
                environment_type: Some("Production".into()),
            },
        )
        .await
        .expect("update_environment_tx can read newly created env inside tx");

    assert_eq!(updated.name, format!("{name}-updated"));
    assert!(!updated.active);
    assert_eq!(updated.environment_type, "Production");

    tx_repo
        .delete_environment_tx(&mut tx, created.id)
        .await
        .expect("delete_environment_tx can delete newly created env inside tx");

    tx.rollback().await.expect("rollback succeeds");

    let lookup = read_repo.get_environment_by_id(created.id).await;
    assert!(matches!(lookup, Err(Error::NotFound(id)) if id == created.id));
}

#[tokio::test]
async fn feature_tx_methods_read_uncommitted_writes_and_rollback_cleanly() {
    let pool = init_pg_pool().await;
    let tx_repo = feature_repository_tx(pool.clone());
    let read_repo = feature_repository(pool.clone());
    let mut tx = pool.begin().await.expect("tx begins");
    let key = format!("tx-feature-{}", Uuid::new_v4());

    let created_id = tx_repo
        .create_feature_tx(
            &mut tx,
            CreateFeature {
                team_id: seeded_team_id(),
                key: key.clone(),
                description: Some("tx create".into()),
                feature_type: FeatureType::Simple,
                lifecycle_stage: "active".to_string(),
                owner: None,
                expires_at: None,
                cleanup_reason: None,
                stages: vec![CreateFeatureStage {
                    id: Uuid::new_v4(),
                    environment_id: seeded_environment_id(),
                    order_index: 0,
                    parent_stage: None,
                    position: "{\"x\":100,\"y\":100}".into(),
                    enabled: true,
                }],
                dependencies: vec![],
                variants: Some(vec![(
                    "control".into(),
                    serde_json::json!("on"),
                    VariantValueType::String,
                    Some("control variant".into()),
                )]),
            },
        )
        .await
        .expect("create_feature_tx sees transaction state");

    let updated = tx_repo
        .update_feature_tx(
            &mut tx,
            UpdateFeature {
                id: created_id,
                key: Some(format!("{key}-updated")),
                description: Some("tx update".into()),
                feature_type: Some(FeatureType::Contextual),
                lifecycle_stage: None,
                owner: None,
                expires_at: None,
                cleanup_reason: None,
                archive_confirmation: false,
                stages: vec![],
                dependencies: vec![],
                variants: Some(vec![]),
            },
        )
        .await
        .expect("update_feature_tx can read newly created feature inside tx");

    assert_eq!(updated.id, created_id);
    assert_eq!(updated.key, format!("{key}-updated"));
    assert!(matches!(updated.feature_type, FeatureType::Contextual));

    tx_repo
        .delete_feature_tx(&mut tx, created_id)
        .await
        .expect("delete_feature_tx can delete newly created feature inside tx");

    tx.rollback().await.expect("rollback succeeds");

    let lookup = read_repo.get_feature_by_id(created_id).await;
    assert!(matches!(lookup, Err(Error::NotFound(id)) if id == created_id));
}

#[tokio::test]
async fn pipeline_tx_methods_read_uncommitted_writes_and_rollback_cleanly() {
    let pool = init_pg_pool().await;
    let tx_repo = pipeline_repository_tx(pool.clone());
    let read_repo = pipeline_repository(pool.clone());
    let mut tx = pool.begin().await.expect("tx begins");
    let name = format!("tx-pipeline-{}", Uuid::new_v4());

    let created_id = tx_repo
        .create_pipeline_tx(
            &mut tx,
            CreatePipeline {
                team_id: seeded_team_id(),
                name: name.clone(),
                stages: vec![CreateStage {
                    id: Uuid::new_v4(),
                    environment_id: seeded_environment_id(),
                    order_index: 0,
                    parent_stage: None,
                    position: "{\"x\":250,\"y\":250}".into(),
                }],
            },
        )
        .await
        .expect("create_pipeline_tx sees transaction state");

    let updated = tx_repo
        .update_pipeline_tx(
            &mut tx,
            UpdatePipeline {
                id: created_id,
                name: Some(format!("{name}-updated")),
                active: Some(false),
                stages: vec![],
            },
        )
        .await
        .expect("update_pipeline_tx can read newly created pipeline inside tx");

    assert_eq!(updated.id, created_id);
    assert_eq!(updated.name, format!("{name}-updated"));
    assert!(!updated.active);

    tx_repo
        .delete_pipeline_tx(&mut tx, created_id)
        .await
        .expect("delete_pipeline_tx can delete newly created pipeline inside tx");

    tx.rollback().await.expect("rollback succeeds");

    let lookup = read_repo.get_pipeline_by_id(created_id).await;
    assert!(matches!(lookup, Err(Error::NotFound(id)) if id == created_id));
}
