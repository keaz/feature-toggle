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

    assert_eq!(result.is_ok(), true);
    let feature = result.unwrap();
    assert_eq!(feature.id, id);
    assert_eq!(feature.name, "Test Feature");
}

#[tokio::test]
async fn test_get_non_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.get_feature_by_id(id).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_create_feature_without_stages() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_name = format!("Without Stages {}", Uuid::new_v4());
    let input = CreateFeature {
        team_id,
        name: random_name.clone(),
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
    let random_name = format!("With Stages {}", Uuid::new_v4());
    let parent = CreateFeatureStage {
        id: Uuid::new_v4(),
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 250, y: 250 }"),
        enabled: true,
    };
    let input = CreateFeature {
        team_id,
        name: random_name.clone(),
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
    let dependency_name = format!("Dependency {}", Uuid::new_v4());
    let dependency_input = CreateFeature {
        team_id,
        name: dependency_name.clone(),
        description: Some("Dependency feature".to_string()),
        feature_type: FeatureType::Simple,
        stages: vec![],
        dependencies: vec![],
    };
    let dependency_result = repository.create_feature(dependency_input).await;
    assert!(dependency_result.is_ok());
    let dependency_id = dependency_result.unwrap();

    // Now create a feature that depends on the first one
    let feature_name = format!("With Dependencies {}", Uuid::new_v4());
    let input = CreateFeature {
        team_id,
        name: feature_name.clone(),
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
        name: "Existing Feature".to_string(),
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
        name: Some("Updated Feature".to_string()),
        description: Some("Updated description".to_string()),
        feature_type: Some(FeatureType::Contextual),
        stages: vec![],
        dependencies: vec![],
    };
    let result = repository.update_feature(input).await;

    assert_eq!(result.is_ok(), true);
    let feature = result.unwrap();
    assert_eq!(feature.name, "Updated Feature");
    assert_eq!(feature.description, Some("Updated description".to_string()));
    assert!(matches!(feature.feature_type, FeatureType::Contextual));
}

#[tokio::test]
async fn test_update_non_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let input = UpdateFeature {
        id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap(),
        name: Some("Non-existing Feature".to_string()),
        description: Some("This feature doesn't exist".to_string()),
        feature_type: Some(FeatureType::Simple),
        stages: vec![],
        dependencies: vec![],
    };
    let result = repository.update_feature(input).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_delete_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb97").unwrap();
    let result = repository.delete_feature(id).await;

    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn test_delete_non_existing_feature() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.delete_feature(id).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_get_all_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_features(team_id, None, None).await;

    assert_eq!(result.is_ok(), true);
    let features = result.unwrap();
    assert!(!features.is_empty());
}

#[tokio::test]
async fn test_name_get_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_features(team_id, Some("Test".to_string()), None)
        .await;

    assert_eq!(result.is_ok(), true);
    let features = result.unwrap();
    assert!(features.iter().all(|p| p.name.contains("Test")));
}

#[tokio::test]
async fn test_feature_type_get_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_features(team_id, None, Some(FeatureType::Simple)).await;

    assert_eq!(result.is_ok(), true);
    let features = result.unwrap();
    assert!(!features.is_empty());
    assert!(features.iter().all(|p| matches!(p.feature_type, FeatureType::Simple)));
}

#[tokio::test]
async fn test_name_and_feature_type_get_features() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_features(team_id, Some("Test".to_string()), Some(FeatureType::Simple))
        .await;

    assert_eq!(result.is_ok(), true);
    let features = result.unwrap();
    assert!(
        features
            .iter()
            .all(|p| p.name.contains("Test") && matches!(p.feature_type, FeatureType::Simple))
    );
}