use feature_toggle_backend::database::{environment, init_pg_pool};
use uuid::Uuid;

#[tokio::test]
async fn test_get_existing_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let result = repository.get_environment_by_id(id).await;

    assert_eq!(result.is_ok(), true);
    let environment = result.unwrap();
    assert_eq!(environment.id, id);
    assert_eq!(environment.name, "Test Environment");
}

#[tokio::test]
async fn test_get_not_found_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.get_environment_by_id(id).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::database::Error::NotFound(_)
    ));
}
