use feature_toggle_backend::database::environment::{CreateEnvironment, UpdateEnvironment};
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
        feature_toggle_backend::Error::NotFound(_)
    ));
}

#[tokio::test]
async fn test_create_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let input = CreateEnvironment {
        name: "New Environment".to_string(),
        active: true,
    };
    let result = repository.create_environment(input).await;

    assert_eq!(result.is_ok(), true);
    let environment = result.unwrap();
    assert_eq!(environment.name, "New Environment");
    assert!(environment.active);
}

#[tokio::test]
async fn test_update_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let input = UpdateEnvironment {
        id: Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(),
        name: Some("Updated Environment".to_string()),
        active: Some(false),
    };
    let result = repository.update_environment(input).await;

    assert_eq!(result.is_ok(), true);
    let environment = result.unwrap();
    assert_eq!(environment.name, "Updated Environment");
    assert_eq!(environment.active, false);
}

#[tokio::test]
async fn test_not_found_update_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let input = UpdateEnvironment {
        id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap(),
        name: Some("Non-existent Environment".to_string()),
        active: Some(true),
    };
    let result = repository.update_environment(input).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::Error::NotFound(_)
    ));
}

#[tokio::test]
async fn test_delete_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let id = Uuid::parse_str("1ab6ca79-a4fc-44ba-87e2-12884edf17f7").unwrap();
    let result = repository.delete_environment(id).await;

    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn test_not_found_delete_environment() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98fca").unwrap();
    let result = repository.delete_environment(id).await;

    assert_eq!(result.is_err(), true);
    let error = result.err().unwrap();
    assert!(matches!(
        error,
        feature_toggle_backend::Error::NotFound(_)
    ));
}

#[tokio::test]
async fn test_non_param_get_environments() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let result = repository.get_environments(None, None).await;

    assert_eq!(result.is_ok(), true);
    let environments = result.unwrap();
    assert!(!environments.is_empty());
    assert!(
        environments
            .iter()
            .any(|env| env.name == "Test Environment")
    );
}

#[tokio::test]
async fn test_active_param_get_environments() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let result = repository.get_environments(None, Some(true)).await;

    assert_eq!(result.is_ok(), true);
    let environments = result.unwrap();
    assert!(!environments.is_empty());
    assert!(environments.iter().all(|env| env.active));
}

#[tokio::test]
async fn test_name_param_get_environments() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let result = repository
        .get_environments(Some("Test".to_string()), None)
        .await;

    assert_eq!(result.is_ok(), true);
    let environments = result.unwrap();
    assert!(!environments.is_empty());
    assert!(environments.iter().all(|env| env.name.contains("Test")));
}

#[tokio::test]
async fn test_name_and_active_param_get_environments() {
    let pool = init_pg_pool().await;
    let repository = environment::environment_repository(pool);

    let result = repository
        .get_environments(Some("Test".to_string()), Some(true))
        .await;

    assert_eq!(result.is_ok(), true);
    let environments = result.unwrap();
    assert!(!environments.is_empty());
    assert!(
        environments
            .iter()
            .all(|env| env.name.contains("Test") && env.active)
    );
}
