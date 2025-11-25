use chrono::{TimeZone, Utc};
use feature_toggle_backend::Error;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::user::{
    CreateUser, UpdateUser, UserRepository, user_repository,
};
use sqlx::Row;
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
    assert!(
        repo.user_exists_by_email("admin@example.com", None)
            .await
            .unwrap()
    );

    // Exclude same id -> should be false for conflict
    let admin_id: Uuid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse().unwrap();
    assert!(
        !repo
            .user_exists_by_email("admin@example.com", Some(admin_id))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn create_and_update_user_and_last_login() {
    let repo = repo().await;
    let suffix = Uuid::new_v4().to_string();
    let username = format!("charlie-{suffix}");
    let email = format!("charlie-{suffix}@example.com");
    // Create
    let created = repo
        .create_user(CreateUser {
            username: username.clone(),
            password_hash: "hash".into(),
            first_name: "Charlie".into(),
            last_name: "Chaplin".into(),
            email: email.clone(),
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .unwrap();

    assert_eq!(created.username, username);
    assert_eq!(created.email, email);

    // Update some fields
    let updated_email = format!("charles-{suffix}@example.com");
    let updated = repo
        .update_user(UpdateUser {
            id: created.id,
            first_name: Some("Charles".into()),
            last_name: None,
            email: Some(updated_email.clone()),
            is_admin: Some(true),
            enabled: Some(true),
        })
        .await
        .unwrap();

    assert_eq!(updated.first_name, "Charles");
    assert_eq!(updated.email, updated_email);
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
    let err = repo
        .create_user(CreateUser {
            username: "admin".into(),
            password_hash: "hash".into(),
            first_name: "Other".into(),
            last_name: "User".into(),
            email: "other@example.com".into(),
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .err()
        .unwrap();

    match err {
        Error::RecordAlreadyExists(field) => assert_eq!(field, "username"),
        _ => panic!("unexpected error: {:?}", err),
    }

    // Try duplicate email
    let err2 = repo
        .create_user(CreateUser {
            username: "someone".into(),
            password_hash: "hash".into(),
            first_name: "Any".into(),
            last_name: "Body".into(),
            email: "admin@example.com".into(),
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .err()
        .unwrap();

    match err2 {
        Error::RecordAlreadyExists(field) => assert_eq!(field, "email"),
        _ => panic!("unexpected error: {:?}", err2),
    }
}

#[tokio::test]
async fn assign_user_teams_replaces_assignments() {
    let pool = init_pg_pool().await;
    let repo = user_repository(pool.clone());

    let user_id: Uuid = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".parse().unwrap(); // bob from seed
    let team_a: Uuid = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27".parse().unwrap(); // Test Team
    let team_b: Uuid = "3eef17bc-9e06-411d-b5f4-7a786e68bb96".parse().unwrap(); // Update Team

    // Ensure some initial state: set a single team
    repo.set_user_teams(user_id, vec![team_a]).await.unwrap();

    // Verify one row
    let count1 = sqlx::query("SELECT COUNT(*) AS c FROM user_teams WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get::<i64, _>("c");
    assert_eq!(count1, 1);

    // Now replace with two teams
    repo.set_user_teams(user_id, vec![team_a, team_b])
        .await
        .unwrap();

    // Verify two rows
    let count2 = sqlx::query("SELECT COUNT(*) AS c FROM user_teams WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get::<i64, _>("c");
    assert_eq!(count2, 2);

    // Replace with zero -> should delete all
    repo.set_user_teams(user_id, vec![]).await.unwrap();
    let count3 = sqlx::query("SELECT COUNT(*) AS c FROM user_teams WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get::<i64, _>("c");
    assert_eq!(count3, 0);
}
