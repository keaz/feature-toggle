use feature_toggle_backend::database::pipeline::{CreatePipeline, UpdatePipeline};
use feature_toggle_backend::database::{init_pg_pool, pipeline};
use uuid::Uuid;

#[tokio::test]
async fn test_get_existing_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_pipeline_by_id(id).await;

    assert_eq!(result.is_ok(), true);
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

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::database::Error::NotFound(_)
    ));
}

#[tokio::test]
async fn test_create_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let random_name = format!("New Pipeline {}", uuid::Uuid::new_v4());
    let input = CreatePipeline {
        name: random_name.clone(),
    };
    let result = repository.create_pipeline(input).await;

    assert_eq!(result.is_ok(), true);
    let pipeline = result.unwrap();
    assert_eq!(pipeline.name, random_name);
    assert!(pipeline.active);
}

#[tokio::test]
async fn test_create_exising_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let input = CreatePipeline {
        name: "Existing Pipeline".to_string(),
    };
    let result = repository.create_pipeline(input).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::database::Error::RecordAlreadyExists(_)
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
    };
    let result = repository.update_pipeline(input).await;

    assert_eq!(result.is_ok(), true);
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
    };
    let result = repository.update_pipeline(input).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::database::Error::NotFound(_)
    ));
}

#[tokio::test]
async fn test_delete_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb97").unwrap();
    let result = repository.delete_pipeline(id).await;

    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn test_delete_non_existing_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.delete_pipeline(id).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::database::Error::NotFound(_)
    ));
}

#[tokio::test]
async fn test_get_all_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let result = repository.get_pipelines(None, None).await;

    assert_eq!(result.is_ok(), true);
    let pipelines = result.unwrap();
    assert!(!pipelines.is_empty());
}

#[tokio::test]
async fn test_name_get_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let result = repository
        .get_pipelines(Some("Test".to_string()), None)
        .await;

    assert_eq!(result.is_ok(), true);
    let pipelines = result.unwrap();
    assert!(pipelines.iter().all(|p| p.name.starts_with("Test")));
}

#[tokio::test]
async fn test_inactive_get_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let result = repository.get_pipelines(None, Some(false)).await;

    assert_eq!(result.is_ok(), true);
    let pipelines = result.unwrap();
    assert!(!pipelines.is_empty());
    assert!(pipelines.iter().all(|p| p.active == false));
}

#[tokio::test]
async fn test_name_and_active_get_pipelines() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let result = repository
        .get_pipelines(Some("Test".to_string()), Some(true))
        .await;

    assert_eq!(result.is_ok(), true);
    let pipelines = result.unwrap();
    assert!(
        pipelines
            .iter()
            .all(|p| p.name.starts_with("Test") && p.active == true)
    );
}
