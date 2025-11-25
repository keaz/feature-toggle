use std::vec;

use feature_toggle_backend::database::pipeline::{CreatePipeline, CreateStage, UpdatePipeline};
use feature_toggle_backend::database::{init_pg_pool, pipeline};
use uuid::Uuid;

#[tokio::test]
async fn test_get_existing_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_pipeline_by_id(id).await;

    assert!(result.is_ok());
    let pipeline = result.unwrap();
    assert_eq!(pipeline.id, id);
    assert_eq!(pipeline.name, "Test Pipeline");
}

#[tokio::test]
async fn test_get_non_existing_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.get_pipeline_by_id(id).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_create_pipeline_with_stages() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_name = format!("With Stages {}", Uuid::new_v4());
    let parent = CreateStage {
        id: Uuid::new_v4(),
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 250, y: 250 }"),
    };
    let input = CreatePipeline {
        team_id,
        name: random_name.clone(),
        stages: vec![
            parent.clone(),
            CreateStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                order_index: 1,
                parent_stage: Some(Box::new(parent)),
                position: String::from("{ x: 500, y: 500 }"),
            },
        ],
    };
    let result = repository.create_pipeline(input).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_create_pipeline_with_stages_parent() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let random_name = format!("With Stages {}", Uuid::new_v4());
    let parent_stage_id = CreateStage {
        id: Uuid::new_v4(),
        environment_id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        order_index: 0,
        parent_stage: None,
        position: String::from("{ x: 250, y: 250 }"),
    };
    let input = CreatePipeline {
        team_id,
        name: random_name.clone(),
        stages: vec![
            parent_stage_id.clone(),
            CreateStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                order_index: 1,
                parent_stage: Some(Box::new(parent_stage_id)),
                position: String::from("{ x: 500, y: 500 }"),
            },
        ],
    };
    let result = repository.create_pipeline(input).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_create_exising_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let input = CreatePipeline {
        team_id,
        name: "Existing Pipeline".to_string(),
        stages: vec![],
    };
    let result = repository.create_pipeline(input).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::Error::RecordAlreadyExists(_)
    ));
}

#[tokio::test]
async fn test_update_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let input = UpdatePipeline {
        id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        name: Some("Updated Pipeline".to_string()),
        active: Some(false),
        stages: vec![],
    };
    let result = repository.update_pipeline(input).await;

    assert!(result.is_ok());
    let pipeline = result.unwrap();
    assert_eq!(pipeline.name, "Updated Pipeline");
    assert!(!pipeline.active);
}

#[tokio::test]
async fn test_update_non_existing_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let input = UpdatePipeline {
        id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap(),
        name: Some("Non-existing Pipeline".to_string()),
        active: Some(true),
        stages: vec![],
    };
    let result = repository.update_pipeline(input).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_delete_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let pipeline_id = repository
        .create_pipeline(CreatePipeline {
            team_id,
            name: format!("Delete Pipeline {}", Uuid::new_v4()),
            stages: vec![],
        })
        .await
        .expect("pipeline to be created");

    let result = repository.delete_pipeline(pipeline_id).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_delete_non_existing_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.delete_pipeline(id).await;

    assert!(result.is_err());
    let error = result.err().unwrap();
    assert!(matches!(error, feature_toggle_backend::Error::NotFound(_)));
}

#[tokio::test]
async fn test_get_all_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_pipelines(team_id, None, None).await;

    assert!(result.is_ok());
    let pipelines = result.unwrap();
    assert!(!pipelines.is_empty());
}

#[tokio::test]
async fn test_name_get_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_pipelines(team_id, Some("Test".to_string()), None)
        .await;

    assert!(result.is_ok());
    let pipelines = result.unwrap();
    assert!(pipelines.iter().all(|p| p.name.starts_with("Test")));
}

#[tokio::test]
async fn test_inactive_get_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_pipelines(team_id, None, Some(false)).await;

    assert!(result.is_ok());
    let pipelines = result.unwrap();
    assert!(!pipelines.is_empty());
    assert!(pipelines.iter().all(|p| !p.active));
}

#[tokio::test]
async fn test_name_and_active_get_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository
        .get_pipelines(team_id, Some("Test".to_string()), Some(true))
        .await;

    assert!(result.is_ok());
    let pipelines = result.unwrap();
    assert!(
        pipelines
            .iter()
            .all(|p| p.name.starts_with("Test") && p.active)
    );
}
