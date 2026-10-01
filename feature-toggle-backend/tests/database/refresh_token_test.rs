//! Short-lived access tokens with rotating refresh tokens: login, rotation,
//! reuse detection (with the concurrent-refresh grace window), logout,
//! disable, password reset and cleanup.

use chrono::{DateTime, Duration, Utc};
use feature_toggle_backend::config::AuthConfig;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jwt_secret::create_secret_tx;
use feature_toggle_backend::database::jwt_token::{
    NewSession, jwt_token_repository, store_session_tx,
};
use feature_toggle_backend::database::refresh_token::refresh_token_repository;
use feature_toggle_backend::database::role::{role_repository, role_repository_tx};
use feature_toggle_backend::database::user::{CreateUser, user_repository, user_repository_tx};
use feature_toggle_backend::logic::jwt_secret::{SigningKey, jwt_secret_logic};
use feature_toggle_backend::logic::jwt_secret_tx::deactivate_all_secrets_in_tx;
use feature_toggle_backend::logic::jwt_token::{JwtTokenLogic, LoginResult, jwt_token_logic};
use feature_toggle_backend::logic::jwt_token_tx::{
    RefreshOutcome, RefreshRejection, refresh_session_in_tx,
};
use feature_toggle_backend::logic::role::role_logic;
use feature_toggle_backend::logic::user::{UpdateUserInput, user_logic};
use feature_toggle_backend::logic::user_tx::{reset_password_in_tx, update_user_in_tx};
use feature_toggle_backend::model::ID;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

const PASSWORD: &str = "refresh test password";
const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

#[derive(Debug, Deserialize)]
struct Claims {
    exp: i64,
    iat: i64,
    is_admin: bool,
    roles: Vec<String>,
}

fn sha256_hex(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

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
    let username = format!("refresh-test-{suffix}");
    let created = user_repository(pool.clone())
        .create_user(CreateUser {
            username: username.clone(),
            password_hash: hash_password(PASSWORD),
            first_name: "Re".into(),
            last_name: "Fresh".into(),
            email: format!("refresh-test-{suffix}@example.com"),
            mobile_number: None,
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .expect("create user");
    (created.id, username)
}

async fn secret(pool: &PgPool) -> SigningKey {
    let logic = jwt_secret_logic(pool.clone(), AuthConfig::default());
    logic.initialize_secret().await.expect("init jwt secret");
    logic.get_signing_key().await.expect("jwt signing key")
}

fn token_logic(pool: &PgPool, auth: AuthConfig) -> Box<dyn JwtTokenLogic> {
    jwt_token_logic(
        jwt_token_repository(pool.clone()),
        refresh_token_repository(pool.clone()),
        user_logic(
            user_repository(pool.clone()),
            activity_log_repository(pool.clone()),
        ),
        role_logic(
            role_repository(pool.clone()),
            activity_log_repository(pool.clone()),
        ),
        jwt_secret_logic(pool.clone(), auth),
        auth,
    )
}

async fn login(pool: &PgPool, username: &str, auth: AuthConfig) -> LoginResult {
    secret(pool).await;
    token_logic(pool, auth)
        .login_user(username.to_string(), PASSWORD.to_string())
        .await
        .expect("login")
}

/// Runs one refresh in its own committed transaction, as the REST handler does.
async fn refresh(pool: &PgPool, token: &str, auth: AuthConfig) -> RefreshOutcome {
    secret(pool).await;
    let mut tx = pool.begin().await.unwrap();
    let outcome = refresh_session_in_tx(
        &mut tx,
        &user_repository_tx(pool.clone()),
        &role_repository_tx(pool.clone()),
        &auth,
        token,
    )
    .await
    .expect("refresh");
    tx.commit().await.unwrap();
    outcome
}

fn rotated(outcome: RefreshOutcome) -> LoginResult {
    match outcome {
        RefreshOutcome::Rotated(result) => result,
        RefreshOutcome::Rejected(rejection) => panic!("expected rotation, got {rejection:?}"),
    }
}

fn rejection(outcome: RefreshOutcome) -> RefreshRejection {
    match outcome {
        RefreshOutcome::Rejected(rejection) => rejection,
        RefreshOutcome::Rotated(_) => panic!("expected rejection, got rotation"),
    }
}

async fn decode_claims(pool: &PgPool, token: &str) -> Claims {
    let key = secret(pool).await;
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_issuer(&["fluxgate"]);
    validation.set_audience(&["fluxgate-api"]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    let header = jsonwebtoken::decode_header(token).expect("decode header");
    assert_eq!(header.kid, Some(key.kid.to_string()));
    jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(key.secret.as_bytes()),
        &validation,
    )
    .expect("decode access token")
    .claims
}

struct Row {
    id: Uuid,
    user_id: Uuid,
    family_id: Uuid,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    replaced_by: Option<Uuid>,
}

async fn row(pool: &PgPool, raw_token: &str) -> Row {
    let (id, user_id, family_id, expires_at, revoked_at, replaced_by) = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            Uuid,
            DateTime<Utc>,
            Option<DateTime<Utc>>,
            Option<Uuid>,
        ),
    >(
        "SELECT id, user_id, family_id, expires_at, revoked_at, replaced_by
         FROM refresh_tokens WHERE token_hash = $1",
    )
    .bind(sha256_hex(raw_token))
    .fetch_one(pool)
    .await
    .expect("refresh token row");
    Row {
        id,
        user_id,
        family_id,
        expires_at,
        revoked_at,
        replaced_by,
    }
}

async fn backdate_revocation(pool: &PgPool, raw_token: &str, seconds: i64) {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = now() - make_interval(secs => $2)
         WHERE token_hash = $1",
    )
    .bind(sha256_hex(raw_token))
    .bind(seconds as f64)
    .execute(pool)
    .await
    .unwrap();
}

async fn access_token_expires_at(pool: &PgPool, access_token: &str) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT expires_at FROM jwt_tokens WHERE token_hash = $1")
        .bind(sha256_hex(access_token))
        .fetch_one(pool)
        .await
        .expect("jwt_tokens row")
}

async fn set_enabled(pool: &PgPool, user_id: Uuid, enabled: Option<bool>, is_admin: Option<bool>) {
    let mut tx = pool.begin().await.unwrap();
    update_user_in_tx(
        &mut tx,
        &user_repository_tx(pool.clone()),
        activity_log_repository(pool.clone()).as_ref(),
        ID::from(user_id),
        UpdateUserInput {
            first_name: None,
            last_name: None,
            email: None,
            mobile_number: None,
            is_admin,
            enabled,
        },
        None,
    )
    .await
    .expect("update user");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn login_returns_a_refresh_token_stored_only_as_hash() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let auth = AuthConfig::default();

    let before = Utc::now();
    let result = login(&pool, &username, auth).await;

    assert_eq!(result.refresh_token.len(), 43);
    assert_eq!(result.expires_in, 30 * 60);
    let stored = row(&pool, &result.refresh_token).await;
    assert_eq!(stored.user_id, user_id);
    assert!(stored.revoked_at.is_none());
    assert!(stored.replaced_by.is_none());
    assert!(stored.expires_at >= before + Duration::days(7) - Duration::seconds(1));
    assert!(stored.expires_at <= Utc::now() + Duration::days(7));

    let raw_stored: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM refresh_tokens WHERE token_hash = $1")
            .bind(&result.refresh_token)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(raw_stored, 0, "the raw refresh token must never be stored");

    // A second login starts a separate family.
    let second = login(&pool, &username, auth).await;
    assert_ne!(
        row(&pool, &second.refresh_token).await.family_id,
        stored.family_id
    );
}

#[tokio::test]
async fn access_token_expiry_honours_config() {
    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig {
        access_token_ttl_minutes: 5,
        refresh_token_ttl_days: 1,
    };

    let login_result = login(&pool, &username, auth).await;
    assert_eq!(login_result.expires_in, 300);
    let claims = decode_claims(&pool, &login_result.token).await;
    assert_eq!(claims.exp - claims.iat, 300);
    assert_eq!(
        access_token_expires_at(&pool, &login_result.token)
            .await
            .timestamp(),
        claims.exp
    );
    let refresh_expires = row(&pool, &login_result.refresh_token).await.expires_at;
    assert!(
        (refresh_expires - Utc::now() - Duration::days(1))
            .num_seconds()
            .abs()
            <= 5
    );

    let refreshed = rotated(refresh(&pool, &login_result.refresh_token, auth).await);
    assert_eq!(refreshed.expires_in, 300);
    let claims = decode_claims(&pool, &refreshed.token).await;
    assert_eq!(claims.exp - claims.iat, 300);
    assert_eq!(
        access_token_expires_at(&pool, &refreshed.token)
            .await
            .timestamp(),
        claims.exp
    );
    let refresh_expires = row(&pool, &refreshed.refresh_token).await.expires_at;
    assert!(
        (refresh_expires - Utc::now() - Duration::days(1))
            .num_seconds()
            .abs()
            <= 5
    );
}

#[tokio::test]
async fn refresh_rotates_and_reuse_after_grace_revokes_the_family() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let first = login(&pool, &username, auth).await;

    let second = rotated(refresh(&pool, &first.refresh_token, auth).await);
    assert_ne!(second.refresh_token, first.refresh_token);
    assert_ne!(second.token, first.token);
    assert_eq!(second.user.username, username);
    assert!(
        jwt_token_repository(pool.clone())
            .is_token_valid(&sha256_hex(&second.token))
            .await
            .unwrap()
    );

    let old = row(&pool, &first.refresh_token).await;
    let new = row(&pool, &second.refresh_token).await;
    assert!(old.revoked_at.is_some());
    assert_eq!(old.replaced_by, Some(new.id));
    assert_eq!(new.family_id, old.family_id);
    assert_eq!(new.user_id, user_id);
    assert!(new.revoked_at.is_none());

    // Presenting the rotated token outside the grace window is reuse.
    backdate_revocation(&pool, &first.refresh_token, 30).await;
    assert_eq!(
        rejection(refresh(&pool, &first.refresh_token, auth).await),
        RefreshRejection::Reused
    );

    // ...and it killed the whole family, including the legitimate successor.
    assert!(row(&pool, &second.refresh_token).await.revoked_at.is_some());
    assert_eq!(
        rejection(refresh(&pool, &second.refresh_token, auth).await),
        RefreshRejection::Reused
    );
}

#[tokio::test]
async fn reuse_within_grace_window_issues_a_new_token_and_keeps_the_family() {
    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let first = login(&pool, &username, auth).await;

    let second = rotated(refresh(&pool, &first.refresh_token, auth).await);
    // A second tab presents the same (just rotated) token.
    let concurrent = rotated(refresh(&pool, &first.refresh_token, auth).await);
    assert_ne!(concurrent.refresh_token, second.refresh_token);

    let old = row(&pool, &first.refresh_token).await;
    let second_row = row(&pool, &second.refresh_token).await;
    let concurrent_row = row(&pool, &concurrent.refresh_token).await;
    // The presented token stays revoked and still points at its first successor.
    assert!(old.revoked_at.is_some());
    assert_eq!(old.replaced_by, Some(second_row.id));
    assert_eq!(concurrent_row.family_id, old.family_id);

    // The family stays valid: both successors can be used.
    rotated(refresh(&pool, &second.refresh_token, auth).await);
    rotated(refresh(&pool, &concurrent.refresh_token, auth).await);
}

#[tokio::test]
async fn reuse_of_a_logout_revoked_token_is_reported_as_reuse() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let logic = token_logic(&pool, auth);

    // Revoked by logout (no replaced_by): reuse immediately, no grace.
    let session = login(&pool, &username, auth).await;
    assert_eq!(
        logic
            .revoke_refresh_token_family(user_id, &session.refresh_token)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        rejection(refresh(&pool, &session.refresh_token, auth).await),
        RefreshRejection::Reused
    );

    // A token rotated a moment ago gets no grace once its family was logged out.
    let session = login(&pool, &username, auth).await;
    let successor = rotated(refresh(&pool, &session.refresh_token, auth).await);
    logic
        .revoke_refresh_token_family(user_id, &successor.refresh_token)
        .await
        .unwrap();
    assert_eq!(
        rejection(refresh(&pool, &session.refresh_token, auth).await),
        RefreshRejection::Reused
    );
}

#[tokio::test]
async fn logout_with_refresh_token_revokes_the_family_only_for_its_owner() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let (other_user_id, _) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let logic = token_logic(&pool, auth);

    let session = login(&pool, &username, auth).await;
    let successor = rotated(refresh(&pool, &session.refresh_token, auth).await);
    let other_session = login(&pool, &username, auth).await;

    // Another user cannot revoke this family.
    assert_eq!(
        logic
            .revoke_refresh_token_family(other_user_id, &successor.refresh_token)
            .await
            .unwrap(),
        0
    );
    assert!(
        row(&pool, &successor.refresh_token)
            .await
            .revoked_at
            .is_none()
    );

    assert_eq!(
        logic
            .revoke_refresh_token_family(user_id, &successor.refresh_token)
            .await
            .unwrap(),
        1
    );
    assert!(
        row(&pool, &successor.refresh_token)
            .await
            .revoked_at
            .is_some()
    );
    assert_eq!(
        rejection(refresh(&pool, &successor.refresh_token, auth).await),
        RefreshRejection::Reused
    );

    // Other sessions (families) of the same user are untouched.
    rotated(refresh(&pool, &other_session.refresh_token, auth).await);
}

#[tokio::test]
async fn disabled_user_cannot_refresh() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let session = login(&pool, &username, auth).await;

    set_enabled(&pool, user_id, Some(false), None).await;

    // Disabling revoked the user's refresh tokens along with the access tokens.
    assert!(
        row(&pool, &session.refresh_token)
            .await
            .revoked_at
            .is_some()
    );
    assert_eq!(
        rejection(refresh(&pool, &session.refresh_token, auth).await),
        RefreshRejection::Invalid
    );

    // Even an unrevoked token is refused while the user is disabled.
    sqlx::query("UPDATE refresh_tokens SET revoked_at = NULL WHERE token_hash = $1")
        .bind(sha256_hex(&session.refresh_token))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        rejection(refresh(&pool, &session.refresh_token, auth).await),
        RefreshRejection::Invalid
    );
}

#[tokio::test]
async fn unknown_and_expired_refresh_tokens_are_invalid() {
    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig::default();

    assert_eq!(
        rejection(refresh(&pool, "not-a-real-token", auth).await),
        RefreshRejection::Invalid
    );

    let session = login(&pool, &username, auth).await;
    sqlx::query(
        "UPDATE refresh_tokens SET expires_at = now() - INTERVAL '1 minute' WHERE token_hash = $1",
    )
    .bind(sha256_hex(&session.refresh_token))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        rejection(refresh(&pool, &session.refresh_token, auth).await),
        RefreshRejection::Invalid
    );
    // An expired token is not rotated.
    assert!(
        row(&pool, &session.refresh_token)
            .await
            .revoked_at
            .is_none()
    );
}

#[tokio::test]
async fn refresh_picks_up_role_and_admin_changes_made_after_login() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let session = login(&pool, &username, auth).await;

    let claims = decode_claims(&pool, &session.token).await;
    assert!(claims.roles.is_empty());
    assert!(!claims.is_admin);

    role_repository(pool.clone())
        .assign_user_roles(
            user_id,
            vec![Uuid::parse_str(APPROVER_ROLE_ID).unwrap()],
            None,
        )
        .await
        .expect("assign role");
    set_enabled(&pool, user_id, None, Some(true)).await;

    let refreshed = rotated(refresh(&pool, &session.refresh_token, auth).await);
    let claims = decode_claims(&pool, &refreshed.token).await;
    assert_eq!(claims.roles, vec!["Approver".to_string()]);
    assert!(claims.is_admin);
    assert!(refreshed.user.is_admin);
}

#[tokio::test]
async fn password_reset_revokes_all_refresh_tokens_of_the_user() {
    let pool = init_pg_pool().await;
    let (user_id, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let first = login(&pool, &username, auth).await;
    let second = login(&pool, &username, auth).await;

    let mut tx = pool.begin().await.unwrap();
    reset_password_in_tx(
        &mut tx,
        &user_repository_tx(pool.clone()),
        activity_log_repository(pool.clone()).as_ref(),
        ID::from(user_id),
        PASSWORD.to_string(),
        "a brand new password".to_string(),
        None,
    )
    .await
    .expect("reset password");
    tx.commit().await.unwrap();

    for session in [&first, &second] {
        assert!(
            row(&pool, &session.refresh_token)
                .await
                .revoked_at
                .is_some()
        );
        assert_eq!(
            rejection(refresh(&pool, &session.refresh_token, auth).await),
            RefreshRejection::Reused
        );
    }
}

#[tokio::test]
async fn concurrent_refreshes_with_one_token_are_serialized() {
    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let session = login(&pool, &username, auth).await;

    // Every task opens its transaction first, then all refresh at once.
    const TASKS: usize = 4;
    secret(&pool).await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(TASKS));
    let mut handles = Vec::new();
    for _ in 0..TASKS {
        let pool = pool.clone();
        let token = session.refresh_token.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            barrier.wait().await;
            let outcome = refresh_session_in_tx(
                &mut tx,
                &user_repository_tx(pool.clone()),
                &role_repository_tx(pool.clone()),
                &auth,
                &token,
            )
            .await
            .expect("refresh");
            tx.commit().await.unwrap();
            outcome
        }));
    }
    let mut new_tokens = Vec::new();
    for handle in handles {
        // Within the grace window every concurrent refresh succeeds.
        new_tokens.push(rotated(handle.await.unwrap()).refresh_token);
    }
    new_tokens.sort();
    new_tokens.dedup();
    assert_eq!(new_tokens.len(), TASKS);

    // The presented token was rotated exactly once: it points at one of them.
    let presented = row(&pool, &session.refresh_token).await;
    let mut successor_ids = Vec::new();
    for token in &new_tokens {
        successor_ids.push(row(&pool, token).await.id);
    }
    assert!(successor_ids.contains(&presented.replaced_by.unwrap()));
}

#[tokio::test]
async fn cleanup_deletes_refresh_tokens_expired_more_than_a_day_ago() {
    let pool = init_pg_pool().await;
    let (user_id, _) = create_user(&pool).await;
    let repo = refresh_token_repository(pool.clone());
    let family = Uuid::new_v4();

    let long_expired = format!("cleanup-{}", Uuid::new_v4());
    let recently_expired = format!("cleanup-{}", Uuid::new_v4());
    let active = format!("cleanup-{}", Uuid::new_v4());
    repo.create_token(
        user_id,
        family,
        sha256_hex(&long_expired),
        Utc::now() - Duration::days(2),
    )
    .await
    .unwrap();
    repo.create_token(
        user_id,
        family,
        sha256_hex(&recently_expired),
        Utc::now() - Duration::hours(1),
    )
    .await
    .unwrap();
    repo.create_token(
        user_id,
        family,
        sha256_hex(&active),
        Utc::now() + Duration::days(1),
    )
    .await
    .unwrap();

    assert!(repo.delete_expired().await.unwrap() >= 1);

    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT token_hash FROM refresh_tokens WHERE family_id = $1")
            .bind(family)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(!remaining.contains(&sha256_hex(&long_expired)));
    assert!(remaining.contains(&sha256_hex(&recently_expired)));
    assert!(remaining.contains(&sha256_hex(&active)));
}

#[actix_web::test]
async fn refresh_endpoint_follows_the_rest_contract() {
    use actix_web::{App, http::StatusCode, test, web};
    use feature_toggle_backend::logic::jwt_secret::JwtSecretLogic;

    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    secret(&pool).await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(auth))
            .app_data(web::Data::new(token_logic(&pool, auth)))
            .app_data(web::Data::new(
                jwt_secret_logic(pool.clone(), auth) as Box<dyn JwtSecretLogic>
            ))
            .service(
                web::scope("/api/v1").configure(feature_toggle_backend::rest::auth::configure),
            ),
    )
    .await;

    let post = |uri: &str, body: serde_json::Value| {
        test::TestRequest::post()
            .uri(uri)
            .set_json(body)
            .to_request()
    };

    let resp = test::call_service(
        &app,
        post(
            "/api/v1/auth/login",
            serde_json::json!({ "username": username, "password": PASSWORD }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let login: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(login["expiresIn"], 1800);
    assert_eq!(login["isTemporary"], false);
    let first = login["refreshToken"].as_str().unwrap().to_string();

    let resp = test::call_service(
        &app,
        post(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refreshToken": first }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = test::read_body_json(resp).await;
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(body["refreshToken"].as_str().is_some_and(|t| t != first));
    assert_eq!(body["expiresIn"], 1800);
    assert_eq!(body["user"]["username"], username.as_str());

    backdate_revocation(&pool, &first, 30).await;
    let resp = test::call_service(
        &app,
        post(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refreshToken": first }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["error"], "refresh_token_reused");

    let resp = test::call_service(
        &app,
        post(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refreshToken": "unknown" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = test::read_body_json(resp).await;
    assert_eq!(body["error"], "invalid_refresh_token");

    let resp = test::call_service(&app, post("/api/v1/auth/refresh", serde_json::json!({}))).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---- emergency deactivation (deactivate-all) vs session writes ----

async fn refresh_in(
    conn: &mut sqlx::PgConnection,
    pool: &PgPool,
    token: &str,
    auth: AuthConfig,
) -> RefreshOutcome {
    refresh_session_in_tx(
        conn,
        &user_repository_tx(pool.clone()),
        &role_repository_tx(pool.clone()),
        &auth,
        token,
    )
    .await
    .expect("refresh")
}

#[tokio::test]
async fn refresh_is_rejected_when_the_signing_secret_is_revoked() {
    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let session = login(&pool, &username, auth).await;

    // Rolled back, so the shared secret and other sessions stay untouched.
    let mut tx = pool.begin().await.unwrap();
    deactivate_all_secrets_in_tx(&mut tx)
        .await
        .expect("deactivate all");
    let outcome = refresh_in(&mut tx, &pool, &session.refresh_token, auth).await;
    assert_eq!(rejection(outcome), RefreshRejection::Invalid);
    tx.rollback().await.unwrap();

    // With the secret live again the same token rotates.
    rotated(refresh(&pool, &session.refresh_token, auth).await);
}

#[tokio::test]
async fn login_session_is_stored_only_under_an_active_unrevoked_secret() {
    let pool = init_pg_pool().await;
    let (user_id, _) = create_user(&pool).await;
    let active_kid = secret(&pool).await.kid;
    let new_session = |kid: Uuid| NewSession {
        user_id,
        kid,
        access_token_hash: format!("session-access-{}", Uuid::new_v4()),
        access_expires_at: Utc::now() + Duration::minutes(30),
        refresh_family_id: Uuid::new_v4(),
        refresh_token_hash: format!("session-refresh-{}", Uuid::new_v4()),
        refresh_expires_at: Utc::now() + Duration::days(7),
    };
    async fn stored_rows(conn: &mut sqlx::PgConnection, session: &NewSession) -> i64 {
        sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM jwt_tokens WHERE token_hash = $1)
                  + (SELECT COUNT(*) FROM refresh_tokens WHERE token_hash = $2)",
        )
        .bind(&session.access_token_hash)
        .bind(&session.refresh_token_hash)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    }

    let mut tx = pool.begin().await.unwrap();
    let live = new_session(active_kid);
    assert!(store_session_tx(&mut tx, &live).await.unwrap());
    assert_eq!(stored_rows(&mut tx, &live).await, 2);

    // Signed by a secret that has since been rotated out: not stored.
    create_secret_tx(&mut tx, format!("session-test-{}", Uuid::new_v4()), None)
        .await
        .expect("rotate secret");
    let rotated_out = new_session(active_kid);
    assert!(!store_session_tx(&mut tx, &rotated_out).await.unwrap());
    assert_eq!(stored_rows(&mut tx, &rotated_out).await, 0);
    tx.rollback().await.unwrap();

    // Signed by a secret revoked by deactivate-all: not stored.
    let mut tx = pool.begin().await.unwrap();
    deactivate_all_secrets_in_tx(&mut tx)
        .await
        .expect("deactivate all");
    let revoked = new_session(active_kid);
    assert!(!store_session_tx(&mut tx, &revoked).await.unwrap());
    assert_eq!(stored_rows(&mut tx, &revoked).await, 0);
    tx.rollback().await.unwrap();
}

/// deactivate-all started while a refresh transaction holds the signing-secret
/// lock waits for it, and then revokes the tokens that refresh created.
#[tokio::test]
async fn deactivate_all_waits_for_an_in_flight_refresh_and_revokes_its_tokens() {
    let pool = init_pg_pool().await;
    let (_, username) = create_user(&pool).await;
    let auth = AuthConfig::default();
    let session = login(&pool, &username, auth).await;

    let mut refresh_tx = pool.begin().await.unwrap();
    let new_session =
        rotated(refresh_in(&mut refresh_tx, &pool, &session.refresh_token, auth).await);
    let new_refresh_hash = sha256_hex(&new_session.refresh_token);
    let new_access_hash = sha256_hex(&new_session.token);

    let deactivate = {
        let pool = pool.clone();
        let (new_refresh_hash, new_access_hash) =
            (new_refresh_hash.clone(), new_access_hash.clone());
        tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            deactivate_all_secrets_in_tx(&mut tx)
                .await
                .expect("deactivate all");
            let refresh_revoked_at: Option<DateTime<Utc>> =
                sqlx::query_scalar("SELECT revoked_at FROM refresh_tokens WHERE token_hash = $1")
                    .bind(&new_refresh_hash)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            let access_revoked: bool =
                sqlx::query_scalar("SELECT is_revoked FROM jwt_tokens WHERE token_hash = $1")
                    .bind(&new_access_hash)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            // Rolled back, so the shared secret and other sessions stay untouched.
            tx.rollback().await.unwrap();
            (refresh_revoked_at, access_revoked)
        })
    };

    // deactivate-all blocks on the secret row the refresh holds FOR SHARE.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!deactivate.is_finished(), "deactivate-all did not wait");

    refresh_tx.commit().await.unwrap();
    let (refresh_revoked_at, access_revoked) = deactivate.await.unwrap();
    assert!(
        refresh_revoked_at.is_some(),
        "refresh token minted during deactivate-all survived"
    );
    assert!(access_revoked);
}
