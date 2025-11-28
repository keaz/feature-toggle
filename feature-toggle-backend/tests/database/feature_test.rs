use std::vec;

use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{CreateFeature, CreateFeatureStage, UpdateFeature};
use feature_toggle_backend::database::{feature, init_pg_pool};
use uuid::Uuid;

#[tokio::test]
async fn test_get_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_feature_by_id(id).await;

    assert!(result.is_ok());
    let feature = result.unwrap();
    assert_eq!(feature.id, id);
    assert_eq!(feature.key, "Test Feature");

    let stages = repository
        .get_feature_stages(feature.id)
        .await
        .expect("feature stages should load");
    assert_eq!(stages.len(), 1);
    let stage = stages.first().unwrap();
    assert_eq!(
        stage.id,
        Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap()
    );
    assert_eq!(
        stage.environment_id,
        Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap()
    );
    assert_eq!(stage.order_index, 0);
    assert_eq!(stage.position, "{ \"x\": 250, \"y\": 250 }");
    assert!(stage.enabled);
}

#[tokio::test]
async fn test_feature_lifecycle_defaults() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let feature = repository
        .get_feature_by_id(id)
        .await
        .expect("should load feature");

    assert_eq!(feature.lifecycle_stage, "active");
    assert_eq!(feature.evaluation_count_7d, 0);
    assert_eq!(feature.evaluation_count_30d, 0);
    assert_eq!(feature.evaluation_count_90d, 0);
    assert!(feature.deprecated_at.is_none());
}

#[tokio::test]
async fn test_get_non_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.get_feature_by_id(id).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_create_feature_without_stages() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_key = format!("Without Stages {}", Uuid::new_v4());
    let input = CreateFeature {
        team_id,
        key: random_key.clone(),
        description: Some("Test feature without stages".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![],
        dependencies: vec![],
        variants: None,
    };
    let result = repository.create_feature(input).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_create_feature_with_stages() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_key = format!("With Stages {}", Uuid::new_v4());
    let parent = CreateFeatureStage {
        id: Uuid::new_v4(),
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 250, y: 250 }"),
        enabled: true,
        bucketing_key: None,
    };
    let input = CreateFeature {
        team_id,
        key: random_key.clone(),
        description: Some("Test feature with stages".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![
            parent.clone(),
            CreateFeatureStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                order_index: 1,
                parent_stage: Some(Box::new(parent)),
                position: String::from("{ x: 500, y: 500 }"),
                enabled: true,
                bucketing_key: None,
            },
        ],
        dependencies: vec![],
        variants: None,
    };
    let result = repository.create_feature(input).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_create_feature_with_dependencies() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    // First create a feature to depend on
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let dependency_key = format!("Dependency {}", Uuid::new_v4());
    let dependency_input = CreateFeature {
        team_id,
        key: dependency_key.clone(),
        description: Some("Dependency feature".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![],
        dependencies: vec![],
        variants: None,
    };
    let dependency_result = repository.create_feature(dependency_input).await;
    assert!(dependency_result.is_ok());
    let dependency_id = dependency_result.unwrap();

    // Now create a feature that depends on the first one
    let feature_key = format!("With Dependencies {}", Uuid::new_v4());
    let input = CreateFeature {
        team_id,
        key: feature_key.clone(),
        description: Some("Test feature with dependencies".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![],
        dependencies: vec![dependency_id],
        variants: None,
    };
    let result = repository.create_feature(input).await;

    assert!(result.is_ok());

    // Verify the dependency was created
    let feature_id = result.unwrap();
    let feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert_eq!(feature.dependencies.len(), 1);
    assert_eq!(feature.dependencies[0].depends_on_id, dependency_id);
}

#[tokio::test]
async fn test_create_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let input = CreateFeature {
        team_id,
        key: "Existing Feature".to_string(),
        description: Some("Test feature".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![],
        dependencies: vec![],
        variants: None,
    };
    let result = repository.create_feature(input).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::Error::RecordAlreadyExists(_)
    ));
}

#[tokio::test]
async fn test_update_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let input = UpdateFeature {
        id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        key: Some("Updated Feature".to_string()),
        description: Some("Updated description".to_string()),
        feature_type: Some(FeatureType::Contextual),
        stages: vec![],
        dependencies: vec![],
        variants: None,
    };
    let result = repository.update_feature(input).await;

    assert!(result.is_ok());
    let feature = result.unwrap();
    assert_eq!(feature.key, "Updated Feature");
    assert_eq!(feature.description, Some("Updated description".to_string()));
    assert!(matches!(feature.feature_type, FeatureType::Contextual));
    let stages = repository
        .get_feature_stages(feature.id)
        .await
        .expect("stages should load");
    assert!(stages.is_empty());
}

#[tokio::test]
async fn test_update_feature_with_existing_stages() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let input = UpdateFeature {
        id: Uuid::parse_str("6eef17bc-9e06-411d-b5f4-7a786e68bb81").unwrap(),
        key: Some("Another feature Updated Feature".to_string()),
        description: Some("Updated description".to_string()),
        feature_type: Some(FeatureType::Contextual),
        stages: vec![CreateFeatureStage {
            id: Uuid::parse_str("4eef17bc-9e06-411d-b5f4-7a786e68bb98").unwrap(),
            environment_id: Uuid::parse_str("78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017").unwrap(),
            order_index: 0,
            parent_stage: None,
            position: String::from("{ x: 200, y: 200 }"),
            enabled: false,
            bucketing_key: None,
        }],
        dependencies: vec![],
        variants: None,
    };
    let result = repository.update_feature(input).await;

    assert!(result.is_ok());
    let feature = result.unwrap();
    assert_eq!(feature.key, "Another feature Updated Feature");
    assert_eq!(feature.description, Some("Updated description".to_string()));
    assert!(matches!(feature.feature_type, FeatureType::Contextual));
    let stages = repository
        .get_feature_stages(feature.id)
        .await
        .expect("stages should load after update");
    assert_eq!(stages.len(), 1);
    let stage = stages.first().unwrap();
    assert_eq!(
        stage.id,
        Uuid::parse_str("4eef17bc-9e06-411d-b5f4-7a786e68bb98").unwrap()
    );
    assert_eq!(
        stage.environment_id,
        Uuid::parse_str("78ccc5d7-e1bb-4e41-b6ef-02adf5c0d017").unwrap()
    );
    assert_eq!(stage.order_index, 0);
    assert_eq!(stage.position, "{ x: 200, y: 200 }");
    assert!(!stage.enabled);
}

#[tokio::test]
async fn test_update_non_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let input = UpdateFeature {
        id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap(),
        key: Some("Non-existing Feature".to_string()),
        description: Some("This feature doesn't exist".to_string()),
        feature_type: Some(FeatureType::Simple),
        stages: vec![],
        dependencies: vec![],
        variants: None,
    };
    let result = repository.update_feature(input).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_delete_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let feature_id = repository
        .create_feature(CreateFeature {
            team_id,
            key: format!("Delete me {}", Uuid::new_v4()),
            description: Some("temp feature".to_string()),
            feature_type: FeatureType::Simple,
            stages: vec![],
            dependencies: vec![],
            variants: None,
        })
        .await
        .expect("feature to be created for delete");

    let result = repository.delete_feature(feature_id).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_delete_non_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.delete_feature(id).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_get_all_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_features(team_id, None, None).await;

    assert!(result.is_ok());
    let features = result.unwrap();
    assert!(!features.is_empty());
}

#[tokio::test]
async fn test_key_get_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_features(team_id, Some("Test".to_string()), None)
        .await;

    assert!(result.is_ok());
    let features = result.unwrap();
    assert!(
        features
            .iter()
            .all(|p| p.key.to_lowercase().contains("test"))
    );
}

#[tokio::test]
async fn test_feature_type_get_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_features(team_id, None, Some(FeatureType::Simple))
        .await;

    assert!(result.is_ok());
    let features = result.unwrap();
    assert!(!features.is_empty());
    assert!(
        features
            .iter()
            .all(|p| matches!(p.feature_type, FeatureType::Simple))
    );
}

#[tokio::test]
async fn test_key_and_feature_type_get_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_features(team_id, Some("Test".to_string()), Some(FeatureType::Simple))
        .await;

    assert!(result.is_ok());
    let features = result.unwrap();
    assert!(features.iter().all(|p| {
        p.key.to_lowercase().contains("test") && matches!(p.feature_type, FeatureType::Simple)
    }));
}

#[tokio::test]
async fn test_create_feature_with_stages_verification() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_key = format!("Stage Verification {}", Uuid::new_v4());

    // Create stage IDs
    let parent_stage_id = Uuid::new_v4();
    let child_stage_id = Uuid::new_v4();

    // Create parent stage
    let parent = CreateFeatureStage {
        id: parent_stage_id,
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 250, y: 250 }"),
        enabled: true,
        bucketing_key: None,
    };

    // Create child stage that depends on parent
    let child = CreateFeatureStage {
        id: child_stage_id,
        environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
        order_index: 1,
        parent_stage: Some(Box::new(parent.clone())),
        position: String::from("{ x: 500, y: 500 }"),
        enabled: true,
        bucketing_key: None,
    };

    // Create feature with stages
    let input = CreateFeature {
        team_id,
        key: random_key.clone(),
        description: Some("Test feature with stages verification".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![parent.clone(), child],
        dependencies: vec![],
        variants: None,
    };

    // Create the feature
    let result = repository.create_feature(input).await;
    assert!(result.is_ok());

    // Get the created feature
    let feature_id = result.unwrap();
    let feature = repository.get_feature_by_id(feature_id).await;
    assert!(feature.is_ok());

    // Verify the feature and its stages
    let feature = feature.unwrap();
    assert_eq!(feature.key, random_key);

    let stages = repository
        .get_feature_stages(feature.id)
        .await
        .expect("stages for created feature should load");
    assert_eq!(stages.len(), 2);

    // Find parent stage
    let parent_stage = stages.iter().find(|s| s.id == parent_stage_id);
    assert!(parent_stage.is_some());
    let parent_stage = parent_stage.unwrap();
    assert_eq!(
        parent_stage.environment_id,
        Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap()
    );
    assert_eq!(parent_stage.order_index, 0);
    assert_eq!(parent_stage.parent_stage_id, None);
    assert_eq!(parent_stage.position, "{ x: 250, y: 250 }");
    assert!(parent_stage.enabled);

    // Find child stage
    let child_stage = stages.iter().find(|s| s.id == child_stage_id);
    assert!(child_stage.is_some());
    let child_stage = child_stage.unwrap();
    assert_eq!(
        child_stage.environment_id,
        Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap()
    );
    assert_eq!(child_stage.order_index, 1);
    assert_eq!(child_stage.parent_stage_id, Some(parent_stage_id));
    assert_eq!(child_stage.position, "{ x: 500, y: 500 }");
    assert!(child_stage.enabled);
}

#[tokio::test]
async fn test_update_feature_with_stages() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_key = format!("Update Stages Test {}", Uuid::new_v4());

    // Create stage IDs for initial feature
    let stage1_id = Uuid::new_v4();
    let stage2_id = Uuid::new_v4();

    // Create stages for initial feature
    let stage1 = CreateFeatureStage {
        id: stage1_id,
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 100, y: 100 }"),
        enabled: true,
        bucketing_key: None,
    };

    let stage2 = CreateFeatureStage {
        id: stage2_id,
        environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
        order_index: 1,
        parent_stage: Some(Box::new(stage1.clone())),
        position: String::from("{ x: 200, y: 200 }"),
        enabled: true,
        bucketing_key: None,
    };

    // Create initial feature with stages
    let create_input = CreateFeature {
        team_id,
        key: random_key.clone(),
        description: Some("Test feature for update stages".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![stage1.clone(), stage2.clone()],
        dependencies: vec![],
        variants: None,
    };

    // Create the feature
    let create_result = repository.create_feature(create_input).await;
    assert!(create_result.is_ok());
    let feature_id = create_result.unwrap();

    // Verify initial feature
    let initial_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    let initial_stages = repository
        .get_feature_stages(initial_feature.id)
        .await
        .expect("initial stages should load");
    assert_eq!(initial_stages.len(), 2);

    // Create a new stage ID for update
    let stage3_id = Uuid::new_v4();

    // Update stage1 (existing stage)
    let updated_stage1 = CreateFeatureStage {
        id: stage1_id,
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 150, y: 150 }"), // Updated position
        enabled: false,                               // Updated enabled status
        bucketing_key: None,
    };

    // Create a new stage3 (new stage)
    let new_stage3 = CreateFeatureStage {
        id: stage3_id,
        environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
        order_index: 2,
        parent_stage: None,
        position: String::from("{ x: 300, y: 300 }"),
        enabled: true,
        bucketing_key: None,
    };

    // Update the feature
    // - stage1 will be updated
    // - stage2 will be deleted (not included in update)
    // - stage3 will be inserted (new stage)
    let update_input = UpdateFeature {
        id: feature_id,
        key: Some(format!("{random_key} - Updated")),
        description: Some("Updated description".to_string()),
        feature_type: Some(FeatureType::Contextual),
        stages: vec![updated_stage1, new_stage3],
        dependencies: vec![],
        variants: None,
    };

    let update_result = repository.update_feature(update_input).await;
    assert!(update_result.is_ok());

    // Get the updated feature
    let updated_feature = update_result.unwrap();

    // Verify feature was updated
    assert_eq!(updated_feature.key, format!("{random_key} - Updated"));
    assert_eq!(
        updated_feature.description,
        Some("Updated description".to_string())
    );
    assert!(matches!(
        updated_feature.feature_type,
        FeatureType::Contextual
    ));

    // Verify stages
    let updated_stages = repository
        .get_feature_stages(updated_feature.id)
        .await
        .expect("updated stages should load");
    assert_eq!(updated_stages.len(), 2); // Should have 2 stages now

    // Find updated stage1
    let updated_stage1 = updated_stages.iter().find(|s| s.id == stage1_id);
    assert!(updated_stage1.is_some());
    let updated_stage1 = updated_stage1.unwrap();
    assert_eq!(updated_stage1.position, "{ x: 150, y: 150 }"); // Verify position was updated

    // Verify stage2 was deleted (should not exist)
    let deleted_stage2 = updated_stages.iter().find(|s| s.id == stage2_id);
    assert!(deleted_stage2.is_none());

    // Find new stage3
    let new_stage3 = updated_stages.iter().find(|s| s.id == stage3_id);
    assert!(new_stage3.is_some());
    let new_stage3 = new_stage3.unwrap();
    assert_eq!(
        new_stage3.environment_id,
        Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap()
    );
    assert_eq!(new_stage3.order_index, 2);
    assert_eq!(new_stage3.position, "{ x: 300, y: 300 }");
    assert!(new_stage3.enabled);
}

// Kill Switch Integration Tests
#[tokio::test]
async fn test_emergency_disable_feature_integration() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    // Create an isolated feature so repeated runs don't depend on seed state
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let feature_id = repository
        .create_feature(CreateFeature {
            team_id,
            key: format!("kill-switch-{}", Uuid::new_v4()),
            description: Some("temp for kill switch".into()),
            feature_type: FeatureType::Simple,
            stages: vec![],
            dependencies: vec![],
            variants: None,
        })
        .await
        .expect("feature to create");

    // Get initial state
    let initial_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert!(
        initial_feature.kill_switch_enabled,
        "Feature should initially be enabled (kill_switch_enabled=true)"
    );

    // Emergency disable without rollback
    let disabled_feature = repository.emergency_disable_feature(feature_id, None).await;
    assert!(disabled_feature.is_ok(), "Emergency disable should succeed");

    let disabled_feature = disabled_feature.unwrap();
    assert!(
        !disabled_feature.kill_switch_enabled,
        "Kill switch should be activated (feature disabled, kill_switch_enabled=false)"
    );
    assert!(
        disabled_feature.kill_switch_activated_at.is_some(),
        "Should have activation timestamp"
    );
    assert!(
        disabled_feature.rollback_scheduled_at.is_none(),
        "Should not have rollback scheduled"
    );

    // Verify state persists by re-fetching
    let persisted_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert!(
        !persisted_feature.kill_switch_enabled,
        "Kill switch activated state should persist (kill_switch_enabled=false)"
    );

    // Re-enable for cleanup
    repository
        .emergency_enable_feature(feature_id)
        .await
        .unwrap();

    let _ = repository.delete_feature(feature_id).await;
}

#[tokio::test]
async fn test_emergency_disable_with_rollback_integration() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let feature_id = Uuid::parse_str("dd663d53-0bcd-44ab-b9e2-5ac27312805e").unwrap();
    let rollback_minutes = 30;
    let before_disable = chrono::Utc::now();

    // Emergency disable with rollback
    let result = repository
        .emergency_disable_feature(feature_id, Some(rollback_minutes))
        .await;
    assert!(
        result.is_ok(),
        "Emergency disable with rollback should succeed"
    );

    let disabled_feature = result.unwrap();
    assert!(
        disabled_feature.kill_switch_enabled,
        "Feature should remain enabled until the scheduled disable executes"
    );
    assert!(
        disabled_feature.kill_switch_activated_at.is_none(),
        "Activation timestamp should remain empty while pending disable"
    );
    assert!(
        disabled_feature.rollback_scheduled_at.is_some(),
        "Should have rollback scheduled"
    );

    // Verify rollback time is approximately correct (within 2 minutes tolerance for CI)
    let expected_rollback = before_disable + chrono::Duration::minutes(rollback_minutes as i64);
    let actual_rollback = disabled_feature.rollback_scheduled_at.unwrap();
    let time_diff = (actual_rollback - expected_rollback).num_seconds().abs();
    assert!(
        time_diff <= 120,
        "Rollback time should be within 2 minutes of expected"
    );

    // Cleanup
    repository
        .emergency_enable_feature(feature_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_emergency_enable_feature_integration() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let feature_id = Uuid::parse_str("00e862c2-29b4-4fd0-9dcb-4d3f274cc5c2").unwrap();

    // First disable the feature
    repository
        .emergency_disable_feature(feature_id, None)
        .await
        .unwrap();

    // Verify it's disabled
    let disabled_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert!(
        !disabled_feature.kill_switch_enabled,
        "Kill switch should be active (feature disabled, kill_switch_enabled=false)"
    );

    // Now enable it
    let result = repository.emergency_enable_feature(feature_id).await;
    assert!(result.is_ok(), "Emergency enable should succeed");

    let enabled_feature = result.unwrap();
    assert!(
        !enabled_feature.kill_switch_enabled,
        "Kill switch should be deactivated (feature enabled, kill_switch_enabled=true)"
    );
    assert!(
        enabled_feature.kill_switch_activated_at.is_none(),
        "Activation timestamp should be cleared"
    );
    assert!(
        enabled_feature.rollback_scheduled_at.is_none(),
        "Rollback schedule should be cleared"
    );

    // Verify state persists
    let persisted_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert!(
        !persisted_feature.kill_switch_enabled,
        "Kill switch should be deactivated (feature enabled, kill_switch_enabled=true)"
    );
}

#[tokio::test]
async fn test_get_features_pending_rollback_integration() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    // Create a test feature that needs rollback
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let create_feature = CreateFeature {
        key: "test_rollback_feature_2".to_string(),
        description: Some("Test feature for rollback".to_string()),
        feature_type: FeatureType::Simple,
        team_id,
        dependencies: vec![],
        variants: None,
        stages: vec![],
    };
    let feature_id = repository.create_feature(create_feature).await.unwrap();

    // Disable with past rollback time (simulate expired rollback)
    let past_time = chrono::Utc::now() - chrono::Duration::minutes(10);

    // Manually set rollback time to past for testing
    let pool_direct = init_pg_pool().await;
    sqlx::query!(
        r#"UPDATE features 
           SET kill_switch_enabled = true,
               kill_switch_activated_at = NULL,
               rollback_scheduled_at = $1
           WHERE id = $2"#,
        past_time,
        feature_id
    )
    .execute(&pool_direct)
    .await
    .unwrap();

    // Get features pending rollback
    let result = repository.get_features_pending_rollback().await;
    assert!(result.is_ok(), "Get pending rollback should succeed");

    let pending_features = result.unwrap();

    // Should find our test feature
    let found_feature = pending_features.iter().find(|f| f.id == feature_id);
    assert!(
        found_feature.is_some(),
        "Should find the test feature pending rollback"
    );

    let found_feature = found_feature.unwrap();
    assert!(
        found_feature.kill_switch_enabled,
        "Kill switch should be scheduled (feature remains enabled until rollback executes)"
    );
    assert!(
        found_feature.rollback_scheduled_at.is_some(),
        "Found feature should have rollback scheduled"
    );

    // Cleanup - delete the test feature
    sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool_direct)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_kill_switch_error_handling_integration() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let nonexistent_id = Uuid::new_v4();

    // Test emergency disable on nonexistent feature
    let disable_result = repository
        .emergency_disable_feature(nonexistent_id, None)
        .await;
    assert!(
        disable_result.is_err(),
        "Should fail for nonexistent feature"
    );

    match disable_result {
        Err(feature_toggle_backend::Error::NotFound(id)) => {
            assert_eq!(id, nonexistent_id, "Should return the correct ID");
        }
        _ => panic!("Expected NotFound error"),
    }

    // Test emergency enable on nonexistent feature
    let enable_result = repository.emergency_enable_feature(nonexistent_id).await;
    assert!(
        enable_result.is_err(),
        "Should fail for nonexistent feature"
    );

    match enable_result {
        Err(feature_toggle_backend::Error::NotFound(id)) => {
            assert_eq!(id, nonexistent_id, "Should return the correct ID");
        }
        _ => panic!("Expected NotFound error"),
    }
}

#[tokio::test]
async fn test_kill_switch_multiple_operations_integration() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let feature_id = Uuid::parse_str("25f955ff-7e4b-4a05-ada2-f82280157649").unwrap();

    // Test multiple disable/enable cycles
    for cycle in 1..=3 {
        // Disable
        let scheduled = repository
            .emergency_disable_feature(feature_id, Some(cycle * 10))
            .await
            .unwrap();
        assert!(
            scheduled.kill_switch_enabled,
            "Cycle {}: feature remains enabled until scheduled disable",
            cycle
        );
        assert!(
            scheduled.kill_switch_activated_at.is_none(),
            "Cycle {}: activation timestamp should be empty while pending",
            cycle
        );
        assert!(
            scheduled.rollback_scheduled_at.is_some(),
            "Cycle {}: rollback_scheduled_at should be populated",
            cycle
        );

        // Simulate scheduler executing the disable
        let disabled = repository
            .emergency_disable_feature(feature_id, None)
            .await
            .unwrap();
        assert!(
            !disabled.kill_switch_enabled,
            "Cycle {}: kill switch should be active after scheduler run",
            cycle
        );
        assert!(
            disabled.kill_switch_activated_at.is_some(),
            "Cycle {}: activation timestamp should be set after disable",
            cycle
        );
        assert!(
            disabled.rollback_scheduled_at.is_none(),
            "Cycle {}: scheduled timestamp should be cleared after disable",
            cycle
        );

        // Enable
        let enabled = repository
            .emergency_enable_feature(feature_id)
            .await
            .unwrap();
        assert!(
            !enabled.kill_switch_enabled,
            "Cycle {}: kill switch should be cleared (feature enabled)",
            cycle
        );
        assert!(
            enabled.kill_switch_activated_at.is_none(),
            "Cycle {}: timestamps should be cleared",
            cycle
        );
    }

    // Final verification
    let final_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert!(
        !final_feature.kill_switch_enabled,
        "Final state should have kill switch off (feature enabled)"
    );
}

// Pagination Integration Tests
#[tokio::test]
async fn test_get_features_paginated_with_real_data() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test pagination with page size 1
    let (features_page1, total) = repo
        .get_features_paginated(team_id, Some("Paginated".to_string()), None, 1, 1)
        .await
        .expect("get_features_paginated ok");

    if total > 0 {
        assert_eq!(features_page1.len(), 1);

        // Test getting second page if there are enough features
        if total > 1 {
            let (features_page2, total2) = repo
                .get_features_paginated(team_id, Some("Paginated".to_string()), None, 2, 1)
                .await
                .expect("get_features_paginated page 2 ok");

            assert_eq!(features_page2.len(), 1);
            assert_eq!(total2, total, "Total should be consistent across pages");

            // Ensure pages contain different features
            assert_ne!(
                features_page1[0].id, features_page2[0].id,
                "Different pages should contain different features"
            );
        }

        // Test larger page size
        let (features_large_page, total3) = repo
            .get_features_paginated(team_id, Some("Paginated".to_string()), None, 1, 10)
            .await
            .expect("get_features_paginated large page ok");

        assert!(features_large_page.len() >= 1);
        assert_eq!(total3, total, "Total should be consistent");
    }
}

#[tokio::test]
async fn test_get_features_paginated_with_filters() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test with feature type filter
    let (simple_features, simple_total) = repo
        .get_features_paginated(team_id, None, Some(FeatureType::Simple), 1, 10)
        .await
        .expect("get simple features ok");

    for feature in &simple_features {
        assert!(matches!(feature.feature_type, FeatureType::Simple));
    }

    // Test with key filter
    if simple_total > 0 {
        let feature_key = &simple_features[0].key;
        let (filtered_features, filtered_total) = repo
            .get_features_paginated(team_id, Some(feature_key.clone()), None, 1, 10)
            .await
            .expect("get filtered features ok");

        assert!(filtered_total > 0, "Should find features matching the key");
        for feature in &filtered_features {
            assert!(feature.key.contains(feature_key), "Key should match filter");
        }
    }
}

#[tokio::test]
async fn test_get_features_paginated_edge_cases() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test with page number beyond available data
    let (empty_features, total) = repo
        .get_features_paginated(team_id, Some("Paginated".to_string()), None, 999, 10)
        .await
        .expect("get_features_paginated beyond data ok");

    assert_eq!(empty_features.len(), 0);

    // Test with very large page size
    let (all_features, total2) = repo
        .get_features_paginated(team_id, Some("Paginated".to_string()), None, 1, 1000)
        .await
        .expect("get_features_paginated large page size ok");

    assert_eq!(
        all_features.len() as i64,
        total2,
        "Should return all available features"
    );
    assert_eq!(total2, total, "Total should be consistent");
}
