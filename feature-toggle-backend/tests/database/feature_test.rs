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
    assert_eq!(feature.stages.len(), 1);
    let stage = feature.stages.first().unwrap();
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
    };
    let result = repository.update_feature(input).await;

    assert!(result.is_ok());
    let feature = result.unwrap();
    assert_eq!(feature.key, "Updated Feature");
    assert_eq!(feature.description, Some("Updated description".to_string()));
    assert!(matches!(feature.feature_type, FeatureType::Contextual));
    assert!(feature.stages.is_empty());
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
    };
    let result = repository.update_feature(input).await;

    assert!(result.is_ok());
    let feature = result.unwrap();
    assert_eq!(feature.key, "Another feature Updated Feature");
    assert_eq!(feature.description, Some("Updated description".to_string()));
    assert!(matches!(feature.feature_type, FeatureType::Contextual));
    assert_eq!(feature.stages.len(), 1);
    let stage = feature.stages.first().unwrap();
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

    let id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb97").unwrap();
    let result = repository.delete_feature(id).await;

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
    assert!(features.iter().all(|p| p.key.contains("Test")));
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
    assert!(
        features
            .iter()
            .all(|p| p.key.contains("Test") && matches!(p.feature_type, FeatureType::Simple))
    );
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
    assert_eq!(feature.stages.len(), 2);

    // Find parent stage
    let parent_stage = feature.stages.iter().find(|s| s.id == parent_stage_id);
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
    let child_stage = feature.stages.iter().find(|s| s.id == child_stage_id);
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
    };

    // Create the feature
    let create_result = repository.create_feature(create_input).await;
    assert!(create_result.is_ok());
    let feature_id = create_result.unwrap();

    // Verify initial feature
    let initial_feature = repository.get_feature_by_id(feature_id).await.unwrap();
    assert_eq!(initial_feature.stages.len(), 2);

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
    assert_eq!(updated_feature.stages.len(), 2); // Should have 2 stages now

    // Find updated stage1
    let updated_stage1 = updated_feature.stages.iter().find(|s| s.id == stage1_id);
    assert!(updated_stage1.is_some());
    let updated_stage1 = updated_stage1.unwrap();
    assert_eq!(updated_stage1.position, "{ x: 150, y: 150 }"); // Verify position was updated
    assert!(!updated_stage1.enabled); // Verify enabled status was updated

    // Verify stage2 was deleted (should not exist)
    let deleted_stage2 = updated_feature.stages.iter().find(|s| s.id == stage2_id);
    assert!(deleted_stage2.is_none());

    // Find new stage3
    let new_stage3 = updated_feature.stages.iter().find(|s| s.id == stage3_id);
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
