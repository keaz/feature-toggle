use async_graphql::ID;
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
    let input = feature_toggle_shared::graphql::CreatePipelineInput {
        name: random_name.clone(),
    };
    let result = repository.create_pipeline(input).await;

    assert_eq!(result.is_ok(), true);
    let pipeline = result.unwrap();
    assert_eq!(pipeline.name, random_name);
    assert!(pipeline.active);
}

#[tokio::test]
async fn test_update_pipeline() {
    let pool = init_pg_pool().await;
    let repository = pipeline::pipeline_repository(pool);

    let input = feature_toggle_shared::graphql::UpdatePipelineInput {
        id: ID::from("3eef17bc-9e06-411d-b5f4-7a786e68bb96"),
        name: Some("Updated Pipeline".to_string()),
        active: Some(false),
    };
    let result = repository.update_pipeline(input).await;

    assert_eq!(result.is_ok(), true);
    let pipeline = result.unwrap();
    assert_eq!(pipeline.name, "Updated Pipeline");
    assert!(!pipeline.active);
}
