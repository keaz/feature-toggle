//! AI-20: the logic layer hands the activity id of the entry it wrote back to
//! the REST layer, so the justification judgment can use it as its subject.
//! Each test uses its own team and deletes it at the end.

use feature_toggle_backend::database::activity_log::{
    ActivityLogRepository, PgActivityLogRepository,
};
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{
    CreateFeature, FeatureRepository, feature_repository_tx,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::logic::feature_tx::{
    emergency_disable_feature_in_tx, emergency_enable_feature_in_tx, update_feature_in_tx,
};
use feature_toggle_backend::model::{
    FeatureType as ModelFeatureType, ID, LifecycleStage, UpdateFeatureInput,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("ai20-{team_id}"))
        .bind("AI-20 justification recording tests")
        .execute(pool)
        .await
        .expect("insert team");
    team_id
}

async fn delete_team(pool: &PgPool, team_id: Uuid) {
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("delete team");
}

async fn seed_feature(pool: &PgPool, team_id: Uuid, key: &str) -> Uuid {
    feature_repository_tx(pool.clone())
        .create_feature(CreateFeature {
            team_id,
            key: key.to_string(),
            description: None,
            feature_type: FeatureType::Simple,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            stages: vec![],
            dependencies: vec![],
            variants: None,
        })
        .await
        .expect("seed feature")
}

fn update_input(
    key: &str,
    stage: LifecycleStage,
    cleanup_reason: Option<&str>,
) -> UpdateFeatureInput {
    UpdateFeatureInput {
        key: key.to_string(),
        description: None,
        feature_type: ModelFeatureType::Simple,
        enabled: None,
        lifecycle_stage: Some(stage),
        owner: None,
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: cleanup_reason.map(|reason| Some(reason.to_string())),
        tags: None,
        archive_confirmation: true,
        dependencies: vec![],
        relationships: vec![],
        stages: vec![],
        variants: None,
    }
}

async fn activity_metadata(pool: &PgPool, id: Uuid) -> (String, Value) {
    let row: (String, Option<Value>) =
        sqlx::query_as("SELECT activity_type, metadata FROM activity_log WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("activity row");
    (row.0, row.1.unwrap_or(Value::Null))
}

#[tokio::test]
async fn archiving_records_cleanup_reason_and_returns_the_activity_id() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let feature_id = seed_feature(&pool, team_id, "ai20-archive").await;
    let repo = feature_repository_tx(pool.clone());
    let activity = PgActivityLogRepository::new(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    let outcome = update_feature_in_tx(
        &mut tx,
        &repo,
        &activity,
        ID::from(feature_id),
        update_input(
            "ai20-archive",
            LifecycleStage::Archived,
            Some("  Replaced by checkout-v3, removed all call sites  "),
        ),
        None,
    )
    .await
    .expect("update");
    tx.commit().await.unwrap();

    assert_eq!(
        outcome.cleanup_reason.as_deref(),
        Some("Replaced by checkout-v3, removed all call sites")
    );
    let (activity_type, metadata) = activity_metadata(&pool, outcome.activity_id).await;
    assert_eq!(activity_type, "feature_lifecycle_updated");
    assert_eq!(
        metadata["cleanup_reason"],
        "Replaced by checkout-v3, removed all call sites"
    );
    assert_eq!(metadata["new_lifecycle_stage"], "archived");
    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn updates_without_archiving_add_no_cleanup_reason() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let feature_id = seed_feature(&pool, team_id, "ai20-plain").await;
    let repo = feature_repository_tx(pool.clone());
    let activity = PgActivityLogRepository::new(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    let outcome = update_feature_in_tx(
        &mut tx,
        &repo,
        &activity,
        ID::from(feature_id),
        update_input("ai20-plain", LifecycleStage::Active, Some("not an archive")),
        None,
    )
    .await
    .expect("update");
    tx.commit().await.unwrap();

    assert_eq!(outcome.cleanup_reason, None);
    let (activity_type, metadata) = activity_metadata(&pool, outcome.activity_id).await;
    assert_eq!(activity_type, "feature_updated");
    assert!(metadata.get("cleanup_reason").is_none());
    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn emergency_paths_return_the_kill_switch_activity_id() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let feature_id = seed_feature(&pool, team_id, "ai20-kill").await;
    let repo = feature_repository_tx(pool.clone());
    let activity = PgActivityLogRepository::new(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    let (_, disabled_id) = emergency_disable_feature_in_tx(
        &mut tx,
        &repo,
        &activity,
        ID::from(feature_id),
        None,
        "Checkout 500s since 14:00".to_string(),
        None,
        None,
    )
    .await
    .expect("disable");
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let (_, enabled_id) = emergency_enable_feature_in_tx(
        &mut tx,
        &repo,
        &activity,
        ID::from(feature_id),
        "Fix deployed in release 4.2".to_string(),
        None,
    )
    .await
    .expect("enable");
    tx.commit().await.unwrap();

    let (disabled_type, disabled_meta) = activity_metadata(&pool, disabled_id).await;
    assert_eq!(disabled_type, "kill_switch_activated");
    assert_eq!(disabled_meta["reason"], "Checkout 500s since 14:00");
    let (enabled_type, enabled_meta) = activity_metadata(&pool, enabled_id).await;
    assert_eq!(enabled_type, "kill_switch_deactivated");
    assert_eq!(enabled_meta["reason"], "Fix deployed in release 4.2");
    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn merge_activity_metadata_sets_one_key_and_keeps_the_rest() {
    let pool = init_pg_pool().await;
    let activity = PgActivityLogRepository::new(pool.clone());
    let with_metadata = activity
        .create_activity(
            feature_toggle_backend::database::activity_log::CreateActivityLog {
                activity_type: "kill_switch_activated".to_string(),
                entity_type: "feature".to_string(),
                entity_id: Uuid::new_v4().to_string(),
                actor_id: None,
                actor_name: None,
                description: "merge test".to_string(),
                metadata: Some(json!({ "reason": "urgent" })),
            },
        )
        .await
        .unwrap();
    let without_metadata = activity
        .create_activity(
            feature_toggle_backend::database::activity_log::CreateActivityLog {
                activity_type: "kill_switch_activated".to_string(),
                entity_type: "feature".to_string(),
                entity_id: Uuid::new_v4().to_string(),
                actor_id: None,
                actor_name: None,
                description: "merge test, no metadata".to_string(),
                metadata: None,
            },
        )
        .await
        .unwrap();

    let verdict = json!({ "verdict": "weak", "probability": 0.1, "model": "jev-1.13.0" });
    activity
        .merge_activity_metadata(with_metadata.id, "ai_justification", verdict.clone())
        .await
        .unwrap();
    activity
        .merge_activity_metadata(without_metadata.id, "ai_justification", verdict.clone())
        .await
        .unwrap();
    // A missing row is not an error.
    activity
        .merge_activity_metadata(Uuid::new_v4(), "ai_justification", verdict.clone())
        .await
        .unwrap();

    let (_, merged) = activity_metadata(&pool, with_metadata.id).await;
    assert_eq!(
        merged,
        json!({ "reason": "urgent", "ai_justification": verdict })
    );
    let (_, created) = activity_metadata(&pool, without_metadata.id).await;
    assert_eq!(created, json!({ "ai_justification": verdict }));

    sqlx::query("DELETE FROM activity_log WHERE id = ANY($1)")
        .bind(vec![with_metadata.id, without_metadata.id])
        .execute(&pool)
        .await
        .unwrap();
}
