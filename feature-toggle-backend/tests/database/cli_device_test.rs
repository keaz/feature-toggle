//! Device-code login for the CLI: authorize, poll, approve in the UI, and the
//! single session that the approval releases.

use actix_web::body::MessageBody;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::{App, HttpMessage, test, web};
use chrono::{Duration, Utc};
use feature_toggle_backend::JwtUser;
use feature_toggle_backend::config::AuthConfig;
use feature_toggle_backend::database::activity_log::{
    ActivityLogRepository, activity_log_repository,
};
use feature_toggle_backend::database::cli_device_authorization::{
    NewCliDeviceAuthorization, cli_device_authorization_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jwt_token::jwt_token_repository;
use feature_toggle_backend::database::refresh_token::refresh_token_repository;
use feature_toggle_backend::database::role::role_repository;
use feature_toggle_backend::database::user::{CreateUser, user_repository};
use feature_toggle_backend::logic::device_login::new_user_code;
use feature_toggle_backend::logic::jwt_secret::jwt_secret_logic;
use feature_toggle_backend::logic::jwt_token::{JwtTokenLogic, LoginResult, jwt_token_logic};
use feature_toggle_backend::logic::role::role_logic;
use feature_toggle_backend::logic::user::ApiUser;
use feature_toggle_backend::logic::user::user_logic;
use feature_toggle_backend::rest;
use feature_toggle_backend::rest::device_auth::DeviceAuthLimiter;
use feature_toggle_backend::rest::sso_auth::SsoLoginConfig;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const UI: &str = "http://ui.test";

async fn create_user(pool: &PgPool) -> JwtUser {
    let suffix = Uuid::new_v4();
    let username = format!("device-test-{suffix}");
    let created = user_repository(pool.clone())
        .create_user(CreateUser {
            username: username.clone(),
            password_hash: "not-used".into(),
            first_name: "De".into(),
            last_name: "Vice".into(),
            email: format!("device-test-{suffix}@example.com"),
            mobile_number: None,
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .expect("create user");
    JwtUser {
        id: created.id,
        username,
        is_admin: false,
        roles: vec![],
        team_id: None,
        token_hash: "h".into(),
    }
}

async fn build_app(
    pool: &PgPool,
    limiter: DeviceAuthLimiter,
    user: Option<JwtUser>,
) -> impl Service<
    actix_http::Request,
    Response = ServiceResponse<impl MessageBody>,
    Error = actix_web::Error,
> {
    build_app_with(pool, limiter, user, None).await
}

async fn build_app_with(
    pool: &PgPool,
    limiter: DeviceAuthLimiter,
    user: Option<JwtUser>,
    tokens_override: Option<Box<dyn JwtTokenLogic>>,
) -> impl Service<
    actix_http::Request,
    Response = ServiceResponse<impl MessageBody>,
    Error = actix_web::Error,
> {
    let auth = AuthConfig::default();
    let secret_logic = jwt_secret_logic(pool.clone(), auth);
    secret_logic.initialize_secret().await.expect("jwt secret");
    let activity: Box<dyn ActivityLogRepository> = activity_log_repository(pool.clone());
    let tokens: Box<dyn JwtTokenLogic> = jwt_token_logic(
        jwt_token_repository(pool.clone()),
        refresh_token_repository(pool.clone()),
        user_logic(user_repository(pool.clone()), activity.clone()),
        role_logic(role_repository(pool.clone()), activity.clone()),
        secret_logic,
        auth,
    );
    let tokens = tokens_override.unwrap_or(tokens);
    test::init_service(
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(activity))
            .app_data(web::Data::new(tokens))
            .app_data(web::Data::new(limiter))
            .app_data(web::Data::new(SsoLoginConfig {
                ui_origin: UI.to_string(),
                public_base_url: None,
            }))
            .wrap_fn(move |req, srv| {
                if let Some(user) = user.clone() {
                    req.extensions_mut().insert(user);
                }
                srv.call(req)
            })
            .service(web::scope("/api/v1").configure(rest::device_auth::configure)),
    )
    .await
}

fn limiter() -> DeviceAuthLimiter {
    DeviceAuthLimiter::new(600, 600)
}

async fn post<S, B>(app: &S, uri: &str, body: Value) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let req = test::TestRequest::post()
        .uri(uri)
        .set_json(body)
        .to_request();
    let resp = test::call_service(app, req).await;
    let status = resp.status();
    let body = test::read_body(resp).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn start<S, B>(app: &S) -> (String, String)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let (status, body) = post(app, "/api/v1/auth/device/authorize", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["deviceCode"].as_str().unwrap().to_string(),
        body["userCode"].as_str().unwrap().to_string(),
    )
}

#[actix_web::test]
async fn approval_releases_exactly_one_session() {
    let pool = init_pg_pool().await;
    let user = create_user(&pool).await;
    let cli = build_app(&pool, limiter(), None).await;
    let browser = build_app(&pool, limiter(), Some(user.clone())).await;

    let (status, body) = post(&cli, "/api/v1/auth/device/authorize", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let device_code = body["deviceCode"].as_str().unwrap().to_string();
    let user_code = body["userCode"].as_str().unwrap().to_string();
    assert_eq!(device_code.len(), 43);
    assert_eq!(user_code.len(), 9);
    assert_eq!(body["verificationUri"], format!("{UI}/device"));
    assert_eq!(
        body["verificationUriComplete"],
        format!("{UI}/device?code={user_code}")
    );
    assert_eq!(body["expiresIn"], 600);
    assert_eq!(body["interval"], 5);

    let token = json!({ "deviceCode": device_code });
    let (status, body) = post(&cli, "/api/v1/auth/device/token", token.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "authorization_pending");
    let (status, body) = post(&cli, "/api/v1/auth/device/token", token.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "slow_down");

    // People type the code in lower case and without the hyphen.
    let typed = user_code.replace('-', "").to_lowercase();
    let (status, body) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": typed, "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = post(&cli, "/api/v1/auth/device/token", token.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(body["refreshToken"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(body["user"]["id"], user.id.to_string());

    let (status, body) = post(&cli, "/api/v1/auth/device/token", token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "expired_token");

    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activity_log WHERE activity_type = 'cli_login_approved' AND entity_id = $1",
    )
    .bind(user.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[actix_web::test]
async fn denial_is_reported_to_the_cli() {
    let pool = init_pg_pool().await;
    let user = create_user(&pool).await;
    let cli = build_app(&pool, limiter(), None).await;
    let browser = build_app(&pool, limiter(), Some(user.clone())).await;
    let (device_code, user_code) = start(&cli).await;
    let (status, _) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": user_code, "approve": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post(
        &cli,
        "/api/v1/auth/device/token",
        json!({ "deviceCode": device_code }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "access_denied");
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM activity_log WHERE activity_type = 'cli_login_denied' AND entity_id = $1",
    )
    .bind(user.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[actix_web::test]
async fn approval_needs_a_pending_unexpired_code_and_a_person() {
    let pool = init_pg_pool().await;
    let user = create_user(&pool).await;
    let cli = build_app(&pool, limiter(), None).await;
    let browser = build_app(&pool, limiter(), Some(user.clone())).await;

    let (status, body) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": "BCDF-GHJK-X", "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": "ZZZZ-ZZZZ", "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (_, user_code) = start(&cli).await;
    let (status, _) = post(
        &cli,
        "/api/v1/auth/device/approve",
        json!({ "userCode": user_code, "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let system_client = JwtUser {
        team_id: Some(Uuid::new_v4()),
        ..user.clone()
    };
    let robot = build_app(&pool, limiter(), Some(system_client)).await;
    let (status, _) = post(
        &robot,
        "/api/v1/auth/device/approve",
        json!({ "userCode": user_code, "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": user_code, "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": user_code, "approve": false }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a decided code cannot be changed"
    );

    let expired_code = new_user_code();
    cli_device_authorization_repository(pool.clone())
        .create(NewCliDeviceAuthorization {
            device_code_hash: format!("expired-{}", Uuid::new_v4()),
            user_code: expired_code.clone(),
            interval_secs: 5,
            expires_at: Utc::now() - Duration::seconds(1),
        })
        .await
        .expect("create expired");
    let (status, _) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": expired_code, "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[actix_web::test]
async fn unknown_device_codes_are_expired() {
    let pool = init_pg_pool().await;
    let cli = build_app(&pool, limiter(), None).await;
    let (status, body) = post(
        &cli,
        "/api/v1/auth/device/token",
        json!({ "deviceCode": "nope" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "expired_token");
}

#[actix_web::test]
async fn public_device_routes_are_rate_limited() {
    let pool = init_pg_pool().await;
    let cli = build_app(&pool, DeviceAuthLimiter::new(1, 1), None).await;
    let peer = "10.1.1.1:4000";
    let first = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), peer),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let resp = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), peer),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(resp.headers().contains_key("retry-after"));
    let token = json!({ "deviceCode": "x" });
    let resp = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/token", token.clone(), peer),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = test::call_service(&cli, post_from("/api/v1/auth/device/token", token, peer)).await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[actix_web::test]
async fn repository_consumes_once_and_cleans_up_expired_rows() {
    let pool = init_pg_pool().await;
    let repo = cli_device_authorization_repository(pool.clone());
    let user = create_user(&pool).await;
    let user_code = new_user_code();
    let row = repo
        .create(NewCliDeviceAuthorization {
            device_code_hash: format!("repo-{}", Uuid::new_v4()),
            user_code: user_code.clone(),
            interval_secs: 5,
            expires_at: Utc::now() + Duration::minutes(10),
        })
        .await
        .expect("create");
    assert_eq!(row.status, "pending");
    assert!(
        repo.find_by_device_code_hash(&row.device_code_hash)
            .await
            .unwrap()
            .is_some()
    );
    let decided = repo
        .decide(&user_code, user.id, true)
        .await
        .unwrap()
        .expect("decided");
    assert_eq!(decided.status, "approved");
    assert_eq!(decided.user_id, Some(user.id));
    assert!(repo.consume_approved(row.id).await.unwrap().is_some());
    assert!(repo.consume_approved(row.id).await.unwrap().is_none());

    let old = repo
        .create(NewCliDeviceAuthorization {
            device_code_hash: format!("old-{}", Uuid::new_v4()),
            user_code: new_user_code(),
            interval_secs: 5,
            expires_at: Utc::now() - Duration::minutes(1),
        })
        .await
        .expect("create expired");
    assert!(repo.delete_expired().await.unwrap() >= 1);
    assert!(
        repo.find_by_device_code_hash(&old.device_code_hash)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("DELETE FROM cli_device_authorizations WHERE id = $1")
        .bind(row.id)
        .execute(&pool)
        .await
        .unwrap();
}

/// Token logic whose session issue always fails, to test what a poll leaves
/// behind when issuing the session goes wrong.
#[derive(Clone)]
struct FailingTokens;

#[async_trait::async_trait]
impl JwtTokenLogic for FailingTokens {
    async fn login_user(
        &self,
        _: String,
        _: String,
    ) -> Result<LoginResult, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn authenticate(
        &self,
        _: String,
        _: String,
    ) -> Result<ApiUser, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn issue_session(
        &self,
        _: ApiUser,
    ) -> Result<LoginResult, feature_toggle_backend::Error> {
        Err(feature_toggle_backend::Error::DatabaseError(
            sqlx::Error::PoolTimedOut,
        ))
    }
    async fn logout_user(&self, _: Uuid) -> Result<u64, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn revoke_refresh_token_family(
        &self,
        _: Uuid,
        _: &str,
    ) -> Result<u64, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn store_token(
        &self,
        _: Uuid,
        _: String,
        _: chrono::DateTime<Utc>,
    ) -> Result<feature_toggle_backend::database::jwt_token::JwtToken, feature_toggle_backend::Error>
    {
        unimplemented!()
    }
    async fn is_token_valid(&self, _: &str) -> Result<bool, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn revoke_token(&self, _: &str) -> Result<bool, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn revoke_all_user_tokens(&self, _: Uuid) -> Result<u64, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn cleanup_expired_tokens(&self) -> Result<u64, feature_toggle_backend::Error> {
        unimplemented!()
    }
    async fn get_user_active_tokens(
        &self,
        _: Uuid,
    ) -> Result<
        Vec<feature_toggle_backend::database::jwt_token::JwtToken>,
        feature_toggle_backend::Error,
    > {
        unimplemented!()
    }
    fn clone_box(&self) -> Box<dyn JwtTokenLogic> {
        Box::new(self.clone())
    }
}

#[actix_web::test]
async fn a_failed_session_issue_keeps_the_approval() {
    let pool = init_pg_pool().await;
    let user = create_user(&pool).await;
    let broken = build_app_with(&pool, limiter(), None, Some(Box::new(FailingTokens))).await;
    let cli = build_app(&pool, limiter(), None).await;
    let browser = build_app(&pool, limiter(), Some(user.clone())).await;
    let (device_code, user_code) = start(&cli).await;
    let (status, _) = post(
        &browser,
        "/api/v1/auth/device/approve",
        json!({ "userCode": user_code, "approve": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let token = json!({ "deviceCode": device_code });
    let (status, _) = post(&broken, "/api/v1/auth/device/token", token.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    // The person does not have to approve again: the next poll gets the session.
    let (status, body) = post(&cli, "/api/v1/auth/device/token", token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["id"], user.id.to_string());
}

fn post_from(uri: &str, body: Value, peer: &str) -> actix_http::Request {
    test::TestRequest::post()
        .uri(uri)
        .peer_addr(peer.parse().unwrap())
        .set_json(body)
        .to_request()
}

#[actix_web::test]
async fn one_noisy_client_does_not_lock_out_others() {
    let pool = init_pg_pool().await;
    // One authorize per client per minute, ten in total.
    let cli = build_app(
        &pool,
        DeviceAuthLimiter::new(1, 1).with_totals(10, 10),
        None,
    )
    .await;
    let first = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), "10.0.0.1:5000"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let again = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), "10.0.0.1:5001"),
    )
    .await;
    assert_eq!(again.status(), StatusCode::TOO_MANY_REQUESTS);
    let other = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), "10.0.0.2:5000"),
    )
    .await;
    assert_eq!(
        other.status(),
        StatusCode::OK,
        "another client still gets in"
    );
}

#[actix_web::test]
async fn the_total_limit_still_applies_across_clients() {
    let pool = init_pg_pool().await;
    let cli = build_app(
        &pool,
        DeviceAuthLimiter::new(10, 10).with_totals(1, 1),
        None,
    )
    .await;
    let first = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), "10.0.0.1:5000"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let other = test::call_service(
        &cli,
        post_from("/api/v1/auth/device/authorize", json!({}), "10.0.0.2:5000"),
    )
    .await;
    assert_eq!(other.status(), StatusCode::TOO_MANY_REQUESTS);
}
