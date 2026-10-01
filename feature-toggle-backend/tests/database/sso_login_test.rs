//! SSO login flow end to end against an in-process mock OpenID provider:
//! authorize, callback (token exchange, id_token validation, user resolution) and
//! the one-time code exchange, plus enforce-SSO for password login and the removal
//! of group mappings when their role or team is deleted.
//!
//! The mock IdP signs RS256 id_tokens with test-only keys committed under
//! `tests/fixtures/oidc/` (they protect nothing).

use actix_web::body::MessageBody;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::StatusCode;
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, test, web};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use feature_toggle_backend::config::AuthConfig;
use feature_toggle_backend::database::activity_log::{
    ActivityLogRepository, activity_log_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jwt_token::jwt_token_repository;
use feature_toggle_backend::database::refresh_token::refresh_token_repository;
use feature_toggle_backend::database::role::role_repository;
use feature_toggle_backend::database::team::team_repository;
use feature_toggle_backend::database::user::user_repository;
use feature_toggle_backend::logic::jwt_secret::jwt_secret_logic;
use feature_toggle_backend::logic::jwt_token::{JwtTokenLogic, jwt_token_logic};
use feature_toggle_backend::logic::oidc_client::OidcClient;
use feature_toggle_backend::logic::role::role_logic;
use feature_toggle_backend::logic::secret_box::SecretBox;
use feature_toggle_backend::logic::sso_provider::SsoSecrets;
use feature_toggle_backend::logic::user::user_logic;
use feature_toggle_backend::rest;
use feature_toggle_backend::rest::sso_auth::{STATE_COOKIE, SsoLoginConfig};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use serial_test::serial;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const CLIENT_ID: &str = "fluxgate-client";
/// Contains characters that HTTP Basic client authentication must form-encode.
const CLIENT_SECRET: &str = "client secret:+/%value";
const UI: &str = "http://ui.test";
const BACKEND: &str = "http://backend.test";
const KID: &str = "test-key-1";
const PASSWORD: &str = "sso login test password";

const IDP_PRIVATE_KEY: &[u8] = include_bytes!("../fixtures/oidc/idp_rsa_private.pem");
const OTHER_PRIVATE_KEY: &[u8] = include_bytes!("../fixtures/oidc/other_rsa_private.pem");
const IDP_JWKS: &str = include_str!("../fixtures/oidc/idp_jwks.json");
const OTHER_JWK: &str = include_str!("../fixtures/oidc/other_jwk.json");

// ------------------------------------------------------------------ mock IdP

struct Grant {
    challenge: String,
    redirect_uri: String,
    id_token: String,
}

struct TokenCall {
    basic: Option<(String, String)>,
    form: HashMap<String, String>,
}

struct IdpState {
    issuer: String,
    jwks: Value,
    auth_methods: Vec<String>,
    userinfo_email: Option<String>,
    userinfo_groups: Option<Value>,
    /// Subject the userinfo endpoint reports (set by the test using it).
    userinfo_sub: String,
    grants: HashMap<String, Grant>,
    token_calls: Vec<TokenCall>,
    jwks_fetches: usize,
    discovery_fetches: usize,
}

struct MockIdp {
    issuer: String,
    state: Arc<Mutex<IdpState>>,
}

impl MockIdp {
    fn grant(&self, code: &str, challenge: &str, redirect_uri: &str, id_token: String) {
        self.state.lock().unwrap().grants.insert(
            code.to_string(),
            Grant {
                challenge: challenge.to_string(),
                redirect_uri: redirect_uri.to_string(),
                id_token,
            },
        );
    }
}

fn form_decode(value: &str) -> String {
    serde_urlencoded::from_str::<Vec<(String, String)>>(&format!("v={value}"))
        .ok()
        .and_then(|mut pairs| pairs.pop())
        .map(|(_, v)| v)
        .unwrap_or_default()
}

async fn idp_discovery(state: web::Data<Mutex<IdpState>>) -> HttpResponse {
    let mut state = state.lock().unwrap();
    state.discovery_fetches += 1;
    let base = &state.issuer;
    HttpResponse::Ok().json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "userinfo_endpoint": format!("{base}/userinfo"),
        "jwks_uri": format!("{base}/jwks"),
        "token_endpoint_auth_methods_supported": state.auth_methods,
        "id_token_signing_alg_values_supported": ["RS256", "HS256", "none"],
    }))
}

async fn idp_jwks(state: web::Data<Mutex<IdpState>>) -> HttpResponse {
    let mut state = state.lock().unwrap();
    state.jwks_fetches += 1;
    HttpResponse::Ok().json(state.jwks.clone())
}

async fn idp_token(
    req: HttpRequest,
    state: web::Data<Mutex<IdpState>>,
    form: web::Form<HashMap<String, String>>,
) -> HttpResponse {
    let form = form.into_inner();
    let basic = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| {
            v.split_once(':')
                .map(|(id, secret)| (form_decode(id), form_decode(secret)))
        });
    let mut state = state.lock().unwrap();
    state.token_calls.push(TokenCall {
        basic: basic.clone(),
        form: form.clone(),
    });
    let invalid = |error: &str| HttpResponse::BadRequest().json(json!({ "error": error }));

    let (client_id, client_secret) = match basic {
        Some(pair) => pair,
        None => (
            form.get("client_id").cloned().unwrap_or_default(),
            form.get("client_secret").cloned().unwrap_or_default(),
        ),
    };
    if client_id != CLIENT_ID || client_secret != CLIENT_SECRET {
        return invalid("invalid_client");
    }
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return invalid("unsupported_grant_type");
    }
    let Some(grant) = form.get("code").and_then(|code| state.grants.remove(code)) else {
        return invalid("invalid_grant");
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != grant.challenge {
        return invalid("invalid_grant");
    }
    if form.get("redirect_uri") != Some(&grant.redirect_uri) {
        return invalid("invalid_grant");
    }
    HttpResponse::Ok().json(json!({
        "access_token": "mock-access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": grant.id_token,
    }))
}

async fn idp_userinfo(req: HttpRequest, state: web::Data<Mutex<IdpState>>) -> HttpResponse {
    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        == Some("Bearer mock-access-token");
    if !authorized {
        return HttpResponse::Unauthorized().finish();
    }
    let state = state.lock().unwrap();
    let sub = state.userinfo_sub.clone();
    HttpResponse::Ok().json(json!({
        "sub": sub,
        "email": state.userinfo_email,
        "email_verified": true,
        "groups": state.userinfo_groups,
    }))
}

async fn start_idp() -> MockIdp {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let state = Arc::new(Mutex::new(IdpState {
        issuer: issuer.clone(),
        jwks: serde_json::from_str(IDP_JWKS).unwrap(),
        auth_methods: vec![
            "client_secret_basic".to_string(),
            "client_secret_post".to_string(),
        ],
        userinfo_email: None,
        userinfo_groups: None,
        userinfo_sub: String::new(),
        grants: HashMap::new(),
        token_calls: Vec::new(),
        jwks_fetches: 0,
        discovery_fetches: 0,
    }));
    let data = web::Data::from(state.clone());
    let server = HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .route(
                "/.well-known/openid-configuration",
                web::get().to(idp_discovery),
            )
            .route("/jwks", web::get().to(idp_jwks))
            .route("/token", web::post().to(idp_token))
            .route("/userinfo", web::get().to(idp_userinfo))
    })
    .workers(1)
    .listen(listener)
    .unwrap()
    .run();
    actix_web::rt::spawn(server);
    MockIdp { issuer, state }
}

// ------------------------------------------------------------------ id_tokens

fn sign_with(claims: &Value, key_pem: &[u8], kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(key_pem).unwrap(),
    )
    .unwrap()
}

fn sign(claims: &Value) -> String {
    sign_with(claims, IDP_PRIVATE_KEY, KID)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn claims(idp: &MockIdp, nonce: &str, sub: &str, email: &str) -> Value {
    json!({
        "iss": idp.issuer,
        "aud": CLIENT_ID,
        "sub": sub,
        "email": email,
        "email_verified": true,
        "preferred_username": format!("pu-{sub}"),
        "given_name": "Grace",
        "family_name": "Hopper",
        "exp": now() + 300,
        "iat": now(),
        "nonce": nonce,
    })
}

// ------------------------------------------------------------------ app

fn secrets() -> SsoSecrets {
    SsoSecrets::with_box(SecretBox::from_base64_key(&STANDARD.encode([9u8; 32])).unwrap())
}

async fn build_app(
    pool: &PgPool,
) -> impl Service<
    actix_http::Request,
    Response = ServiceResponse<impl MessageBody>,
    Error = actix_web::Error,
> {
    build_app_with(pool, OidcClient::new().unwrap()).await
}

async fn build_app_with(
    pool: &PgPool,
    oidc: OidcClient,
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
    test::init_service(
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(activity))
            .app_data(web::Data::new(tokens))
            .app_data(web::Data::new(secrets()))
            .app_data(web::Data::new(oidc))
            .app_data(web::Data::new(SsoLoginConfig {
                ui_origin: UI.to_string(),
                public_base_url: Some(BACKEND.to_string()),
            }))
            .service(
                web::scope("/api/v1")
                    .configure(rest::auth::configure)
                    .configure(rest::sso::configure)
                    .configure(rest::sso_auth::configure),
            ),
    )
    .await
}

struct ProviderOpts {
    jit: bool,
    linking: bool,
    domains: Vec<String>,
    enabled: bool,
}

impl Default for ProviderOpts {
    fn default() -> Self {
        Self {
            jit: true,
            linking: false,
            domains: vec![],
            enabled: true,
        }
    }
}

struct TestProvider {
    id: Uuid,
    slug: String,
}

async fn create_provider(pool: &PgPool, idp: &MockIdp, opts: ProviderOpts) -> TestProvider {
    let id = Uuid::new_v4();
    let slug = format!("idp-{}", &Uuid::new_v4().simple().to_string()[..10]);
    let sealed = secrets().seal(id, CLIENT_SECRET).unwrap();
    sqlx::query(
        "INSERT INTO sso_providers (id, slug, display_name, issuer_url, client_id, client_secret_enc,
                                    allowed_email_domains, jit_provisioning, allow_email_linking, enabled)
         VALUES ($1, $2, 'Mock IdP', $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(&slug)
    .bind(&idp.issuer)
    .bind(CLIENT_ID)
    .bind(sealed)
    .bind(&opts.domains)
    .bind(opts.jit)
    .bind(opts.linking)
    .bind(opts.enabled)
    .execute(pool)
    .await
    .unwrap();
    TestProvider { id, slug }
}

/// Deletes the providers of this mock IdP (their identities, states and codes
/// cascade); provisioned users stay, as other tests' users do.
async fn cleanup(pool: &PgPool, idp: &MockIdp) {
    sqlx::query("DELETE FROM sso_providers WHERE issuer_url = $1")
        .bind(&idp.issuer)
        .execute(pool)
        .await
        .unwrap();
}

struct AuthRequest {
    state: String,
    /// Value of the `fluxgate_sso_state` cookie set by authorize.
    cookie: String,
    nonce: String,
    challenge: String,
    redirect_uri: String,
    params: HashMap<String, String>,
}

async fn get_location<S, B>(app: &S, uri: &str) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    get_redirect(app, uri, None).await.0
}

/// GETs `uri` (optionally with the state cookie) and returns the redirect target
/// and the `fluxgate_sso_state` cookie the response sets, if any.
async fn get_redirect<S, B>(
    app: &S,
    uri: &str,
    state_cookie: Option<&str>,
) -> (String, Option<actix_web::cookie::Cookie<'static>>)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let mut req = test::TestRequest::get().uri(uri);
    if let Some(value) = state_cookie {
        req = req.cookie(actix_web::cookie::Cookie::new(
            STATE_COOKIE,
            value.to_string(),
        ));
    }
    let resp = test::call_service(app, req.to_request()).await;
    let cookie = resp
        .response()
        .cookies()
        .find(|c| c.name() == STATE_COOKIE)
        .map(|c| c.into_owned());
    assert_eq!(resp.status(), StatusCode::FOUND, "{uri}");
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap(),
        "no-store"
    );
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    (location, cookie)
}

/// The cookie value a browser holds for `state`: base64url(SHA-256(state)).
fn cookie_for(state: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(state.as_bytes()))
}

fn query_of(location: &str) -> HashMap<String, String> {
    reqwest::Url::parse(location)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn authorize<S, B>(app: &S, idp: &MockIdp, slug: &str, redirect: Option<&str>) -> AuthRequest
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let mut uri = format!("/api/v1/auth/sso/{slug}/authorize");
    if let Some(redirect) = redirect {
        uri.push_str(&format!(
            "?{}",
            serde_urlencoded::to_string([("redirect", redirect)]).unwrap()
        ));
    }
    let (location, cookie) = get_redirect(app, &uri, None).await;
    assert!(
        location.starts_with(&format!("{}/authorize?", idp.issuer)),
        "{location}"
    );
    let params = query_of(&location);
    let cookie = cookie.expect("authorize sets the state cookie");
    assert_eq!(cookie.value(), cookie_for(&params["state"]));
    assert_eq!(cookie.http_only(), Some(true));
    assert_eq!(cookie.same_site(), Some(actix_web::cookie::SameSite::Lax));
    assert_ne!(cookie.secure(), Some(true), "http backend");
    assert_eq!(cookie.path(), Some("/api/v1/auth/sso/"));
    assert_eq!(
        cookie.max_age(),
        Some(actix_web::cookie::time::Duration::seconds(600))
    );
    AuthRequest {
        cookie: cookie.value().to_string(),
        state: params["state"].clone(),
        nonce: params["nonce"].clone(),
        challenge: params["code_challenge"].clone(),
        redirect_uri: params["redirect_uri"].clone(),
        params,
    }
}

async fn callback<S, B>(
    app: &S,
    slug: &str,
    code: &str,
    state: &str,
    cookie: Option<&str>,
) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let query = serde_urlencoded::to_string([("code", code), ("state", state)]).unwrap();
    callback_query(app, slug, &query, cookie).await
}

/// Calls the callback with a raw query and checks that the response clears the
/// state cookie, whatever the outcome.
async fn callback_query<S, B>(app: &S, slug: &str, query: &str, cookie: Option<&str>) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let (location, set) = get_redirect(
        app,
        &format!("/api/v1/auth/sso/{slug}/callback?{query}"),
        cookie,
    )
    .await;
    let set = set.expect("callback clears the state cookie");
    assert_eq!(set.value(), "");
    assert_eq!(set.max_age(), Some(actix_web::cookie::time::Duration::ZERO));
    assert_eq!(set.path(), Some("/api/v1/auth/sso/"));
    location
}

/// authorize -> IdP grant with the id_token built by `make_token` -> callback.
async fn login_with<S, B>(
    app: &S,
    idp: &MockIdp,
    slug: &str,
    redirect: Option<&str>,
    make_token: impl FnOnce(&AuthRequest) -> String,
) -> String
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let auth = authorize(app, idp, slug, redirect).await;
    let code = format!("code-{}", Uuid::new_v4());
    idp.grant(
        &code,
        &auth.challenge,
        &auth.redirect_uri,
        make_token(&auth),
    );
    callback(app, slug, &code, &auth.state, Some(&auth.cookie)).await
}

fn sso_error(location: &str) -> Option<String> {
    if !location.starts_with(&format!("{UI}/login?")) {
        return None;
    }
    query_of(location).get("ssoError").cloned()
}

fn one_time_code(location: &str) -> String {
    assert!(
        location.starts_with(&format!("{UI}/auth/sso/complete?")),
        "{location}"
    );
    query_of(location)["code"].clone()
}

async fn exchange<S, B>(app: &S, code: &str) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let req = test::TestRequest::post()
        .uri("/api/v1/auth/sso/exchange")
        .set_json(json!({ "code": code }))
        .to_request();
    let resp = test::call_service(app, req).await;
    let status = resp.status();
    let body = test::read_body(resp).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn password_login<S, B>(app: &S, username: &str, password: &str) -> (StatusCode, Value)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let req = test::TestRequest::post()
        .uri("/api/v1/auth/login")
        .set_json(json!({ "username": username, "password": password }))
        .to_request();
    let resp = test::call_service(app, req).await;
    let status = resp.status();
    let body = test::read_body(resp).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

fn hash_password(password: &str) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

/// Inserts a user; `password: None` makes an SSO-only (passwordless) account.
async fn insert_user(
    pool: &PgPool,
    email: &str,
    password: Option<&str>,
    is_admin: bool,
    auth_source: &str,
) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let username = unique("sso-login-user");
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin, auth_source)
         VALUES ($1, $2, $3, 'Local', 'User', $4, $5, $6)",
    )
    .bind(id)
    .bind(&username)
    .bind(password.map(hash_password))
    .bind(email)
    .bind(is_admin)
    .bind(auth_source)
    .execute(pool)
    .await
    .unwrap();
    (id, username)
}

async fn identity_user(pool: &PgPool, provider: &TestProvider, sub: &str) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT user_id FROM user_identities WHERE provider_id = $1 AND subject = $2",
    )
    .bind(provider.id)
    .bind(sub)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// Activity types logged for the user, sorted (entries of one transaction share
/// a timestamp).
async fn activity_types_for(pool: &PgPool, user_id: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT activity_type FROM activity_log WHERE entity_type = 'user' AND entity_id = $1
         ORDER BY activity_type",
    )
    .bind(user_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

// ------------------------------------------------------------------ tests

#[actix_web::test]
async fn jit_login_provisions_user_and_exchange_issues_one_session() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let email = format!("{}@Example.com", unique("jit"));

    let auth = authorize(&app, &idp, &provider.slug, Some("/features?env=prod")).await;
    let p = &auth.params;
    assert_eq!(p["response_type"], "code");
    assert_eq!(p["client_id"], CLIENT_ID);
    assert_eq!(
        p["redirect_uri"],
        format!("{BACKEND}/api/v1/auth/sso/{}/callback", provider.slug)
    );
    assert_eq!(p["scope"], "openid email profile");
    assert_eq!(p["code_challenge_method"], "S256");
    for value in [&auth.state, &auth.nonce, &auth.challenge] {
        assert_eq!(URL_SAFE_NO_PAD.decode(value).unwrap().len(), 32);
    }
    assert_ne!(auth.state, auth.nonce);

    let code = "jit-code";
    idp.grant(
        code,
        &auth.challenge,
        &auth.redirect_uri,
        sign(&claims(&idp, &auth.nonce, &sub, &email)),
    );
    let location = callback(&app, &provider.slug, code, &auth.state, Some(&auth.cookie)).await;
    let params = query_of(&location);
    assert_eq!(params["redirect"], "/features?env=prod");
    let one_time = one_time_code(&location);
    assert_eq!(URL_SAFE_NO_PAD.decode(&one_time).unwrap().len(), 32);

    // Client authentication used HTTP Basic with the decrypted, form-encoded secret.
    {
        let state = idp.state.lock().unwrap();
        let call = state.token_calls.last().unwrap();
        assert_eq!(
            call.basic,
            Some((CLIENT_ID.to_string(), CLIENT_SECRET.to_string()))
        );
        assert!(!call.form.contains_key("client_secret"));
    }

    // Only the hash of the one-time code is stored.
    let stored: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sso_login_codes WHERE code_hash = $1")
            .bind(&one_time)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, 0);

    let user_id = identity_user(&pool, &provider, &sub)
        .await
        .expect("identity");
    let row: (
        String,
        Option<String>,
        String,
        String,
        String,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT username, password_hash, first_name, last_name, auth_source, is_admin,
                is_temporary_password, last_login IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, format!("pu-{sub}"));
    assert_eq!(row.1, None);
    assert_eq!((row.2.as_str(), row.3.as_str()), ("Grace", "Hopper"));
    assert_eq!(row.4, "sso");
    // last_login waits for the exchange (see last_login_is_set_only_when_...).
    assert!(!row.5 && !row.6 && !row.7);
    assert_eq!(
        activity_types_for(&pool, user_id).await,
        vec!["sso_login", "sso_user_provisioned"]
    );

    let (status, body) = exchange(&app, &one_time).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["token"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(body["refreshToken"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(body["expiresIn"], 1800);
    assert_eq!(body["isTemporary"], false);
    assert_eq!(body["user"]["id"], user_id.to_string());
    assert_eq!(body["user"]["authSource"], "sso");
    assert_eq!(body["user"]["identities"][0]["providerSlug"], provider.slug);
    assert_eq!(body["user"]["identities"][0]["subject"], sub);

    // Single use: the code and the state cannot be replayed.
    let (status, body) = exchange(&app, &one_time).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid_sso_code");
    let replay = callback(&app, &provider.slug, code, &auth.state, Some(&auth.cookie)).await;
    assert_eq!(sso_error(&replay).as_deref(), Some("sso_state_invalid"));
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn existing_identity_logs_in_the_same_user_and_refreshes_email() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let first_email = format!("{}@example.com", unique("first"));
    let second_email = format!("{}@example.com", unique("second"));

    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &first_email))
    })
    .await;
    one_time_code(&location);
    let user_id = identity_user(&pool, &provider, &sub).await.unwrap();

    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &second_email))
    })
    .await;
    let code = one_time_code(&location);
    assert_eq!(identity_user(&pool, &provider, &sub).await, Some(user_id));
    let identity_email: Option<String> = sqlx::query_scalar(
        "SELECT email FROM user_identities WHERE provider_id = $1 AND subject = $2",
    )
    .bind(provider.id)
    .bind(&sub)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identity_email.as_deref(), Some(second_email.as_str()));
    let users_with_email: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = $1")
        .bind(&second_email)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users_with_email, 0, "no second account is provisioned");
    assert_eq!(
        activity_types_for(&pool, user_id).await,
        vec!["sso_login", "sso_login", "sso_user_provisioned"]
    );
    let (status, body) = exchange(&app, &code).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["id"], user_id.to_string());
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn email_linking_requires_setting_verified_email_and_a_human_account() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let linking = create_provider(
        &pool,
        &idp,
        ProviderOpts {
            linking: true,
            ..Default::default()
        },
    )
    .await;
    let no_linking = create_provider(&pool, &idp, ProviderOpts::default()).await;

    // Allowed: linking on, verified email, local user (email compared case-insensitively).
    let email = format!("{}@example.com", unique("link"));
    let (local_id, local_name) = insert_user(&pool, &email, Some(PASSWORD), false, "local").await;
    let sub = unique("sub");
    let upper = email.to_uppercase();
    let location = login_with(&app, &idp, &linking.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &upper))
    })
    .await;
    let code = one_time_code(&location);
    assert_eq!(identity_user(&pool, &linking, &sub).await, Some(local_id));
    assert_eq!(
        activity_types_for(&pool, local_id).await,
        vec!["sso_identity_linked", "sso_login"]
    );
    let (status, body) = exchange(&app, &code).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["username"], local_name);
    assert_eq!(body["user"]["authSource"], "local");
    // The linked account keeps its password.
    let (status, _) = password_login(&app, &local_name, PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    // Blocked: provider does not allow linking.
    let email = format!("{}@example.com", unique("nolink"));
    insert_user(&pool, &email, Some(PASSWORD), false, "local").await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &no_linking.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_linking_not_allowed")
    );
    assert_eq!(identity_user(&pool, &no_linking, &sub).await, None);

    // Blocked: email not verified.
    let email = format!("{}@example.com", unique("unverified"));
    insert_user(&pool, &email, Some(PASSWORD), false, "local").await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &linking.slug, None, |a| {
        let mut c = claims(&idp, &a.nonce, &sub, &email);
        c["email_verified"] = json!(false);
        sign(&c)
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_linking_not_allowed")
    );
    assert_eq!(identity_user(&pool, &linking, &sub).await, None);

    // Blocked: system-client shadow user.
    let email = format!("{}@example.com", unique("shadow"));
    insert_user(&pool, &email, None, false, "system").await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &linking.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_linking_not_allowed")
    );
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn domain_jit_and_disabled_rules_reject_login() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;

    let restricted = create_provider(
        &pool,
        &idp,
        ProviderOpts {
            domains: vec!["allowed.example".to_string()],
            ..Default::default()
        },
    )
    .await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &restricted.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, "x@other.example"))
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_email_domain_not_allowed")
    );
    let email = format!("{}@ALLOWED.example", unique("ok"));
    let location = login_with(&app, &idp, &restricted.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    one_time_code(&location);

    let no_jit = create_provider(
        &pool,
        &idp,
        ProviderOpts {
            jit: false,
            ..Default::default()
        },
    )
    .await;
    let email = format!("{}@example.com", unique("nojit"));
    let location = login_with(&app, &idp, &no_jit.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &unique("sub"), &email))
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_user_not_provisioned")
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    // Disabled user: rejected at callback, and a code issued before disabling is
    // rejected at exchange.
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let email = format!("{}@example.com", unique("disabled"));
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    let pending_code = one_time_code(&location);
    let user_id = identity_user(&pool, &provider, &sub).await.unwrap();
    sqlx::query("UPDATE users SET enabled = FALSE WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = exchange(&app, &pending_code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid_sso_code");
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_account_disabled")
    );
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn invalid_id_tokens_are_rejected() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;

    type Mutator = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Mutator, bool)> = vec![
        ("bad signature", Box::new(|_| {}), true),
        (
            "wrong aud",
            Box::new(|c| c["aud"] = json!("someone-else")),
            false,
        ),
        (
            "wrong iss",
            Box::new(|c| c["iss"] = json!("https://evil.example")),
            false,
        ),
        (
            "expired",
            Box::new(|c| c["exp"] = json!(now() - 120)),
            false,
        ),
        (
            "future iat",
            Box::new(|c| c["iat"] = json!(now() + 600)),
            false,
        ),
        (
            "wrong nonce",
            Box::new(|c| c["nonce"] = json!("not-the-nonce")),
            false,
        ),
        (
            "no nonce",
            Box::new(|c| {
                c.as_object_mut().unwrap().remove("nonce");
            }),
            false,
        ),
        (
            "no aud",
            Box::new(|c| {
                c.as_object_mut().unwrap().remove("aud");
            }),
            false,
        ),
    ];
    for (name, mutate, other_key) in cases {
        let sub = unique("sub");
        let location = login_with(&app, &idp, &provider.slug, None, |a| {
            let mut c = claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com"));
            mutate(&mut c);
            if other_key {
                // Same kid as the real key, signed by a different key.
                sign_with(&c, OTHER_PRIVATE_KEY, KID)
            } else {
                sign(&c)
            }
        })
        .await;
        assert_eq!(
            sso_error(&location).as_deref(),
            Some("sso_token_invalid"),
            "{name}: {location}"
        );
        assert_eq!(identity_user(&pool, &provider, &sub).await, None, "{name}");
    }

    // An HS256 token "signed" with the public key material, and an unsigned one.
    for alg_token in [
        {
            let mut header = Header::new(Algorithm::HS256);
            header.kid = Some(KID.to_string());
            Box::new(move |c: &Value| {
                encode(&header, c, &EncodingKey::from_secret(IDP_JWKS.as_bytes())).unwrap()
            }) as Box<dyn Fn(&Value) -> String>
        },
        Box::new(|c: &Value| {
            let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
            let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(c).unwrap());
            format!("{header}.{payload}.")
        }),
    ] {
        let sub = unique("sub");
        let location = login_with(&app, &idp, &provider.slug, None, |a| {
            alg_token(&claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com")))
        })
        .await;
        assert_eq!(sso_error(&location).as_deref(), Some("sso_token_invalid"));
    }
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn unknown_kid_refetches_jwks_once() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;

    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(
            &idp,
            &a.nonce,
            &unique("sub"),
            &format!("{}@example.com", unique("k")),
        ))
    })
    .await;
    one_time_code(&location);
    assert_eq!(
        idp.state.lock().unwrap().jwks_fetches,
        1,
        "cached after first use"
    );

    // The IdP rotates in a new key; a token with the new kid triggers one refetch.
    {
        let mut state = idp.state.lock().unwrap();
        let other: Value = serde_json::from_str(OTHER_JWK).unwrap();
        state.jwks["keys"].as_array_mut().unwrap().push(other);
    }
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign_with(
            &claims(
                &idp,
                &a.nonce,
                &unique("sub"),
                &format!("{}@example.com", unique("k")),
            ),
            OTHER_PRIVATE_KEY,
            "other-key",
        )
    })
    .await;
    one_time_code(&location);
    assert_eq!(idp.state.lock().unwrap().jwks_fetches, 2);

    // A kid that does not exist even after the refetch is rejected.
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign_with(
            &claims(
                &idp,
                &a.nonce,
                &unique("sub"),
                &format!("{}@example.com", unique("k")),
            ),
            OTHER_PRIVATE_KEY,
            "missing-kid",
        )
    })
    .await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_token_invalid"));
    // Rate limit: no second refetch within 60 seconds for the same issuer.
    assert_eq!(idp.state.lock().unwrap().jwks_fetches, 2);
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn jwks_refetch_does_not_extend_the_metadata_cache() {
    let pool = init_pg_pool().await;
    let ttl = std::time::Duration::from_millis(2000);
    let app = build_app_with(&pool, OidcClient::with_ttl(ttl).unwrap()).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let email = || format!("{}@example.com", unique("ttl"));

    // t=0: discovery + JWKS fetched and cached.
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &unique("sub"), &email()))
    })
    .await;
    one_time_code(&location);

    // t~1.2s: unknown kid refetches only the JWKS.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    {
        let mut state = idp.state.lock().unwrap();
        let other: Value = serde_json::from_str(OTHER_JWK).unwrap();
        state.jwks["keys"].as_array_mut().unwrap().push(other);
    }
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign_with(
            &claims(&idp, &a.nonce, &unique("sub"), &email()),
            OTHER_PRIVATE_KEY,
            "other-key",
        )
    })
    .await;
    one_time_code(&location);
    {
        let state = idp.state.lock().unwrap();
        assert_eq!((state.discovery_fetches, state.jwks_fetches), (1, 2));
    }

    // t~2.4s: past the TTL of the first discovery fetch (but not of the JWKS
    // refetch): the metadata is loaded again.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &unique("sub"), &email()))
    })
    .await;
    one_time_code(&location);
    assert_eq!(idp.state.lock().unwrap().discovery_fetches, 2);
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn callback_requires_the_state_cookie_of_the_starting_browser() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let email = format!("{sub}@example.com");

    let victim = authorize(&app, &idp, &provider.slug, None).await;
    let attacker = authorize(&app, &idp, &provider.slug, None).await;
    let code = "csrf-code";
    idp.grant(
        code,
        &victim.challenge,
        &victim.redirect_uri,
        sign(&claims(&idp, &victim.nonce, &sub, &email)),
    );

    // No cookie, and a cookie of another authorization request: rejected.
    let location = callback(&app, &provider.slug, code, &victim.state, None).await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));
    let location = callback(
        &app,
        &provider.slug,
        code,
        &victim.state,
        Some(&attacker.cookie),
    )
    .await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));
    let location = callback(
        &app,
        &provider.slug,
        code,
        &victim.state,
        Some(&victim.state),
    )
    .await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));
    assert_eq!(identity_user(&pool, &provider, &sub).await, None);

    // The rejected attempts did not burn the state: the right browser completes.
    let location = callback(
        &app,
        &provider.slug,
        code,
        &victim.state,
        Some(&victim.cookie),
    )
    .await;
    one_time_code(&location);
    assert!(identity_user(&pool, &provider, &sub).await.is_some());
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn unverified_email_never_satisfies_a_domain_restriction() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let restricted = create_provider(
        &pool,
        &idp,
        ProviderOpts {
            domains: vec!["allowed.example".to_string()],
            ..Default::default()
        },
    )
    .await;
    let open = create_provider(&pool, &idp, ProviderOpts::default()).await;

    for verified in [json!(false), json!("false"), Value::Null] {
        let sub = unique("sub");
        let email = format!("{}@allowed.example", unique("unv"));
        let location = login_with(&app, &idp, &restricted.slug, None, |a| {
            let mut c = claims(&idp, &a.nonce, &sub, &email);
            c["email_verified"] = verified.clone();
            sign(&c)
        })
        .await;
        assert_eq!(
            sso_error(&location).as_deref(),
            Some("sso_email_domain_not_allowed"),
            "{verified}"
        );
    }

    // Without a domain list, an unverified email may still be provisioned.
    let sub = unique("sub");
    let email = format!("{}@anywhere.example", unique("unv"));
    let location = login_with(&app, &idp, &open.slug, None, |a| {
        let mut c = claims(&idp, &a.nonce, &sub, &email);
        c["email_verified"] = json!(false);
        sign(&c)
    })
    .await;
    one_time_code(&location);
    assert!(identity_user(&pool, &open, &sub).await.is_some());
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn email_linking_never_links_a_system_admin() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let linking = create_provider(
        &pool,
        &idp,
        ProviderOpts {
            linking: true,
            ..Default::default()
        },
    )
    .await;
    let email = format!("{}@example.com", unique("admin-link"));
    insert_user(&pool, &email, Some(PASSWORD), true, "local").await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &linking.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    assert_eq!(
        sso_error(&location).as_deref(),
        Some("sso_linking_not_allowed")
    );
    assert_eq!(identity_user(&pool, &linking, &sub).await, None);
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn several_audiences_require_azp_of_this_client() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;

    for (azp, accepted) in [
        (None, false),
        (Some("other-client"), false),
        (Some(CLIENT_ID), true),
    ] {
        let sub = unique("sub");
        let location = login_with(&app, &idp, &provider.slug, None, |a| {
            let mut c = claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com"));
            c["aud"] = json!([CLIENT_ID, "other-client"]);
            if let Some(azp) = azp {
                c["azp"] = json!(azp);
            }
            sign(&c)
        })
        .await;
        if accepted {
            one_time_code(&location);
        } else {
            assert_eq!(
                sso_error(&location).as_deref(),
                Some("sso_token_invalid"),
                "{azp:?}"
            );
        }
    }
    cleanup(&pool, &idp).await;
}

async fn last_login(pool: &PgPool, user_id: Uuid) -> Option<chrono::DateTime<chrono::Utc>> {
    sqlx::query_scalar("SELECT last_login FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[actix_web::test]
async fn last_login_is_set_only_when_a_session_is_issued_by_sso() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com")))
    })
    .await;
    let code = one_time_code(&location);
    let user_id = identity_user(&pool, &provider, &sub).await.unwrap();
    assert_eq!(last_login(&pool, user_id).await, None, "callback alone");
    let (status, body) = exchange(&app, &code).await;
    assert_eq!(status, StatusCode::OK);
    assert!(last_login(&pool, user_id).await.is_some(), "after exchange");
    assert!(body["user"]["lastLogin"].is_string());
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn state_and_provider_errors_redirect_with_codes() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;

    // Unknown and missing state.
    let fake = "not-a-real-state";
    let location = callback(&app, &provider.slug, "c", fake, Some(&cookie_for(fake))).await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));
    let location = callback_query(&app, &provider.slug, "code=c", None).await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));

    // IdP error parameter: provider error, and the state is used up.
    let auth = authorize(&app, &idp, &provider.slug, None).await;
    let location = callback_query(
        &app,
        &provider.slug,
        &format!("error=access_denied&state={}", auth.state),
        Some(&auth.cookie),
    )
    .await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_provider_error"));
    let location = callback(&app, &provider.slug, "c", &auth.state, Some(&auth.cookie)).await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));

    // A state issued for one provider is not accepted on another provider's callback.
    let other = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let auth = authorize(&app, &idp, &provider.slug, None).await;
    let location = callback(&app, &other.slug, "c", &auth.state, Some(&auth.cookie)).await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));

    // A failed token exchange (unknown code) is a provider error and burns the state.
    let auth = authorize(&app, &idp, &provider.slug, None).await;
    let location = callback(
        &app,
        &provider.slug,
        "never-granted",
        &auth.state,
        Some(&auth.cookie),
    )
    .await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_provider_error"));

    // An expired state is rejected.
    let auth = authorize(&app, &idp, &provider.slug, None).await;
    sqlx::query("UPDATE sso_login_states SET expires_at = now() - INTERVAL '1 second' WHERE provider_id = $1")
        .bind(provider.id)
        .execute(&pool)
        .await
        .unwrap();
    let code = "expired-state-code";
    idp.grant(
        code,
        &auth.challenge,
        &auth.redirect_uri,
        sign(&claims(&idp, &auth.nonce, &unique("sub"), "e@example.com")),
    );
    let location = callback(&app, &provider.slug, code, &auth.state, Some(&auth.cookie)).await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_state_invalid"));

    // Unknown, disabled and malformed providers at authorize.
    let disabled = create_provider(
        &pool,
        &idp,
        ProviderOpts {
            enabled: false,
            ..Default::default()
        },
    )
    .await;
    for slug in ["no-such-provider", disabled.slug.as_str(), "Bad_Slug"] {
        let location = get_location(&app, &format!("/api/v1/auth/sso/{slug}/authorize")).await;
        assert_eq!(
            location,
            format!("{UI}/login?ssoError=sso_provider_error"),
            "{slug}"
        );
    }
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn open_redirect_attempts_are_dropped() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;

    for evil in [
        "//evil.example",
        "https://evil.example",
        "/\\evil.example",
        "evil",
    ] {
        let sub = unique("sub");
        let location = login_with(&app, &idp, &provider.slug, Some(evil), |a| {
            sign(&claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com")))
        })
        .await;
        assert!(
            location.starts_with(&format!("{UI}/auth/sso/complete?")),
            "{location}"
        );
        assert!(
            !query_of(&location).contains_key("redirect"),
            "{evil}: {location}"
        );
    }
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn expired_exchange_code_is_rejected() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com")))
    })
    .await;
    let code = one_time_code(&location);
    let expires_in: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (expires_at - created_at))::float8 FROM sso_login_codes
         WHERE provider_id = $1",
    )
    .bind(provider.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((59.0..=61.0).contains(&expires_in), "{expires_in}");
    sqlx::query("UPDATE sso_login_codes SET expires_at = now() - INTERVAL '1 second' WHERE provider_id = $1")
        .bind(provider.id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = exchange(&app, &code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid_sso_code");
    let (status, _) = exchange(&app, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn missing_email_falls_back_to_userinfo_then_fails() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;

    let sub = unique("sub");
    let email = format!("{}@example.com", unique("userinfo"));
    idp.state.lock().unwrap().userinfo_sub = sub.clone();
    idp.state.lock().unwrap().userinfo_email = Some(email.clone());
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        let mut c = claims(&idp, &a.nonce, &sub, "unused");
        c.as_object_mut().unwrap().remove("email");
        sign(&c)
    })
    .await;
    one_time_code(&location);
    let user_id = identity_user(&pool, &provider, &sub).await.unwrap();
    let stored: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, email);

    idp.state.lock().unwrap().userinfo_email = None;
    let sub = unique("sub");
    idp.state.lock().unwrap().userinfo_sub = sub.clone();
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        let mut c = claims(&idp, &a.nonce, &sub, "unused");
        c.as_object_mut().unwrap().remove("email");
        sign(&c)
    })
    .await;
    assert_eq!(sso_error(&location).as_deref(), Some("sso_email_missing"));
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
async fn client_secret_post_is_used_when_basic_is_not_offered() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    idp.state.lock().unwrap().auth_methods = vec!["client_secret_post".to_string()];
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let sub = unique("sub");
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &format!("{sub}@example.com")))
    })
    .await;
    one_time_code(&location);
    {
        let state = idp.state.lock().unwrap();
        let call = state.token_calls.last().unwrap();
        assert!(call.basic.is_none());
        assert_eq!(call.form["client_secret"], CLIENT_SECRET);
        assert_eq!(call.form["client_id"], CLIENT_ID);
    }
    cleanup(&pool, &idp).await;
}

#[actix_web::test]
#[serial(sso_settings)]
async fn enforce_sso_blocks_password_login_for_non_admins_only() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let (user_id, user) = insert_user(
        &pool,
        &format!("{}@example.com", unique("enf")),
        Some(PASSWORD),
        false,
        "local",
    )
    .await;
    let (_, admin) = insert_user(
        &pool,
        &format!("{}@example.com", unique("enf-admin")),
        Some(PASSWORD),
        true,
        "local",
    )
    .await;
    let (_, sso_only) = insert_user(
        &pool,
        &format!("{}@example.com", unique("enf-sso")),
        None,
        false,
        "sso",
    )
    .await;

    let set = |on: bool| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE sso_settings SET enforce_sso = $1")
                .bind(on)
                .execute(&pool)
                .await
                .unwrap();
        }
    };

    // Without enforcement a non-admin logs in; an SSO-only user never can.
    let (status, body) = password_login(&app, &user, PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["authSource"], "local");
    assert!(body["user"]["identities"].is_array());
    let (status, _) = password_login(&app, &sso_only, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = password_login(&app, &sso_only, PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A successful password login records last_login.
    assert!(last_login(&pool, user_id).await.is_some());
    sqlx::query("UPDATE users SET last_login = NULL WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    set(true).await;
    let outcome = async {
        let (status, body) = password_login(&app, &user, PASSWORD).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "sso_required");
        // A refused login is not a login.
        assert_eq!(last_login(&pool, user_id).await, None);
        // Wrong password: the normal 401, not revealing the account.
        let (status, body) = password_login(&app, &user, "wrong password").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_ne!(body["error"], "sso_required");
        let (status, _) = password_login(&app, "no-such-user-sso", PASSWORD).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = password_login(&app, &sso_only, PASSWORD).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // System admins keep password login (break-glass).
        let (status, body) = password_login(&app, &admin, PASSWORD).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    };
    // Restore the global setting even if an assertion fails.
    let result = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(outcome)).await;
    set(false).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[actix_web::test]
async fn deleting_a_role_or_team_removes_its_group_mappings() {
    let pool = init_pg_pool().await;
    let idp_issuer = "https://mapping-cleanup.example";
    let provider_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sso_providers (id, slug, display_name, issuer_url, client_id)
         VALUES ($1, $2, 'Cleanup', $3, 'c')",
    )
    .bind(provider_id)
    .bind(unique("cleanup"))
    .bind(idp_issuer)
    .execute(&pool)
    .await
    .unwrap();

    let role = role_repository(pool.clone())
        .create_role(&unique("sso-cleanup-role"), "temp")
        .await
        .unwrap();
    let team = team_repository(pool.clone())
        .create_team(feature_toggle_backend::database::team::CreateTeam {
            name: unique("sso-cleanup-team"),
            description: "temp".to_string(),
        })
        .await
        .unwrap();
    for (target_type, target_id) in [("role", role.id), ("team", team.id)] {
        sqlx::query(
            "INSERT INTO sso_group_mappings (id, provider_id, group_value, target_type, target_id)
             VALUES ($1, $2, 'g', $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(provider_id)
        .bind(target_type)
        .bind(target_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    let count = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sso_group_mappings WHERE provider_id = $1",
        )
        .bind(provider_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(count(pool.clone()).await, 2);
    role_repository(pool.clone())
        .delete_role(role.id)
        .await
        .unwrap();
    assert_eq!(count(pool.clone()).await, 1);
    team_repository(pool.clone())
        .delete_team(team.id)
        .await
        .unwrap();
    assert_eq!(count(pool.clone()).await, 0);

    sqlx::query("DELETE FROM sso_providers WHERE id = $1")
        .bind(provider_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[actix_web::test]
async fn groups_from_userinfo_sync_roles_and_teams_across_logins() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool).await;
    let idp = start_idp().await;
    let provider = create_provider(&pool, &idp, ProviderOpts::default()).await;
    let role_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sync')")
        .bind(team_id)
        .bind(format!("sync-team-{team_id}"))
        .execute(&pool)
        .await
        .unwrap();
    for (group, kind, target) in [
        ("fluxgate-devs", "role", Some(role_id)),
        ("fluxgate-devs", "team", Some(team_id)),
    ] {
        sqlx::query(
            "INSERT INTO sso_group_mappings (id, provider_id, group_value, target_type, target_id)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(provider.id)
        .bind(group)
        .bind(kind)
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    }

    // The id_token has no groups claim: userinfo supplies them.
    let sub = unique("sub");
    idp.state.lock().unwrap().userinfo_sub = sub.clone();
    idp.state.lock().unwrap().userinfo_groups = Some(json!(["fluxgate-devs"]));
    let email = format!("{sub}@example.com");
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    let code = one_time_code(&location);
    let user_id = identity_user(&pool, &provider, &sub).await.unwrap();
    let (status, _) = exchange(&app, &code).await;
    assert_eq!(status, StatusCode::OK);
    let roles: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT role_id, source FROM user_roles WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(roles, vec![(role_id, "sso".to_string())]);
    let teams: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT team_id, source FROM user_teams WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(teams, vec![(team_id, "sso".to_string())]);

    // The group is gone at the IdP: authoritative sync removes both.
    idp.state.lock().unwrap().userinfo_groups = Some(json!([]));
    let location = login_with(&app, &idp, &provider.slug, None, |a| {
        sign(&claims(&idp, &a.nonce, &sub, &email))
    })
    .await;
    one_time_code(&location);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM user_roles WHERE user_id = $1)
              + (SELECT count(*) FROM user_teams WHERE user_id = $1)",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &idp).await;
}
