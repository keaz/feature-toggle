use chrono::{TimeZone, Utc};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::user::{user_repository, CreateUser, UpdateUser, UserRepository};
use feature_toggle_backend::Error;
use uuid::Uuid;

// Helper to init repo
async fn repo() -> Box<dyn UserRepository> {
    let pool = init_pg_pool().await;
    user_repository(pool)
}

#[tokio::test]
async fn get_user_by_id_and_username_and_email() {
    let repo = repo().await;
    // Seeded in init.sql
    let admin_id: Uuid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap();

    let by_id = repo.get_user_by_id(admin_id).await.unwrap();
    assert_eq!(by_id.username, "admin");
    assert_eq!(by_id.email, "admin@example.com");

    let by_username = repo.get_user_by_username("admin").await.unwrap();
    assert_eq!(by_username.id, admin_id);

    let by_email = repo.get_user_by_email("admin@example.com").await.unwrap();
    assert_eq!(by_email.id, admin_id);
}

#[tokio::test]
async fn existence_checks() {
    let repo = repo().await;
    assert!(repo.user_exists_by_username("admin").await.unwrap());
    assert!(repo.user_exists_by_email("admin@example.com", None).await.unwrap());

    // Exclude same id -> should be false for conflict
    let admin_id: Uuid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap();
    assert!(!repo.user_exists_by_email("admin@example.com", Some(admin_id)).await.unwrap());
}

#[tokio::test]
async fn create_and_update_user_and_last_login() {
    let repo = repo().await;
    // Create
    let created = repo.create_user(CreateUser {
        username: "charlie".into(),
        password_hash: "hash".into(),
        first_name: "Charlie".into(),
        last_name: "Chaplin".into(),
        email: "charlie@example.com".into(),
        is_admin: false,
    }).await.unwrap();

    assert_eq!(created.username, "charlie");
    assert_eq!(created.email, "charlie@example.com");

    // Update some fields
    let updated = repo.update_user(UpdateUser {
        id: created.id,
        first_name: Some("Charles".into()),
        last_name: None,
        email: Some("charles@example.com".into()),
        is_admin: Some(true),
        enabled: Some(true),
    }).await.unwrap();

    assert_eq!(updated.first_name, "Charles");
    assert_eq!(updated.email, "charles@example.com");
    assert!(updated.is_admin);

    // Update last login
    let when = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    repo.update_last_login(updated.id, when).await.unwrap();
    let fetched = repo.get_user_by_id(updated.id).await.unwrap();
    assert!(fetched.last_login.is_some());
}

#[tokio::test]
async fn unique_violation_is_mapped() {
    let repo = repo().await;
    // Try to create another user with existing username "admin"
    let err = repo.create_user(CreateUser{
        username: "admin".into(),
        password_hash: "hash".into(),
        first_name: "Other".into(),
        last_name: "User".into(),
        email: "other@example.com".into(),
        is_admin: false,
    }).await.err().unwrap();

    match err { Error::RecordAlreadyExists(field) => assert_eq!(field, "username"), _ => panic!("unexpected error: {:?}", err) }

    // Try duplicate email
    let err2 = repo.create_user(CreateUser{
        username: "someone".into(),
        password_hash: "hash".into(),
        first_name: "Any".into(),
        last_name: "Body".into(),
        email: "admin@example.com".into(),
        is_admin: false,
    }).await.err().unwrap();

    match err2 { Error::RecordAlreadyExists(field) => assert_eq!(field, "email"), _ => panic!("unexpected error: {:?}", err2) }
}
