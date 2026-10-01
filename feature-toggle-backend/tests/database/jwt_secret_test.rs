//! JWT signing-secret rotation and emergency deactivation, against the
//! database. Every test runs inside a transaction that is rolled back, so the
//! shared signing secret and other tests' sessions are never touched.

use chrono::{DateTime, Duration, Utc};
use feature_toggle_backend::config::AuthConfig;
use feature_toggle_backend::database::entity::JwtSecret;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jwt_secret::create_secret_tx;
use feature_toggle_backend::database::jwt_token::jwt_token_repository;
use feature_toggle_backend::database::refresh_token::refresh_token_repository;
use feature_toggle_backend::database::user::{CreateUser, user_repository};
use feature_toggle_backend::logic::jwt_secret::{jwt_secret_logic, secret_verifies_tokens};
use feature_toggle_backend::logic::jwt_secret_tx::deactivate_all_secrets_in_tx;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

type SecretRow = (
    Uuid,
    String,
    bool,
    DateTime<Utc>,
    Option<Uuid>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

async fn load_secret(conn: &mut PgConnection, id: Uuid) -> JwtSecret {
    let (id, secret, is_active, created_at, created_by, expires_at, deactivated_at, revoked_at) =
        sqlx::query_as::<_, SecretRow>(
            "SELECT id, secret, is_active, created_at, created_by, expires_at, deactivated_at, revoked_at
             FROM jwt_secrets WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .expect("load jwt secret");
    JwtSecret {
        id,
        secret,
        is_active,
        created_at,
        created_by,
        expires_at,
        deactivated_at,
        revoked_at,
    }
}

/// Makes sure a committed active secret exists and returns its id.
async fn active_secret_id(pool: &PgPool) -> Uuid {
    let logic = jwt_secret_logic(pool.clone(), AuthConfig::default());
    logic.initialize_secret().await.expect("init jwt secret");
    logic.get_signing_key().await.expect("signing key").kid
}

async fn create_user(pool: &PgPool) -> Uuid {
    let suffix = Uuid::new_v4();
    user_repository(pool.clone())
        .create_user(CreateUser {
            username: format!("jwt-secret-test-{suffix}"),
            password_hash: "x".into(),
            first_name: "Jwt".into(),
            last_name: "Secret".into(),
            email: format!("jwt-secret-test-{suffix}@example.com"),
            mobile_number: None,
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .expect("create user")
        .id
}

#[tokio::test]
async fn rotation_keeps_the_previous_secret_verifying_for_one_access_token_lifetime() {
    let pool = init_pg_pool().await;
    let previous_id = active_secret_id(&pool).await;
    let grace = AuthConfig::default().access_token_ttl();

    let mut tx = pool.begin().await.unwrap();
    let before = Utc::now();
    let current = create_secret_tx(&mut tx, format!("rotation-test-{}", Uuid::new_v4()), None)
        .await
        .expect("rotate secret");
    assert!(current.is_active);
    assert!(current.deactivated_at.is_none() && current.revoked_at.is_none());

    let previous = load_secret(&mut tx, previous_id).await;
    assert!(!previous.is_active);
    assert!(previous.revoked_at.is_none());
    let deactivated_at = previous
        .deactivated_at
        .expect("rotation sets deactivated_at");
    assert!(deactivated_at >= before - Duration::seconds(5));

    // Tokens signed by the previous secret verify within the grace window only.
    assert!(secret_verifies_tokens(&previous, Utc::now(), grace));
    assert!(secret_verifies_tokens(
        &previous,
        deactivated_at + grace - Duration::seconds(1),
        grace
    ));
    assert!(!secret_verifies_tokens(
        &previous,
        deactivated_at + grace,
        grace
    ));

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn deactivate_all_revokes_every_secret_and_session_without_grace() {
    let pool = init_pg_pool().await;
    let previous_id = active_secret_id(&pool).await;
    let user_id = create_user(&pool).await;
    let access_hash = format!("deactivate-all-access-{}", Uuid::new_v4());
    let refresh_hash = format!("deactivate-all-refresh-{}", Uuid::new_v4());
    jwt_token_repository(pool.clone())
        .store_token(
            user_id,
            access_hash.clone(),
            Utc::now() + Duration::hours(1),
        )
        .await
        .expect("store access token");
    refresh_token_repository(pool.clone())
        .create_token(
            user_id,
            Uuid::new_v4(),
            refresh_hash.clone(),
            Utc::now() + Duration::days(1),
        )
        .await
        .expect("store refresh token");
    let grace = AuthConfig::default().access_token_ttl();

    let mut tx = pool.begin().await.unwrap();
    // A rotation just happened, so the previous secret is inside its grace window.
    let current = create_secret_tx(&mut tx, format!("deactivate-test-{}", Uuid::new_v4()), None)
        .await
        .expect("rotate secret");
    let previous = load_secret(&mut tx, previous_id).await;
    assert!(secret_verifies_tokens(&previous, Utc::now(), grace));

    let outcome = deactivate_all_secrets_in_tx(&mut tx)
        .await
        .expect("deactivate all");
    assert!(outcome.secrets >= 2);
    assert!(outcome.access_tokens >= 1);
    assert!(outcome.refresh_tokens >= 1);

    // No secret verifies anything any more: not the active one, and not the
    // rotated-out one that was still within its grace window.
    let now = Utc::now();
    for id in [current.id, previous_id] {
        let secret = load_secret(&mut tx, id).await;
        assert!(!secret.is_active);
        assert!(secret.revoked_at.is_some());
        assert!(!secret_verifies_tokens(&secret, now, grace));
    }
    let usable: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jwt_secrets WHERE is_active OR revoked_at IS NULL",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(usable, 0);

    let access_revoked: bool =
        sqlx::query_scalar("SELECT is_revoked FROM jwt_tokens WHERE token_hash = $1")
            .bind(&access_hash)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(access_revoked);

    let refresh_revoked_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM refresh_tokens WHERE token_hash = $1")
            .bind(&refresh_hash)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(refresh_revoked_at.is_some());

    tx.rollback().await.unwrap();
}
