//! B19: features may only depend on boolean flags.
//!
//! The evaluation engine treats a dependency as passed only when it evaluates
//! to the JSON boolean `true`, so a dependency on a flag that serves string,
//! number or object values always blocks its dependents. The backend rejects
//! such configurations when they are introduced (new dependency, or a
//! depended-on flag becoming non-boolean), while leaving pre-existing data
//! editable.
//!
//! Fixtures are written through the repository (committed, bypassing the
//! logic-layer validation) so pre-existing states can be set up. Each test uses
//! its own team and deletes it at the end.

use feature_toggle_backend::Error;
use feature_toggle_backend::database::activity_log::PgActivityLogRepository;
use feature_toggle_backend::database::entity::{FeatureType, VariantValueType};
use feature_toggle_backend::database::feature::{
    CreateFeature, FeatureRepository, feature_repository_tx,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::logic::feature_tx::{
    create_feature_in_tx, rollback_feature_to_version_in_tx, update_feature_in_tx,
};
use feature_toggle_backend::model::{
    CreateFeatureInput, CreateFeatureVariantInput, Feature as ModelFeature,
    FeatureType as ModelFeatureType, ID, UpdateFeatureInput,
    VariantValueType as ModelVariantValueType,
};
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("b19-{team_id}"))
        .bind("B19 dependency value type tests")
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

fn value_type_of(value: &JsonValue) -> VariantValueType {
    match value {
        JsonValue::Bool(_) => VariantValueType::Boolean,
        JsonValue::Number(_) => VariantValueType::Number,
        JsonValue::String(_) => VariantValueType::String,
        _ => VariantValueType::Json,
    }
}

fn model_value_type_of(value: &JsonValue) -> ModelVariantValueType {
    match value {
        JsonValue::Bool(_) => ModelVariantValueType::Boolean,
        JsonValue::Number(_) => ModelVariantValueType::Number,
        JsonValue::String(_) => ModelVariantValueType::String,
        _ => ModelVariantValueType::Json,
    }
}

/// Writes a feature straight through the repository (no dependency-value
/// validation), so tests can set up pre-existing states.
async fn seed_feature(
    pool: &PgPool,
    team_id: Uuid,
    key: &str,
    feature_type: FeatureType,
    variants: &[(&str, JsonValue)],
    dependencies: Vec<Uuid>,
) -> Uuid {
    let repo = feature_repository_tx(pool.clone());
    repo.create_feature(CreateFeature {
        team_id,
        key: key.to_string(),
        description: None,
        feature_type,
        lifecycle_stage: "active".to_string(),
        owner: None,
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: vec![],
        stages: vec![],
        dependencies,
        variants: if variants.is_empty() {
            None
        } else {
            Some(
                variants
                    .iter()
                    .map(|(control, value)| {
                        (
                            (*control).to_string(),
                            value.clone(),
                            value_type_of(value),
                            None,
                        )
                    })
                    .collect(),
            )
        },
    })
    .await
    .expect("seed feature")
}

fn variant_inputs(variants: &[(&str, JsonValue)]) -> Vec<CreateFeatureVariantInput> {
    variants
        .iter()
        .map(|(control, value)| CreateFeatureVariantInput {
            control: (*control).to_string(),
            value: value.clone(),
            value_type: model_value_type_of(value),
            description: None,
        })
        .collect()
}

fn update_input(
    key: &str,
    feature_type: ModelFeatureType,
    dependencies: &[Uuid],
    variants: Option<&[(&str, JsonValue)]>,
    description: &str,
) -> UpdateFeatureInput {
    UpdateFeatureInput {
        key: key.to_string(),
        description: Some(description.to_string()),
        feature_type,
        enabled: None,
        lifecycle_stage: None,
        owner: None,
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: None,
        archive_confirmation: false,
        dependencies: dependencies.iter().copied().map(ID::from).collect(),
        relationships: vec![],
        stages: vec![],
        variants: variants.map(variant_inputs),
    }
}

async fn run_update(
    pool: &PgPool,
    feature_id: Uuid,
    input: UpdateFeatureInput,
) -> Result<ModelFeature, Error> {
    let repo = feature_repository_tx(pool.clone());
    let activity = PgActivityLogRepository::new(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let result =
        update_feature_in_tx(&mut tx, &repo, &activity, ID::from(feature_id), input, None).await;
    match result {
        Ok(feature) => {
            tx.commit().await.expect("commit");
            Ok(feature)
        }
        Err(err) => {
            tx.rollback().await.expect("rollback");
            Err(err)
        }
    }
}

async fn run_create(
    pool: &PgPool,
    team_id: Uuid,
    key: &str,
    dependencies: &[Uuid],
) -> Result<ID, Error> {
    let repo = feature_repository_tx(pool.clone());
    let activity = PgActivityLogRepository::new(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let result = create_feature_in_tx(
        &mut tx,
        &repo,
        &activity,
        ID::from(team_id),
        CreateFeatureInput {
            key: key.to_string(),
            description: None,
            feature_type: ModelFeatureType::Simple,
            enabled: None,
            lifecycle_stage: None,
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: None,
            dependencies: dependencies.iter().copied().map(ID::from).collect(),
            relationships: vec![],
            stages: vec![],
            variants: None,
        },
        None,
    )
    .await;
    match result {
        Ok(id) => {
            tx.commit().await.expect("commit");
            Ok(id)
        }
        Err(err) => {
            tx.rollback().await.expect("rollback");
            Err(err)
        }
    }
}

async fn run_rollback(pool: &PgPool, feature_id: Uuid, version_id: Uuid) -> Result<(), Error> {
    let repo = feature_repository_tx(pool.clone());
    let activity = PgActivityLogRepository::new(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let result = rollback_feature_to_version_in_tx(
        &mut tx,
        &repo,
        &activity,
        ID::from(feature_id),
        ID::from(version_id),
        false,
        None,
    )
    .await;
    match result {
        Ok(_) => {
            tx.commit().await.expect("commit");
            Ok(())
        }
        Err(err) => {
            tx.rollback().await.expect("rollback");
            Err(err)
        }
    }
}

async fn latest_version_id(pool: &PgPool, feature_id: Uuid) -> Uuid {
    let repo = feature_repository_tx(pool.clone());
    let (versions, _) = repo
        .list_feature_versions(feature_id, 0, 1)
        .await
        .expect("list versions");
    versions.first().expect("at least one version").id
}

fn assert_non_boolean_rejection<T: std::fmt::Debug>(result: Result<T, Error>, key: &str) {
    match result {
        Err(Error::InvalidInput(message)) => {
            assert!(
                message.contains("non-boolean") && message.contains(key),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected InvalidInput for non-boolean dependency, got {other:?}"),
    }
}

fn string_variants() -> Vec<(&'static str, JsonValue)> {
    vec![("dark", json!("dark")), ("light", json!("light"))]
}

fn boolean_variants() -> Vec<(&'static str, JsonValue)> {
    vec![("off", json!(false)), ("on", json!(true))]
}

#[tokio::test]
async fn adding_dependency_on_non_boolean_flag_is_rejected() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;

    let theme = seed_feature(
        &pool,
        team_id,
        "b19-theme",
        FeatureType::Contextual,
        &string_variants(),
        vec![],
    )
    .await;
    let dependent = seed_feature(
        &pool,
        team_id,
        "b19-dependent",
        FeatureType::Simple,
        &[],
        vec![],
    )
    .await;

    let create = run_create(&pool, team_id, "b19-new", &[theme]).await;
    let update = run_update(
        &pool,
        dependent,
        update_input(
            "b19-dependent",
            ModelFeatureType::Simple,
            &[theme],
            None,
            "add theme dependency",
        ),
    )
    .await;

    delete_team(&pool, team_id).await;
    assert_non_boolean_rejection(create, "b19-theme");
    assert_non_boolean_rejection(update, "b19-theme");
}

#[tokio::test]
async fn adding_dependency_on_boolean_flag_is_allowed() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;

    let boolean_flag = seed_feature(
        &pool,
        team_id,
        "b19-bool",
        FeatureType::Contextual,
        &boolean_variants(),
        vec![],
    )
    .await;
    // Variants of Simple features are not evaluated, so a Simple flag is
    // boolean whatever values its stored variants hold.
    let simple_with_numbers = seed_feature(
        &pool,
        team_id,
        "b19-simple-numbers",
        FeatureType::Simple,
        &[("v1", json!(1)), ("v2", json!(2))],
        vec![],
    )
    .await;
    let no_variants = seed_feature(
        &pool,
        team_id,
        "b19-plain",
        FeatureType::Contextual,
        &[],
        vec![],
    )
    .await;

    let created = run_create(
        &pool,
        team_id,
        "b19-new",
        &[boolean_flag, simple_with_numbers, no_variants],
    )
    .await;

    delete_team(&pool, team_id).await;
    created.expect("dependencies on boolean flags are allowed");
}

#[tokio::test]
async fn making_depended_on_flag_non_boolean_is_rejected() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;

    let dependency = seed_feature(
        &pool,
        team_id,
        "b19-dependency",
        FeatureType::Contextual,
        &boolean_variants(),
        vec![],
    )
    .await;
    let simple_dependency = seed_feature(
        &pool,
        team_id,
        "b19-simple-dependency",
        FeatureType::Simple,
        &[("v1", json!(1))],
        vec![],
    )
    .await;
    seed_feature(
        &pool,
        team_id,
        "b19-dependent",
        FeatureType::Simple,
        &[],
        vec![dependency, simple_dependency],
    )
    .await;

    // Variants change to string values.
    let variants_change = run_update(
        &pool,
        dependency,
        update_input(
            "b19-dependency",
            ModelFeatureType::Contextual,
            &[],
            Some(&string_variants()),
            "switch to strings",
        ),
    )
    .await;
    // Type change: a Simple flag's stored numeric variants become evaluable.
    let type_change = run_update(
        &pool,
        simple_dependency,
        update_input(
            "b19-simple-dependency",
            ModelFeatureType::Contextual,
            &[],
            None,
            "switch to contextual",
        ),
    )
    .await;
    // Changing to other boolean variants stays allowed.
    let boolean_change = run_update(
        &pool,
        dependency,
        update_input(
            "b19-dependency",
            ModelFeatureType::Contextual,
            &[],
            Some(&[("enabled", json!(true))]),
            "still boolean",
        ),
    )
    .await;

    delete_team(&pool, team_id).await;
    assert_non_boolean_rejection(variants_change, "b19-dependent");
    assert_non_boolean_rejection(type_change, "b19-dependent");
    boolean_change.expect("boolean variant changes are allowed");
}

#[tokio::test]
async fn unrelated_update_with_pre_existing_non_boolean_dependency_succeeds() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;

    let theme = seed_feature(
        &pool,
        team_id,
        "b19-theme",
        FeatureType::Contextual,
        &string_variants(),
        vec![],
    )
    .await;
    let dependent = seed_feature(
        &pool,
        team_id,
        "b19-dependent",
        FeatureType::Simple,
        &[],
        vec![theme],
    )
    .await;

    // Dependent side: same dependencies, other fields change.
    let dependent_update = run_update(
        &pool,
        dependent,
        update_input(
            "b19-dependent",
            ModelFeatureType::Simple,
            &[theme],
            None,
            "unrelated description change",
        ),
    )
    .await;
    // Dependency side: already non-boolean, variants re-saved with new values.
    let dependency_update = run_update(
        &pool,
        theme,
        update_input(
            "b19-theme",
            ModelFeatureType::Contextual,
            &[],
            Some(&[("dark", json!("dark")), ("blue", json!("blue"))]),
            "unrelated variant edit",
        ),
    )
    .await;

    delete_team(&pool, team_id).await;
    dependent_update.expect("unrelated edit of the dependent keeps working");
    dependency_update.expect("edit of an already non-boolean dependency keeps working");
}

#[tokio::test]
async fn rollback_cannot_introduce_non_boolean_dependency() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;

    // Dependent side: a version that depended on a non-boolean flag.
    let theme = seed_feature(
        &pool,
        team_id,
        "b19-theme",
        FeatureType::Contextual,
        &string_variants(),
        vec![],
    )
    .await;
    let dependent = seed_feature(
        &pool,
        team_id,
        "b19-dependent",
        FeatureType::Simple,
        &[],
        vec![theme],
    )
    .await;
    run_update(
        &pool,
        dependent,
        update_input(
            "b19-dependent",
            ModelFeatureType::Simple,
            &[theme],
            None,
            "legacy state",
        ),
    )
    .await
    .expect("unrelated update of legacy state");
    let legacy_dependent_version = latest_version_id(&pool, dependent).await;
    run_update(
        &pool,
        dependent,
        update_input(
            "b19-dependent",
            ModelFeatureType::Simple,
            &[],
            None,
            "drop dependency",
        ),
    )
    .await
    .expect("removing a dependency is allowed");

    // Dependency side: a version with string variants, now depended on.
    let flag = seed_feature(
        &pool,
        team_id,
        "b19-flag",
        FeatureType::Contextual,
        &string_variants(),
        vec![],
    )
    .await;
    run_update(
        &pool,
        flag,
        update_input(
            "b19-flag",
            ModelFeatureType::Contextual,
            &[],
            None,
            "string state",
        ),
    )
    .await
    .expect("update without dependents");
    let string_flag_version = latest_version_id(&pool, flag).await;
    run_update(
        &pool,
        flag,
        update_input(
            "b19-flag",
            ModelFeatureType::Contextual,
            &[],
            Some(&boolean_variants()),
            "boolean state",
        ),
    )
    .await
    .expect("switch to boolean");
    seed_feature(
        &pool,
        team_id,
        "b19-flag-dependent",
        FeatureType::Simple,
        &[],
        vec![flag],
    )
    .await;

    let dependent_rollback = run_rollback(&pool, dependent, legacy_dependent_version).await;
    let dependency_rollback = run_rollback(&pool, flag, string_flag_version).await;

    delete_team(&pool, team_id).await;
    assert_non_boolean_rejection(dependent_rollback, "b19-theme");
    assert_non_boolean_rejection(dependency_rollback, "b19-flag-dependent");
}
