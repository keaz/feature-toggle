//! Disabled users must not be able to authenticate, and disabling a user must
//! revoke every session token they hold.

use chrono::{Duration, Utc};
use feature_toggle_backend::Error;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jwt_token::jwt_token_repository;
use feature_toggle_backend::database::user::{CreateUser, user_repository, user_repository_tx};
use feature_toggle_backend::logic::user::{UpdateUserInput, user_logic};
use feature_toggle_backend::logic::user_tx::update_user_in_tx;
use feature_toggle_backend::model::ID;
use sqlx::PgPool;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery staple";

fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hash password")
        .to_string()
}

async fn create_user(pool: &PgPool) -> (Uuid, String) {
    let suffix = Uuid::new_v4();
    let username = format!("disable-test-{suffix}");
    let created = user_repository(pool.clone())
        .create_user(CreateUser {
            username: username.clone(),
            password_hash: hash_password(PASSWORD),
            first_name: "Dis".into(),
            last_name: "Able".into(),
            email: format!("disable-test-{suffix}@example.com"),
            mobile_number: None,
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .expect("create user");
    (created.id, username)
}

async fn store_tokens(pool: &PgPool, user_id: Uuid, count: usize) -> Vec<String> {
    let repo = jwt_token_repository(pool.clone());
    let mut hashes = Vec::new();
    for _ in 0..count {
        let hash = format!("disable-test-token-{}", Uuid::new_v4());
        repo.store_token(user_id, hash.clone(), Utc::now() + Duration::hours(1))
            .await
            .expect("store token");
        hashes.push(hash);
    }
    hashes
}

async fn disable_via_api_path(pool: &PgPool, user_id: Uuid, enabled: bool) {
    let repo = user_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin tx");
    update_user_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        ID::from(user_id),
        UpdateUserInput {
            first_name: None,
            last_name: None,
            email: None,
            mobile_number: None,
            is_admin: None,
            enabled: Some(enabled),
        },
        None,
    )
    .await
    .expect("update user");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn login_with_disabled_user_fails_with_account_disabled() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let logic = user_logic(
        user_repository(pool.clone()),
        activity_log_repository(pool.clone()),
    );

    // Enabled user can authenticate.
    logic
        .authenticate_user(username.clone(), PASSWORD.to_string())
        .await
        .expect("enabled user authenticates");

    disable_via_api_path(&pool, user_id, false).await;

    let err = logic
        .authenticate_user(username.clone(), PASSWORD.to_string())
        .await
        .expect_err("disabled user must be rejected");
    assert!(matches!(err, Error::AccountDisabled), "got {err:?}");

    // A wrong password must not reveal that the account is disabled.
    let err = logic
        .authenticate_user(username.clone(), "wrong password".to_string())
        .await
        .expect_err("wrong password must be rejected");
    assert!(matches!(err, Error::Unauthorized(_)), "got {err:?}");

    // Re-enabling restores access.
    disable_via_api_path(&pool, user_id, true).await;
    logic
        .authenticate_user(username, PASSWORD.to_string())
        .await
        .expect("re-enabled user authenticates");
}

#[tokio::test]
async fn disabling_a_user_revokes_all_their_tokens() {
    let pool = init_pg_pool().await;
    let (user_id, _) = create_user(&pool).await;
    let (other_user_id, _) = create_user(&pool).await;
    let hashes = store_tokens(&pool, user_id, 3).await;
    let other_hashes = store_tokens(&pool, other_user_id, 1).await;
    let tokens = jwt_token_repository(pool.clone());

    for hash in &hashes {
        assert!(tokens.is_token_valid(hash).await.unwrap());
    }

    disable_via_api_path(&pool, user_id, false).await;

    for hash in &hashes {
        assert!(
            !tokens.is_token_valid(hash).await.unwrap(),
            "token issued before disabling must be rejected afterwards"
        );
    }
    let revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jwt_tokens WHERE user_id = $1 AND is_revoked = TRUE AND revoked_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revoked, 3);

    // Other users' sessions are untouched.
    assert!(tokens.is_token_valid(&other_hashes[0]).await.unwrap());
}

#[tokio::test]
async fn updating_a_user_without_disabling_keeps_tokens() {
    let pool = init_pg_pool().await;
    let (user_id, _) = create_user(&pool).await;
    let hashes = store_tokens(&pool, user_id, 1).await;
    let tokens = jwt_token_repository(pool.clone());

    // enabled = true (or unset) must not touch sessions.
    disable_via_api_path(&pool, user_id, true).await;
    assert!(tokens.is_token_valid(&hashes[0]).await.unwrap());
}

#[tokio::test]
async fn token_revocation_rolls_back_with_the_transaction() {
    let pool = init_pg_pool().await;
    let (user_id, _) = create_user(&pool).await;
    let hashes = store_tokens(&pool, user_id, 1).await;

    let repo = user_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    update_user_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        ID::from(user_id),
        UpdateUserInput {
            first_name: None,
            last_name: None,
            email: None,
            mobile_number: None,
            is_admin: None,
            enabled: Some(false),
        },
        None,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    // Neither the disable nor the revocation was committed.
    assert!(
        user_repository(pool.clone())
            .get_user_by_id(user_id)
            .await
            .unwrap()
            .enabled
    );
    assert!(
        jwt_token_repository(pool)
            .is_token_valid(&hashes[0])
            .await
            .unwrap()
    );
}
