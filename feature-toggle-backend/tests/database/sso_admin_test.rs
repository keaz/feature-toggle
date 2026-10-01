//! SSO admin REST API: provider CRUD, secret handling, mappings, settings, the public
//! provider list, the connection test against an in-process mock IdP, and the
//! SSO-aware user API.

use actix_web::body::MessageBody;
use actix_web::dev::{Service, ServiceResponse};
use actix_web::http::{Method, StatusCode};
use actix_web::{App, HttpMessage, test, web};
use feature_toggle_backend::JwtUser;
use feature_toggle_backend::database::activity_log::{
    ActivityLogRepository, activity_log_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::role::{RoleRepositoryTx, role_repository_tx};
use feature_toggle_backend::database::sso_provider::sso_provider_repository;
use feature_toggle_backend::database::user::user_repository;
use feature_toggle_backend::logic::secret_box::{SecretBox, SecretBoxError};
use feature_toggle_backend::logic::sso_provider::{
    SsoSecrets, client_secret_env_var, resolve_client_secret,
};
use feature_toggle_backend::logic::user::{UserLogic, user_logic};
use feature_toggle_backend::rest;
use serde_json::{Value, json};
use serial_test::serial;
use sqlx::PgPool;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";
const SECRET: &str = "super-secret-client-value";

fn secrets_with_key() -> SsoSecrets {
    let key = base64_key([5u8; 32]);
    SsoSecrets::with_box(SecretBox::from_base64_key(&key).unwrap())
}

fn base64_key(bytes: [u8; 32]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unique_slug(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..10])
}

/// The seeded administrator (`init.sql`); activity log entries reference a real user.
fn admin_user() -> JwtUser {
    JwtUser {
        id: Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap(),
        username: "sso-admin-test".to_string(),
        is_admin: true,
        roles: vec![],
        team_id: None,
        token_hash: "hash".to_string(),
    }
}

fn plain_user() -> JwtUser {
    JwtUser {
        is_admin: false,
        username: "sso-plain-test".to_string(),
        ..admin_user()
    }
}

async fn build_app(
    pool: &PgPool,
    secrets: SsoSecrets,
) -> impl Service<
    actix_http::Request,
    Response = ServiceResponse<impl MessageBody>,
    Error = actix_web::Error,
> {
    let activity: Box<dyn ActivityLogRepository> = activity_log_repository(pool.clone());
    let users: Box<dyn UserLogic> = user_logic(user_repository(pool.clone()), activity.clone());
    test::init_service(
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(activity))
            .app_data(web::Data::new(users))
            .app_data(web::Data::new(secrets))
            .service(
                web::scope("/api/v1")
                    .configure(rest::sso::configure)
                    .configure(rest::user::configure)
                    .configure(rest::auth::configure),
            ),
    )
    .await
}

async fn call<S, B>(
    app: &S,
    method: Method,
    uri: &str,
    body: Option<Value>,
    user: Option<JwtUser>,
) -> (StatusCode, Value, String)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let mut builder = test::TestRequest::default().method(method).uri(uri);
    if let Some(body) = body {
        builder = builder.set_json(body);
    }
    let req = builder.to_request();
    if let Some(user) = user {
        req.extensions_mut().insert(user);
    }
    let resp = test::call_service(app, req).await;
    let status = resp.status();
    let bytes = test::read_body(resp).await;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, text)
}

fn provider_body(slug: &str) -> Value {
    json!({
        "slug": slug,
        "displayName": "Test IdP",
        "issuerUrl": "https://idp.example.com",
        "clientId": "client-1",
    })
}

async fn create_provider<S, B>(app: &S, slug: &str, extra: Value) -> Value
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let mut body = provider_body(slug);
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let (status, json, text) = call(
        app,
        Method::POST,
        "/api/v1/sso/providers",
        Some(body),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    json
}

async fn delete_provider<S, B>(app: &S, id: &str)
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let (status, _, text) = call(
        app,
        Method::DELETE,
        &format!("/api/v1/sso/providers/{id}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
}

// ------------------------------------------------------------------ access control

#[actix_web::test]
#[serial(sso_settings)]
async fn every_admin_route_rejects_non_admin_and_anonymous() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let id = Uuid::new_v4();
    let routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/v1/sso/providers".into(), None),
        (
            Method::POST,
            "/api/v1/sso/providers".into(),
            Some(provider_body("denied")),
        ),
        (Method::GET, format!("/api/v1/sso/providers/{id}"), None),
        (
            Method::PATCH,
            format!("/api/v1/sso/providers/{id}"),
            Some(json!({"enabled": true})),
        ),
        (Method::DELETE, format!("/api/v1/sso/providers/{id}"), None),
        (
            Method::POST,
            format!("/api/v1/sso/providers/{id}/test"),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/sso/providers/{id}/mappings"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/v1/sso/providers/{id}/mappings"),
            Some(json!([])),
        ),
        (Method::GET, "/api/v1/sso/settings".into(), None),
        (
            Method::PUT,
            "/api/v1/sso/settings".into(),
            Some(json!({"enforceSso": true})),
        ),
    ];
    for (method, uri, body) in routes {
        let (status, _, text) =
            call(&app, method.clone(), &uri, body.clone(), Some(plain_user())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {text}");
        let (status, _, text) = call(&app, method.clone(), &uri, body, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}: {text}");
    }
    // The rejected create did not create anything.
    assert!(
        sso_provider_repository(pool.clone())
            .find_provider_by_slug("denied")
            .await
            .unwrap()
            .is_none()
    );
}

// ------------------------------------------------------------------ CRUD and secrets

#[actix_web::test]
async fn provider_crud_round_trip_never_exposes_the_secret() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let slug = unique_slug("crud");
    let mut all_bodies: Vec<String> = Vec::new();

    // Create with defaults and a secret.
    let (status, created, text) = call(
        &app,
        Method::POST,
        "/api/v1/sso/providers",
        Some({
            let mut b = provider_body(&slug);
            b["clientSecret"] = json!(SECRET);
            b
        }),
        Some(admin_user()),
    )
    .await;
    all_bodies.push(text);
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["slug"], slug);
    assert_eq!(created["displayName"], "Test IdP");
    assert_eq!(created["issuerUrl"], "https://idp.example.com");
    assert_eq!(created["clientId"], "client-1");
    assert_eq!(created["hasClientSecret"], true);
    assert_eq!(created["clientSecretFromEnv"], false);
    assert_eq!(created["scopes"], json!(["openid", "email", "profile"]));
    assert_eq!(created["groupsClaim"], "groups");
    assert_eq!(created["allowedEmailDomains"], json!([]));
    assert_eq!(created["jitProvisioning"], true);
    assert_eq!(created["allowEmailLinking"], false);
    assert_eq!(created["roleSyncMode"], "authoritative");
    assert_eq!(created["enabled"], false);
    assert!(created["createdAt"].is_string() && created["updatedAt"].is_string());
    assert!(created.get("clientSecret").is_none());
    assert!(created.get("clientSecretEnc").is_none());

    // The stored value is ciphertext, not the secret.
    let stored: String =
        sqlx::query_scalar("SELECT client_secret_enc FROM sso_providers WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(stored, SECRET);
    assert!(!stored.contains(SECRET));

    // Get and list.
    let (status, got, text) = call(
        &app,
        Method::GET,
        &format!("/api/v1/sso/providers/{id}"),
        None,
        Some(admin_user()),
    )
    .await;
    all_bodies.push(text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, created);
    let (status, list, text) = call(
        &app,
        Method::GET,
        "/api/v1/sso/providers",
        None,
        Some(admin_user()),
    )
    .await;
    all_bodies.push(text);
    assert_eq!(status, StatusCode::OK);
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == json!(id))
    );

    // Patch without clientSecret keeps the secret; other fields change.
    let (status, patched, text) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{id}"),
        Some(json!({
            "displayName": "Renamed",
            "enabled": true,
            "scopes": ["openid", "groups"],
            "allowedEmailDomains": ["Example.com"],
            "roleSyncMode": "additive",
            "allowEmailLinking": true,
            "jitProvisioning": false,
            "groupsClaim": "roles",
        })),
        Some(admin_user()),
    )
    .await;
    all_bodies.push(text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["displayName"], "Renamed");
    assert_eq!(patched["enabled"], true);
    assert_eq!(patched["scopes"], json!(["openid", "groups"]));
    assert_eq!(patched["allowedEmailDomains"], json!(["example.com"]));
    assert_eq!(patched["roleSyncMode"], "additive");
    assert_eq!(patched["allowEmailLinking"], true);
    assert_eq!(patched["jitProvisioning"], false);
    assert_eq!(patched["groupsClaim"], "roles");
    assert_eq!(patched["slug"], slug);
    assert_eq!(
        patched["hasClientSecret"], true,
        "omitted secret is unchanged"
    );
    let stored_after: String =
        sqlx::query_scalar("SELECT client_secret_enc FROM sso_providers WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, stored_after);

    // Replace the secret.
    let (status, replaced, text) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{id}"),
        Some(json!({"clientSecret": "another-secret"})),
        Some(admin_user()),
    )
    .await;
    all_bodies.push(text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replaced["hasClientSecret"], true);
    let replaced_enc: String =
        sqlx::query_scalar("SELECT client_secret_enc FROM sso_providers WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(replaced_enc, stored);

    // Empty string clears it.
    let (status, cleared, text) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{id}"),
        Some(json!({"clientSecret": ""})),
        Some(admin_user()),
    )
    .await;
    all_bodies.push(text);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["hasClientSecret"], false);

    // Duplicate slug conflicts.
    let (status, _, _) = call(
        &app,
        Method::POST,
        "/api/v1/sso/providers",
        Some(provider_body(&slug)),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // The secret and its ciphertext never appeared in any response.
    for body in &all_bodies {
        assert!(!body.contains(SECRET), "{body}");
        assert!(!body.contains("another-secret"), "{body}");
        assert!(!body.contains(&stored), "{body}");
        assert!(!body.contains(&replaced_enc), "{body}");
    }

    // The activity log recorded create, update and delete without secrets.
    delete_provider(&app, &id).await;
    let (status, _, _) = call(
        &app,
        Method::GET,
        &format!("/api/v1/sso/providers/{id}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let rows: Vec<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT activity_type, metadata FROM activity_log WHERE entity_type = 'sso_provider' AND entity_id = $1 ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let types: Vec<&str> = rows.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "sso_provider_created",
            "sso_provider_updated",
            "sso_provider_updated",
            "sso_provider_updated",
            "sso_provider_deleted"
        ]
    );
    for (_, metadata) in rows {
        let text = metadata.unwrap().to_string();
        assert!(!text.contains(SECRET) && !text.contains(&stored));
    }
}

#[actix_web::test]
async fn secret_is_bound_to_its_provider_row() {
    let pool = init_pg_pool().await;
    let secrets = secrets_with_key();
    let app = build_app(&pool, secrets.clone()).await;
    let p1 = create_provider(
        &app,
        &unique_slug("aad"),
        json!({"clientSecret": "secret-one"}),
    )
    .await;
    let p2 = create_provider(
        &app,
        &unique_slug("aad"),
        json!({"clientSecret": "secret-two"}),
    )
    .await;
    let (id1, id2) = (
        p1["id"].as_str().unwrap().to_string(),
        p2["id"].as_str().unwrap().to_string(),
    );

    let repo = sso_provider_repository(pool.clone());
    let load = |id: &str| {
        let repo = &repo;
        let id = Uuid::parse_str(id).unwrap();
        async move { repo.get_provider_by_id(id).await.unwrap() }
    };
    assert_eq!(
        resolve_client_secret(&load(&id1).await, &secrets).unwrap(),
        Some("secret-one".to_string())
    );
    assert_eq!(
        resolve_client_secret(&load(&id2).await, &secrets).unwrap(),
        Some("secret-two".to_string())
    );

    // Copy provider one's ciphertext onto provider two: it must not decrypt there.
    sqlx::query(
        "UPDATE sso_providers SET client_secret_enc = (SELECT client_secret_enc FROM sso_providers WHERE id = $1::uuid) WHERE id = $2::uuid",
    )
    .bind(&id1)
    .bind(&id2)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolve_client_secret(&load(&id2).await, &secrets),
        Err(SecretBoxError::DecryptionFailed)
    );

    delete_provider(&app, &id1).await;
    delete_provider(&app, &id2).await;
}

#[actix_web::test]
async fn env_override_sets_client_secret_from_env() {
    let pool = init_pg_pool().await;
    let secrets = secrets_with_key();
    let app = build_app(&pool, secrets.clone()).await;
    let slug = unique_slug("envov");
    let var = client_secret_env_var(&slug);
    assert!(var.starts_with("FLUXGATE_SSO_ENVOV_"));
    assert!(var.ends_with("_CLIENT_SECRET"));

    let created = create_provider(&app, &slug, json!({})).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["clientSecretFromEnv"], false);
    assert_eq!(created["hasClientSecret"], false);

    // SAFETY: the variable name is unique to this test.
    unsafe { std::env::set_var(&var, "env-secret-value") };
    let (status, got, text) = call(
        &app,
        Method::GET,
        &format!("/api/v1/sso/providers/{id}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["clientSecretFromEnv"], true);
    assert_eq!(
        got["hasClientSecret"], false,
        "nothing stored in the database"
    );
    assert!(!text.contains("env-secret-value"));

    let provider = sso_provider_repository(pool.clone())
        .get_provider_by_id(Uuid::parse_str(&id).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resolve_client_secret(&provider, &secrets).unwrap(),
        Some("env-secret-value".to_string())
    );

    // An empty value does not count as an override.
    unsafe { std::env::set_var(&var, "") };
    let (_, got, _) = call(
        &app,
        Method::GET,
        &format!("/api/v1/sso/providers/{id}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(got["clientSecretFromEnv"], false);

    unsafe { std::env::remove_var(&var) };
    delete_provider(&app, &id).await;
}

#[actix_web::test]
async fn saving_a_secret_without_encryption_key_is_rejected() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, SsoSecrets::disabled()).await;
    let slug = unique_slug("nokey");

    // Create with a secret: 400 encryption_key_missing and nothing persisted.
    let mut body = provider_body(&slug);
    body["clientSecret"] = json!(SECRET);
    let (status, json, text) = call(
        &app,
        Method::POST,
        "/api/v1/sso/providers",
        Some(body),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "encryption_key_missing");
    assert!(!text.contains(SECRET));
    assert!(
        sso_provider_repository(pool.clone())
            .find_provider_by_slug(&slug)
            .await
            .unwrap()
            .is_none()
    );

    // Without a secret the provider is fine.
    let created = create_provider(&app, &slug, json!({})).await;
    let id = created["id"].as_str().unwrap().to_string();

    // Patch with a non-empty secret: same error, nothing changed.
    let (status, json, _) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{id}"),
        Some(json!({"clientSecret": SECRET, "displayName": "Changed"})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "encryption_key_missing");
    let (_, got, _) = call(
        &app,
        Method::GET,
        &format!("/api/v1/sso/providers/{id}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(got["displayName"], "Test IdP");

    // Clearing needs no key.
    let (status, _, _) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{id}"),
        Some(json!({"clientSecret": ""})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    delete_provider(&app, &id).await;
}

#[actix_web::test]
async fn provider_validation_errors_are_invalid_input() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let cases = vec![
        json!({"slug": "Bad_Slug", "displayName": "x", "issuerUrl": "https://a.example", "clientId": "c"}),
        json!({"slug": "-lead", "displayName": "x", "issuerUrl": "https://a.example", "clientId": "c"}),
        json!({"slug": "ok", "displayName": "", "issuerUrl": "https://a.example", "clientId": "c"}),
        json!({"slug": "ok", "displayName": "x", "issuerUrl": "not-a-url", "clientId": "c"}),
        json!({"slug": "ok", "displayName": "x", "issuerUrl": "ftp://a.example", "clientId": "c"}),
        json!({"slug": "ok", "displayName": "x", "issuerUrl": "https://a.example", "clientId": ""}),
        json!({"slug": "ok", "displayName": "x", "issuerUrl": "https://a.example", "clientId": "c", "roleSyncMode": "bogus"}),
        json!({"slug": "ok", "displayName": "x", "issuerUrl": "https://a.example", "clientId": "c", "scopes": ["email"]}),
        json!({"displayName": "x", "issuerUrl": "https://a.example", "clientId": "c"}),
    ];
    for body in cases {
        let (status, json, text) = call(
            &app,
            Method::POST,
            "/api/v1/sso/providers",
            Some(body.clone()),
            Some(admin_user()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {text}");
        assert_eq!(json["error"], "invalid_input", "{body}: {text}");
        assert!(json["message"].is_string());
    }
    // Patch validation and unknown ids.
    let (status, json, _) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{}", Uuid::new_v4()),
        Some(json!({"roleSyncMode": "bogus"})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "invalid_input");
    let (status, _, _) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/sso/providers/{}", Uuid::new_v4()),
        Some(json!({"enabled": true})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(
        &app,
        Method::GET,
        "/api/v1/sso/providers/not-a-uuid",
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[actix_web::test]
async fn deleting_a_provider_removes_identities_and_mappings_but_keeps_users() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let created = create_provider(&app, &unique_slug("del"), json!({})).await;
    let id = created["id"].as_str().unwrap().to_string();
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email, auth_source)
         VALUES ($1, $2, NULL, 'Sso', 'Del', $3, 'sso')",
    )
    .bind(user_id)
    .bind(format!("sso-del-{user_id}"))
    .bind(format!("sso-del-{user_id}@example.com"))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_identities (id, user_id, provider_id, subject, email) VALUES ($1, $2, $3::uuid, 'sub-1', 'x@example.com')",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&id)
    .execute(&pool)
    .await
    .unwrap();
    let (status, _, text) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/sso/providers/{id}/mappings"),
        Some(json!([{"groupValue": "admins", "targetType": "admin", "targetId": null}])),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");

    delete_provider(&app, &id).await;

    let identities: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_identities WHERE provider_id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mappings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sso_group_mappings WHERE provider_id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((identities, mappings, users), (0, 0, 1));
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[actix_web::test]
async fn changing_the_issuer_clears_the_providers_identities() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let created = create_provider(&app, &unique_slug("iss"), json!({})).await;
    let id = created["id"].as_str().unwrap().to_string();
    let user_id = insert_user_row(&pool, "sso", None).await;
    sqlx::query(
        "INSERT INTO user_identities (id, user_id, provider_id, subject, email) VALUES ($1, $2, $3::uuid, 'sub-iss', 'x@example.com')",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(&id)
    .execute(&pool)
    .await
    .unwrap();
    let identities = || {
        let pool = pool.clone();
        let id = id.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM user_identities WHERE provider_id = $1::uuid",
            )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let last_metadata = || {
        let pool = pool.clone();
        let id = id.clone();
        async move {
            sqlx::query_scalar::<_, Value>(
                "SELECT metadata FROM activity_log WHERE activity_type = 'sso_provider_updated'
                 AND entity_id = $1 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let patch = |body: Value| {
        let (app, id) = (&app, id.clone());
        async move {
            let (status, _, text) = call(
                app,
                Method::PATCH,
                &format!("/api/v1/sso/providers/{id}"),
                Some(body),
                Some(admin_user()),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{text}");
        }
    };

    // Same issuer (or another field): identities stay.
    patch(json!({"issuerUrl": "https://idp.example.com", "displayName": "Renamed"})).await;
    assert_eq!(identities().await, 1);
    let metadata = last_metadata().await;
    assert_eq!(metadata["issuer_changed"], false);
    assert_eq!(metadata["identities_cleared"], 0);

    // New issuer: identities go, the user stays.
    patch(json!({"issuerUrl": "https://other-idp.example.com"})).await;
    assert_eq!(identities().await, 0);
    let metadata = last_metadata().await;
    assert_eq!(metadata["issuer_changed"], true);
    assert_eq!(metadata["identities_cleared"], 1);
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 1);

    delete_provider(&app, &id).await;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
}

// ------------------------------------------------------------------ mappings

#[actix_web::test]
async fn mapping_validation_and_replace_semantics() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let created = create_provider(&app, &unique_slug("map"), json!({})).await;
    let id = created["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/sso/providers/{id}/mappings");
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sso map test')")
        .bind(team_id)
        .bind(format!("sso-map-{team_id}"))
        .execute(&pool)
        .await
        .unwrap();

    let put = |body: Value| {
        let (app, uri) = (&app, uri.clone());
        async move { call(app, Method::PUT, &uri, Some(body), Some(admin_user())).await }
    };

    // Starts empty.
    let (status, list, _) = call(&app, Method::GET, &uri, None, Some(admin_user())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([]));

    // Valid replace: role, team, admin.
    let (status, mappings, text) = put(json!([
        {"groupValue": "approvers", "targetType": "role", "targetId": APPROVER_ROLE_ID},
        {"groupValue": "platform", "targetType": "team", "targetId": team_id.to_string()},
        {"groupValue": "root", "targetType": "admin", "targetId": null},
    ]))
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let mappings = mappings.as_array().unwrap();
    assert_eq!(mappings.len(), 3);
    for m in mappings {
        assert!(m["id"].is_string());
    }
    let admin = mappings
        .iter()
        .find(|m| m["targetType"] == "admin")
        .unwrap();
    assert!(admin["targetId"].is_null());
    let role = mappings.iter().find(|m| m["targetType"] == "role").unwrap();
    assert_eq!(role["groupValue"], "approvers");
    assert_eq!(role["targetId"], APPROVER_ROLE_ID);

    // GET returns the same set.
    let (_, listed, _) = call(&app, Method::GET, &uri, None, Some(admin_user())).await;
    assert_eq!(listed.as_array().unwrap().len(), 3);

    // Each invalid body is a 400 invalid_input and leaves the stored set unchanged.
    let missing = Uuid::new_v4().to_string();
    let invalid = vec![
        json!([{"groupValue": "g", "targetType": "role", "targetId": missing}]),
        json!([{"groupValue": "g", "targetType": "team", "targetId": missing}]),
        json!([{"groupValue": "g", "targetType": "role", "targetId": null}]),
        json!([{"groupValue": "g", "targetType": "admin", "targetId": APPROVER_ROLE_ID}]),
        json!([{"groupValue": "g", "targetType": "bogus", "targetId": null}]),
        json!([{"groupValue": "  ", "targetType": "admin", "targetId": null}]),
        json!([{"groupValue": "g", "targetType": "role", "targetId": "not-a-uuid"}]),
        // The team id is not a role and the role id is not a team.
        json!([{"groupValue": "g", "targetType": "role", "targetId": team_id.to_string()}]),
        json!([{"groupValue": "g", "targetType": "team", "targetId": APPROVER_ROLE_ID}]),
        // Duplicates, including duplicate admin mappings.
        json!([
            {"groupValue": "dup", "targetType": "role", "targetId": APPROVER_ROLE_ID},
            {"groupValue": "dup", "targetType": "role", "targetId": APPROVER_ROLE_ID},
        ]),
        json!([
            {"groupValue": "dup", "targetType": "admin", "targetId": null},
            {"groupValue": " dup ", "targetType": "admin", "targetId": null},
        ]),
    ];
    for body in invalid {
        let (status, json, text) = put(body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {text}");
        assert_eq!(json["error"], "invalid_input", "{body}: {text}");
    }
    let (_, listed, _) = call(&app, Method::GET, &uri, None, Some(admin_user())).await;
    assert_eq!(
        listed.as_array().unwrap().len(),
        3,
        "failed replaces roll back"
    );

    // The same group may map to several different targets.
    let (status, multi, text) = put(json!([
        {"groupValue": "eng", "targetType": "role", "targetId": APPROVER_ROLE_ID},
        {"groupValue": "eng", "targetType": "team", "targetId": team_id.to_string()},
    ]))
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(multi.as_array().unwrap().len(), 2);

    // Empty array clears; unknown provider is 404.
    let (status, cleared, _) = put(json!([])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared, json!([]));
    let (status, _, _) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/sso/providers/{}/mappings", Uuid::new_v4()),
        Some(json!([])),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_log WHERE activity_type = 'sso_mappings_updated' AND entity_id = $1",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        logged, 3,
        "valid replaces are logged, rejected ones are not"
    );

    delete_provider(&app, &id).await;
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();
}

// ------------------------------------------------------------------ settings

#[actix_web::test]
#[serial(sso_settings)]
async fn settings_round_trip() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    // Enforcing SSO needs a break-glass admin (enabled, manual, with a password).
    let break_glass = insert_user_row(&pool, "local", Some("hash")).await;
    sqlx::query("UPDATE users SET is_admin = TRUE, admin_source = 'manual' WHERE id = $1")
        .bind(break_glass)
        .execute(&pool)
        .await
        .unwrap();
    let (status, original, _) = call(
        &app,
        Method::GET,
        "/api/v1/sso/settings",
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let original = original["enforceSso"].as_bool().unwrap();

    for value in [!original, original] {
        let (status, put, text) = call(
            &app,
            Method::PUT,
            "/api/v1/sso/settings",
            Some(json!({"enforceSso": value})),
            Some(admin_user()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(put, json!({"enforceSso": value}));
        let (_, got, _) = call(
            &app,
            Method::GET,
            "/api/v1/sso/settings",
            None,
            Some(admin_user()),
        )
        .await;
        assert_eq!(got, json!({"enforceSso": value}));
    }
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_log WHERE activity_type = 'sso_settings_updated'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(logged >= 2);
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(break_glass)
        .execute(&pool)
        .await
        .unwrap();
}

// ------------------------------------------------------------------ public list

#[actix_web::test]
async fn public_provider_list_shows_only_enabled_providers_without_auth() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let on = create_provider(
        &app,
        &unique_slug("pub-on"),
        json!({"enabled": true, "displayName": "Visible IdP"}),
    )
    .await;
    let off = create_provider(&app, &unique_slug("pub-off"), json!({"enabled": false})).await;

    // No authenticated user at all.
    let (status, list, text) =
        call(&app, Method::GET, "/api/v1/auth/sso/providers", None, None).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let list = list.as_array().unwrap();
    let entry = list
        .iter()
        .find(|p| p["slug"] == on["slug"])
        .expect("enabled provider listed");
    assert_eq!(
        entry,
        &json!({"slug": on["slug"], "displayName": "Visible IdP"})
    );
    assert!(!list.iter().any(|p| p["slug"] == off["slug"]));
    for item in list {
        let keys: Vec<&String> = item.as_object().unwrap().keys().collect();
        assert_eq!(
            keys.len(),
            2,
            "only slug and displayName are public: {item}"
        );
    }

    delete_provider(&app, on["id"].as_str().unwrap()).await;
    delete_provider(&app, off["id"].as_str().unwrap()).await;
}

// ------------------------------------------------------------------ connection test

/// Serves canned responses per path on a loopback port until the test ends.
async fn spawn_idp(routes: impl FnOnce(&str) -> HashMap<String, (u16, String)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes = routes(&base);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut read = 0;
                loop {
                    let Ok(n) = stream.read(&mut buf[read..]).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    read += n;
                    if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let (status, body) = routes
                    .get(&path)
                    .cloned()
                    .unwrap_or((404, "not found".to_string()));
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    base
}

async fn run_test_endpoint<S, B>(app: &S, issuer_url: &str) -> Value
where
    S: Service<actix_http::Request, Response = ServiceResponse<B>, Error = actix_web::Error>,
    B: MessageBody,
{
    let created = create_provider(
        app,
        &unique_slug("probe"),
        json!({"issuerUrl": issuer_url, "clientSecret": SECRET}),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_string();
    let (status, result, text) = call(
        app,
        Method::POST,
        &format!("/api/v1/sso/providers/{id}/test"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(!text.contains(SECRET));
    delete_provider(app, &id).await;
    result
}

fn jwks() -> String {
    json!({"keys": [{"kty": "RSA", "kid": "k1", "n": "AQAB", "e": "AQAB"}]}).to_string()
}

#[actix_web::test]
async fn connection_test_reports_ok_issuer_mismatch_and_unreachable() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;

    // Healthy IdP.
    let base = spawn_idp(|base| {
        HashMap::from([
            (
                "/.well-known/openid-configuration".to_string(),
                (
                    200,
                    json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": format!("{base}/token"),
                        "jwks_uri": format!("{base}/jwks"),
                    })
                    .to_string(),
                ),
            ),
            ("/jwks".to_string(), (200, jwks())),
        ])
    })
    .await;
    let ok = run_test_endpoint(&app, &base).await;
    assert_eq!(ok["ok"], true, "{ok}");
    assert_eq!(ok["issuer"], json!(base));
    assert_eq!(
        ok["authorizationEndpoint"],
        json!(format!("{base}/authorize"))
    );
    assert!(ok["error"].is_null());

    // Trailing slash on the configured issuer is tolerated.
    let ok = run_test_endpoint(&app, &format!("{base}/")).await;
    assert_eq!(ok["ok"], true, "{ok}");

    // Discovery names a different issuer.
    let base = spawn_idp(|base| {
        HashMap::from([
            (
                "/.well-known/openid-configuration".to_string(),
                (
                    200,
                    json!({
                        "issuer": "https://evil.example.com",
                        "authorization_endpoint": format!("{base}/authorize"),
                        "jwks_uri": format!("{base}/jwks"),
                    })
                    .to_string(),
                ),
            ),
            ("/jwks".to_string(), (200, jwks())),
        ])
    })
    .await;
    let mismatch = run_test_endpoint(&app, &base).await;
    assert_eq!(mismatch["ok"], false);
    assert_eq!(mismatch["issuer"], "https://evil.example.com");
    assert!(
        mismatch["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("issuer mismatch"),
        "{mismatch}"
    );

    // JWKS endpoint fails.
    let base = spawn_idp(|base| {
        HashMap::from([
            (
                "/.well-known/openid-configuration".to_string(),
                (
                    200,
                    json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/authorize"),
                        "jwks_uri": format!("{base}/jwks"),
                    })
                    .to_string(),
                ),
            ),
            ("/jwks".to_string(), (500, "boom".to_string())),
        ])
    })
    .await;
    let broken = run_test_endpoint(&app, &base).await;
    assert_eq!(broken["ok"], false);
    assert!(
        broken["error"].as_str().unwrap().contains("JWKS"),
        "{broken}"
    );

    // Discovery is not JSON, and discovery is a 404.
    let base = spawn_idp(|_| {
        HashMap::from([(
            "/.well-known/openid-configuration".to_string(),
            (200, "<html>nope</html>".to_string()),
        )])
    })
    .await;
    let bad_json = run_test_endpoint(&app, &base).await;
    assert_eq!(bad_json["ok"], false);
    assert!(bad_json["error"].is_string());
    let base = spawn_idp(|_| HashMap::new()).await;
    let not_found = run_test_endpoint(&app, &base).await;
    assert_eq!(not_found["ok"], false);
    assert!(not_found["error"].as_str().unwrap().contains("404"));

    // Nothing listening: 200 with an error, never a 500.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let unreachable = run_test_endpoint(&app, &closed).await;
    assert_eq!(unreachable["ok"], false);
    assert!(unreachable["issuer"].is_null());
    assert!(unreachable["authorizationEndpoint"].is_null());
    assert!(unreachable["error"].is_string());
}

// ------------------------------------------------------------------ users API

async fn insert_user_row(pool: &PgPool, auth_source: &str, password: Option<&str>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email, auth_source)
         VALUES ($1, $2, $3, 'Api', 'User', $4, $5)",
    )
    .bind(id)
    .bind(format!("sso-api-{id}"))
    .bind(password)
    .bind(format!("sso-api-{id}@example.com"))
    .bind(auth_source)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[actix_web::test]
async fn users_api_exposes_auth_source_sso_assignments_and_identities() {
    let pool = init_pg_pool().await;
    let app = build_app(&pool, secrets_with_key()).await;
    let provider = create_provider(&app, &unique_slug("usr"), json!({})).await;
    let provider_id = provider["id"].as_str().unwrap().to_string();

    let sso_user = insert_user_row(&pool, "sso", None).await;
    let local_user = insert_user_row(&pool, "local", Some("x")).await;
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sso user test')")
        .bind(team_id)
        .bind(format!("sso-usr-{team_id}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_teams (user_id, team_id, source) VALUES ($1, $2, 'sso')")
        .bind(sso_user)
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();
    let roles = role_repository_tx(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    roles
        .add_sso_user_roles_tx(
            &mut tx,
            sso_user,
            vec![Uuid::parse_str(APPROVER_ROLE_ID).unwrap()],
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    sqlx::query(
        "INSERT INTO user_identities (id, user_id, provider_id, subject, email, last_login)
         VALUES ($1, $2, $3::uuid, 'subject-1', 'idp@example.com', now())",
    )
    .bind(Uuid::new_v4())
    .bind(sso_user)
    .bind(&provider_id)
    .execute(&pool)
    .await
    .unwrap();

    // Single user.
    let (status, user, text) = call(
        &app,
        Method::GET,
        &format!("/api/v1/users/{sso_user}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(user["authSource"], "sso");
    assert_eq!(user["ssoManagedRoleIds"], json!([APPROVER_ROLE_ID]));
    assert_eq!(user["ssoManagedTeamIds"], json!([team_id.to_string()]));
    let identities = user["identities"].as_array().unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0]["providerSlug"], provider["slug"]);
    assert_eq!(identities[0]["subject"], "subject-1");
    assert_eq!(identities[0]["email"], "idp@example.com");
    assert!(identities[0]["lastLogin"].is_string());

    let (_, local, _) = call(
        &app,
        Method::GET,
        &format!("/api/v1/users/{local_user}"),
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(local["authSource"], "local");
    assert_eq!(local["ssoManagedRoleIds"], json!([]));
    assert_eq!(local["identities"], json!([]));

    // List endpoint batches the details.
    let (status, list, text) = call(
        &app,
        Method::GET,
        "/api/v1/users?limit=100",
        None,
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let items = list["items"].as_array().unwrap();
    if let Some(found) = items
        .iter()
        .find(|u| u["id"] == json!(sso_user.to_string()))
    {
        assert_eq!(found["authSource"], "sso");
        assert_eq!(found["identities"].as_array().unwrap().len(), 1);
    }

    // PATCH keeps the new fields in its response.
    let (status, patched, text) = call(
        &app,
        Method::PATCH,
        &format!("/api/v1/users/{sso_user}"),
        Some(json!({"firstName": "Renamed"})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(patched["authSource"], "sso");
    assert_eq!(patched["ssoManagedRoleIds"], json!([APPROVER_ROLE_ID]));

    // Temporary password reset is refused for SSO users and still works for local ones.
    let (status, json, text) = call(
        &app,
        Method::POST,
        &format!("/api/v1/auth/users/{sso_user}/temporary-password"),
        Some(json!({"temporaryPassword": "Temp-pass-123!"})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert_eq!(json["error"], "sso_user_no_local_password");
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(sso_user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(hash.is_none(), "no password was written for the SSO user");
    let (status, _, text) = call(
        &app,
        Method::POST,
        &format!("/api/v1/auth/users/{local_user}/temporary-password"),
        Some(json!({"temporaryPassword": "Temp-pass-123!"})),
        Some(admin_user()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");

    delete_provider(&app, &provider_id).await;
    for id in [sso_user, local_user] {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();
}
