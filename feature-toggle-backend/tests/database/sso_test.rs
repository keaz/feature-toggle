//! SSO schema and repositories: providers, identities, group mappings, login
//! states and codes, settings, and the manual-versus-SSO assignment sources.

use chrono::{Duration, Utc};
use feature_toggle_backend::Error;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::role::{
    RoleRepositoryTx, role_repository, role_repository_tx,
};
use feature_toggle_backend::database::sso_group_mapping::{
    NewSsoGroupMapping, SsoGroupMappingRepositoryTx, sso_group_mapping_repository,
    sso_group_mapping_repository_tx,
};
use feature_toggle_backend::database::sso_login_code::{
    NewSsoLoginCode, SsoLoginCodeRepositoryTx, sso_login_code_repository,
    sso_login_code_repository_tx,
};
use feature_toggle_backend::database::sso_login_state::{
    NewSsoLoginState, SsoLoginStateRepositoryTx, sso_login_state_repository,
    sso_login_state_repository_tx,
};
use feature_toggle_backend::database::sso_provider::{
    CreateSsoProvider, SsoProviderRepositoryTx, UpdateSsoProvider, sso_provider_repository,
    sso_provider_repository_tx,
};
use feature_toggle_backend::database::sso_settings::{
    SsoSettingsRepositoryTx, sso_settings_repository, sso_settings_repository_tx,
};
use feature_toggle_backend::database::system_client::{
    CreateSystemClient, system_client_repository,
};
use feature_toggle_backend::database::user::{
    CreateUser, UserRepositoryTx, user_repository, user_repository_tx,
};
use feature_toggle_backend::database::user_identity::{
    CreateUserIdentity, UserIdentityRepositoryTx, user_identity_repository,
    user_identity_repository_tx,
};
use feature_toggle_backend::logic::user::user_logic;
use sqlx::PgPool;
use uuid::Uuid;

const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";
const REQUESTER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000002";
const TEAM_ADMIN_ROLE_ID: &str = "00000000-0000-0000-0000-000000000003";
const MIGRATION: &str =
    include_str!("../../migrations/20261002010000_sso_user_assignment_sources.sql");

fn role(id: &str) -> Uuid {
    Uuid::parse_str(id).unwrap()
}

fn new_provider(slug: &str) -> CreateSsoProvider {
    CreateSsoProvider {
        slug: slug.to_string(),
        display_name: "Test IdP".to_string(),
        issuer_url: "https://idp.example.com".to_string(),
        client_id: "client-id".to_string(),
        client_secret_enc: Some("encrypted-secret".to_string()),
        scopes: vec!["openid".into(), "email".into()],
        groups_claim: "groups".to_string(),
        allowed_email_domains: vec!["example.com".into()],
        jit_provisioning: true,
        allow_email_linking: false,
        role_sync_mode: "authoritative".to_string(),
        enabled: true,
    }
}

fn unique_slug(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn create_provider(
    pool: &PgPool,
    prefix: &str,
) -> feature_toggle_backend::database::entity::SsoProvider {
    let repo = sso_provider_repository_tx(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    let provider = repo
        .create_provider_tx(&mut tx, new_provider(&unique_slug(prefix)))
        .await
        .expect("create provider");
    tx.commit().await.unwrap();
    provider
}

async fn delete_provider(pool: &PgPool, id: Uuid) {
    let repo = sso_provider_repository_tx(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    repo.delete_provider_tx(&mut tx, id)
        .await
        .expect("delete provider");
    tx.commit().await.unwrap();
}

async fn create_user(pool: &PgPool, password_hash: Option<&str>, auth_source: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email, auth_source)
         VALUES ($1, $2, $3, 'Sso', 'Test', $4, $5)",
    )
    .bind(id)
    .bind(format!("sso-test-{id}"))
    .bind(password_hash)
    .bind(format!("sso-test-{id}@example.com"))
    .bind(auth_source)
    .execute(pool)
    .await
    .expect("insert user");
    id
}

async fn delete_user(pool: &PgPool, id: Uuid) {
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("delete user");
}

async fn create_team(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sso test')")
        .bind(id)
        .bind(format!("sso-team-{id}"))
        .execute(pool)
        .await
        .expect("insert team");
    id
}

async fn delete_team(pool: &PgPool, id: Uuid) {
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("delete team");
}

async fn role_sources(pool: &PgPool, user_id: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT role_id, source FROM user_roles WHERE user_id = $1 ORDER BY role_id")
        .bind(user_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn team_sources(pool: &PgPool, user_id: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT team_id, source FROM user_teams WHERE user_id = $1 ORDER BY team_id")
        .bind(user_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------- migration

#[tokio::test]
async fn test_migration_creates_tables_columns_and_seed_row() {
    let pool = init_pg_pool().await;

    for table in [
        "sso_providers",
        "user_identities",
        "sso_group_mappings",
        "sso_login_states",
        "sso_login_codes",
        "sso_settings",
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1)",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(exists, "table {table} must exist");
    }

    let settings_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sso_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(settings_rows, 1, "sso_settings holds exactly one row");
    let second = sqlx::query("INSERT INTO sso_settings (id, enforce_sso) VALUES (FALSE, TRUE)")
        .execute(&pool)
        .await;
    assert!(second.is_err(), "a second settings row must be rejected");

    let nullable: String = sqlx::query_scalar(
        "SELECT is_nullable FROM information_schema.columns
         WHERE table_name = 'users' AND column_name = 'password_hash'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nullable, "YES");
}

#[tokio::test]
async fn test_provider_defaults_and_constraints() {
    let pool = init_pg_pool().await;
    let slug = unique_slug("defaults");
    let row: (
        Vec<String>,
        String,
        Vec<String>,
        bool,
        bool,
        String,
        bool,
        Option<String>,
    ) = sqlx::query_as(
        "INSERT INTO sso_providers (id, slug, display_name, issuer_url, client_id)
             VALUES ($1, $2, 'D', 'https://i', 'c')
             RETURNING scopes, groups_claim, allowed_email_domains, jit_provisioning,
                       allow_email_linking, role_sync_mode, enabled, client_secret_enc",
    )
    .bind(Uuid::new_v4())
    .bind(&slug)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, vec!["openid", "email", "profile"]);
    assert_eq!(row.1, "groups");
    assert!(row.2.is_empty());
    assert!(row.3, "jit_provisioning defaults to true");
    assert!(!row.4, "allow_email_linking defaults to false");
    assert_eq!(row.5, "authoritative");
    assert!(!row.6, "enabled defaults to false");
    assert!(row.7.is_none());

    let bad_mode = sqlx::query(
        "INSERT INTO sso_providers (id, slug, display_name, issuer_url, client_id, role_sync_mode)
         VALUES ($1, $2, 'D', 'https://i', 'c', 'bogus')",
    )
    .bind(Uuid::new_v4())
    .bind(unique_slug("badmode"))
    .execute(&pool)
    .await;
    assert!(bad_mode.is_err(), "role_sync_mode must be restricted");

    sqlx::query("DELETE FROM sso_providers WHERE slug = $1")
        .bind(&slug)
        .execute(&pool)
        .await
        .unwrap();
}

/// The backfill statement of the migration marks existing system client shadow
/// users as 'system' and leaves every other user 'local'.
#[tokio::test]
async fn test_migration_backfills_system_auth_source_for_shadow_users_only() {
    let pool = init_pg_pool().await;
    let team_id = create_team(&pool).await;
    let shadow_id = create_user(&pool, Some("x"), "local").await;
    let human_id = create_user(&pool, Some("x"), "local").await;
    sqlx::query(
        "INSERT INTO system_clients (id, team_id, name, enabled, expires_at)
         VALUES ($1, $2, $3, TRUE, NOW() + INTERVAL '1 day')",
    )
    .bind(shadow_id)
    .bind(team_id)
    .bind(format!("sso-mig-{shadow_id}"))
    .execute(&pool)
    .await
    .unwrap();

    let backfill = MIGRATION
        .lines()
        .find(|l| l.starts_with("UPDATE users SET auth_source"))
        .expect("backfill statement in migration");
    sqlx::query(backfill).execute(&pool).await.unwrap();

    let source = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT auth_source FROM users WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(source(shadow_id).await, "system");
    assert_eq!(source(human_id).await, "local");

    delete_team(&pool, team_id).await;
    delete_user(&pool, shadow_id).await;
    delete_user(&pool, human_id).await;
}

#[tokio::test]
async fn test_new_system_client_shadow_user_has_system_auth_source() {
    let pool = init_pg_pool().await;
    let team_id = create_team(&pool).await;
    let client = system_client_repository(pool.clone())
        .create_system_client(
            team_id,
            CreateSystemClient {
                name: format!("sso-sc-{}", Uuid::new_v4().simple()),
                description: None,
                enabled: true,
                expires_at: Utc::now() + Duration::days(1),
            },
        )
        .await
        .expect("create system client");

    let shadow = user_repository(pool.clone())
        .get_user_by_id(client.id)
        .await
        .expect("shadow user");
    assert_eq!(shadow.auth_source, "system");

    delete_team(&pool, team_id).await;
    delete_user(&pool, client.id).await;
}

// ------------------------------------------------------------- providers

#[tokio::test]
async fn test_provider_create_get_find_list() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "crud").await;
    let repo = sso_provider_repository(pool.clone());

    let by_id = repo.get_provider_by_id(provider.id).await.unwrap();
    assert_eq!(by_id.slug, provider.slug);
    assert_eq!(by_id.client_secret_enc.as_deref(), Some("encrypted-secret"));
    assert_eq!(by_id.scopes, vec!["openid", "email"]);
    assert_eq!(by_id.allowed_email_domains, vec!["example.com"]);

    let by_slug = repo.find_provider_by_slug(&provider.slug).await.unwrap();
    assert_eq!(by_slug.map(|p| p.id), Some(provider.id));
    assert!(
        repo.find_provider_by_slug("no-such-slug")
            .await
            .unwrap()
            .is_none()
    );

    assert!(
        repo.list_providers()
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == provider.id)
    );
    assert!(
        repo.list_enabled_providers()
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == provider.id)
    );

    delete_provider(&pool, provider.id).await;
    assert!(matches!(
        repo.get_provider_by_id(provider.id).await,
        Err(Error::NotFound(id)) if id == provider.id
    ));
}

#[tokio::test]
async fn test_provider_duplicate_slug_is_rejected() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "dup").await;
    let repo = sso_provider_repository_tx(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    let err = repo
        .create_provider_tx(&mut tx, new_provider(&provider.slug))
        .await
        .expect_err("duplicate slug must fail");
    assert!(
        matches!(err, Error::RecordAlreadyExists(ref f) if f == "slug"),
        "{err:?}"
    );
    drop(tx);

    delete_provider(&pool, provider.id).await;
}

#[tokio::test]
async fn test_provider_partial_update_and_secret_semantics() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "upd").await;
    let repo = sso_provider_repository_tx(pool.clone());

    // Secret omitted = unchanged; other fields change.
    let mut tx = pool.begin().await.unwrap();
    let updated = repo
        .update_provider_tx(
            &mut tx,
            provider.id,
            UpdateSsoProvider {
                display_name: Some("Renamed".into()),
                enabled: Some(false),
                scopes: Some(vec!["openid".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(updated.display_name, "Renamed");
    assert!(!updated.enabled);
    assert_eq!(updated.scopes, vec!["openid"]);
    assert_eq!(
        updated.client_secret_enc.as_deref(),
        Some("encrypted-secret")
    );
    assert_eq!(updated.issuer_url, provider.issuer_url);
    assert!(updated.updated_at >= provider.updated_at);

    // Some(None) clears, Some(Some(v)) sets.
    let mut tx = pool.begin().await.unwrap();
    let cleared = repo
        .update_provider_tx(
            &mut tx,
            provider.id,
            UpdateSsoProvider {
                client_secret_enc: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(cleared.client_secret_enc.is_none());
    let set = repo
        .update_provider_tx(
            &mut tx,
            provider.id,
            UpdateSsoProvider {
                client_secret_enc: Some(Some("new-secret".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(set.client_secret_enc.as_deref(), Some("new-secret"));
    tx.commit().await.unwrap();

    // A disabled provider is not listed as enabled.
    assert!(
        !sso_provider_repository(pool.clone())
            .list_enabled_providers()
            .await
            .unwrap()
            .iter()
            .any(|p| p.id == provider.id)
    );

    let mut tx = pool.begin().await.unwrap();
    let missing = Uuid::new_v4();
    assert!(matches!(
        repo.update_provider_tx(&mut tx, missing, UpdateSsoProvider::default()).await,
        Err(Error::NotFound(id)) if id == missing
    ));
    drop(tx);

    delete_provider(&pool, provider.id).await;
}

#[tokio::test]
async fn test_provider_delete_cascades_but_keeps_users() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "cascade").await;
    let user_id = create_user(&pool, None, "sso").await;

    let mut tx = pool.begin().await.unwrap();
    user_identity_repository_tx(pool.clone())
        .create_identity_tx(
            &mut tx,
            CreateUserIdentity {
                user_id,
                provider_id: provider.id,
                subject: "sub-1".into(),
                email: Some("a@example.com".into()),
                last_login: None,
            },
        )
        .await
        .unwrap();
    sso_group_mapping_repository_tx(pool.clone())
        .replace_mappings_tx(
            &mut tx,
            provider.id,
            vec![NewSsoGroupMapping {
                group_value: "admins".into(),
                target_type: "admin".into(),
                target_id: None,
            }],
        )
        .await
        .unwrap();
    sso_login_state_repository_tx(pool.clone())
        .create_state_tx(
            &mut tx,
            NewSsoLoginState {
                state_hash: unique_slug("state"),
                provider_id: provider.id,
                nonce: "n".into(),
                pkce_verifier: "v".into(),
                redirect_path: None,
                expires_at: Utc::now() + Duration::minutes(10),
            },
        )
        .await
        .unwrap();
    sso_login_code_repository_tx(pool.clone())
        .create_code_tx(
            &mut tx,
            NewSsoLoginCode {
                code_hash: unique_slug("code"),
                user_id,
                provider_id: provider.id,
                expires_at: Utc::now() + Duration::seconds(60),
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    delete_provider(&pool, provider.id).await;

    for table in [
        "user_identities",
        "sso_group_mappings",
        "sso_login_states",
        "sso_login_codes",
    ] {
        let remaining: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE provider_id = $1"
        ))
        .bind(provider.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "{table} rows must cascade");
    }
    let user_remains: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(user_remains, "users survive provider deletion");

    delete_user(&pool, user_id).await;
}

// ------------------------------------------------------------ identities

#[tokio::test]
async fn test_identity_create_find_list_and_unique_subject() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "ident").await;
    let user_id = create_user(&pool, None, "sso").await;
    let repo = user_identity_repository_tx(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    let identity = repo
        .create_identity_tx(
            &mut tx,
            CreateUserIdentity {
                user_id,
                provider_id: provider.id,
                subject: "subject-1".into(),
                email: Some("first@example.com".into()),
                last_login: None,
            },
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let reader = user_identity_repository(pool.clone());
    let found = reader
        .find_identity(provider.id, "subject-1")
        .await
        .unwrap()
        .expect("identity found");
    assert_eq!(found.id, identity.id);
    assert_eq!(found.user_id, user_id);
    assert!(
        reader
            .find_identity(provider.id, "other")
            .await
            .unwrap()
            .is_none()
    );

    // Duplicate (provider, subject) is rejected, even for another user.
    let other_user = create_user(&pool, None, "sso").await;
    let mut tx = pool.begin().await.unwrap();
    let err = repo
        .create_identity_tx(
            &mut tx,
            CreateUserIdentity {
                user_id: other_user,
                provider_id: provider.id,
                subject: "subject-1".into(),
                email: None,
                last_login: None,
            },
        )
        .await
        .expect_err("duplicate subject must fail");
    assert!(matches!(err, Error::RecordAlreadyExists(_)), "{err:?}");
    drop(tx);

    let login_at = Utc::now();
    let mut tx = pool.begin().await.unwrap();
    repo.record_login_tx(
        &mut tx,
        identity.id,
        Some("new@example.com".into()),
        login_at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let listed = reader.list_identities_for_user(user_id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].provider_slug, provider.slug);
    assert_eq!(listed[0].subject, "subject-1");
    assert_eq!(listed[0].email.as_deref(), Some("new@example.com"));
    assert!(listed[0].last_login.is_some());

    let batch = reader
        .list_identities_for_users(vec![user_id, other_user])
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].user_id, user_id);

    // Deleting the user removes the identity.
    delete_user(&pool, user_id).await;
    delete_user(&pool, other_user).await;
    assert!(
        reader
            .find_identity(provider.id, "subject-1")
            .await
            .unwrap()
            .is_none()
    );
    delete_provider(&pool, provider.id).await;
}

// -------------------------------------------------------------- mappings

#[tokio::test]
async fn test_group_mappings_replace_dedupe_and_validate() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "map").await;
    let other = create_provider(&pool, "map-other").await;
    let repo = sso_group_mapping_repository_tx(pool.clone());
    let team_id = create_team(&pool).await;

    let mapping = |group: &str, kind: &str, target: Option<Uuid>| NewSsoGroupMapping {
        group_value: group.into(),
        target_type: kind.into(),
        target_id: target,
    };

    let mut tx = pool.begin().await.unwrap();
    // Mapping on another provider must survive a replace on this one.
    repo.replace_mappings_tx(
        &mut tx,
        other.id,
        vec![mapping("devs", "team", Some(team_id))],
    )
    .await
    .unwrap();
    let first = repo
        .replace_mappings_tx(
            &mut tx,
            provider.id,
            vec![
                mapping("admins", "admin", None),
                mapping("admins", "admin", None),
                mapping("devs", "role", Some(role(APPROVER_ROLE_ID))),
                mapping("devs", "team", Some(team_id)),
                mapping("devs", "team", Some(team_id)),
            ],
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        first.len(),
        3,
        "duplicates collapse, including NULL-target admin rows"
    );

    let reader = sso_group_mapping_repository(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    let second = repo
        .replace_mappings_tx(&mut tx, provider.id, vec![mapping("ops", "admin", None)])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].group_value, "ops");
    assert_eq!(second[0].target_id, None);
    assert_eq!(reader.list_mappings(provider.id).await.unwrap(), second);
    assert_eq!(reader.list_mappings(other.id).await.unwrap().len(), 1);

    // Empty replace clears.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repo.replace_mappings_tx(&mut tx, provider.id, vec![])
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();

    // admin with a target and role without one both violate the CHECK.
    for bad in [
        mapping("g", "admin", Some(team_id)),
        mapping("g", "role", None),
        mapping("g", "team", None),
    ] {
        let mut tx = pool.begin().await.unwrap();
        let err = repo
            .replace_mappings_tx(&mut tx, provider.id, vec![bad])
            .await
            .expect_err("inconsistent mapping must fail");
        assert!(matches!(err, Error::InvalidInput(_)), "{err:?}");
        drop(tx);
    }

    let mut tx = pool.begin().await.unwrap();
    let err = repo
        .replace_mappings_tx(&mut tx, provider.id, vec![mapping("g", "bogus", None)])
        .await;
    assert!(err.is_err(), "unknown target type must fail");
    drop(tx);

    delete_provider(&pool, provider.id).await;
    delete_provider(&pool, other.id).await;
    delete_team(&pool, team_id).await;
}

// ---------------------------------------------------------- login state

#[tokio::test]
async fn test_login_state_is_single_use_and_expires() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "state").await;
    let repo = sso_login_state_repository(pool.clone());
    let hash = unique_slug("state-hash");

    repo.create_state(NewSsoLoginState {
        state_hash: hash.clone(),
        provider_id: provider.id,
        nonce: "nonce".into(),
        pkce_verifier: "verifier".into(),
        redirect_path: Some("/features".into()),
        expires_at: Utc::now() + Duration::minutes(10),
    })
    .await
    .unwrap();

    let consumed = repo
        .consume_state(&hash)
        .await
        .unwrap()
        .expect("first consume");
    assert_eq!(consumed.nonce, "nonce");
    assert_eq!(consumed.pkce_verifier, "verifier");
    assert_eq!(consumed.redirect_path.as_deref(), Some("/features"));
    assert_eq!(consumed.provider_id, provider.id);
    assert!(
        repo.consume_state(&hash).await.unwrap().is_none(),
        "second consume fails"
    );
    assert!(repo.consume_state("unknown").await.unwrap().is_none());

    // Expired states cannot be consumed and are removed by cleanup.
    let expired_hash = unique_slug("expired-state");
    repo.create_state(NewSsoLoginState {
        state_hash: expired_hash.clone(),
        provider_id: provider.id,
        nonce: "n".into(),
        pkce_verifier: "v".into(),
        redirect_path: None,
        expires_at: Utc::now() - Duration::seconds(1),
    })
    .await
    .unwrap();
    assert!(repo.consume_state(&expired_hash).await.unwrap().is_none());
    assert!(repo.delete_expired_states().await.unwrap() >= 1);
    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sso_login_states WHERE state_hash = $1")
            .bind(&expired_hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0);

    // Duplicate state hash is rejected.
    let dup = unique_slug("dup-state");
    let input = NewSsoLoginState {
        state_hash: dup,
        provider_id: provider.id,
        nonce: "n".into(),
        pkce_verifier: "v".into(),
        redirect_path: None,
        expires_at: Utc::now() + Duration::minutes(1),
    };
    repo.create_state(input.clone()).await.unwrap();
    assert!(repo.create_state(input).await.is_err());

    delete_provider(&pool, provider.id).await;
}

#[tokio::test]
async fn test_login_state_consume_is_atomic_under_concurrency() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "state-race").await;
    let repo = sso_login_state_repository(pool.clone());
    let hash = unique_slug("race-state");
    repo.create_state(NewSsoLoginState {
        state_hash: hash.clone(),
        provider_id: provider.id,
        nonce: "n".into(),
        pkce_verifier: "v".into(),
        redirect_path: None,
        expires_at: Utc::now() + Duration::minutes(10),
    })
    .await
    .unwrap();

    let attempts = (0..8).map(|_| {
        let repo = repo.clone();
        let hash = hash.clone();
        tokio::spawn(async move { repo.consume_state(&hash).await.unwrap() })
    });
    let mut winners = 0;
    for handle in attempts {
        if handle.await.unwrap().is_some() {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one concurrent callback may consume the state"
    );

    delete_provider(&pool, provider.id).await;
}

// ----------------------------------------------------------- login code

#[tokio::test]
async fn test_login_code_is_single_use_and_expires() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "code").await;
    let user_id = create_user(&pool, None, "sso").await;
    let repo = sso_login_code_repository(pool.clone());
    let hash = unique_slug("code-hash");

    repo.create_code(NewSsoLoginCode {
        code_hash: hash.clone(),
        user_id,
        provider_id: provider.id,
        expires_at: Utc::now() + Duration::seconds(60),
    })
    .await
    .unwrap();

    let consumed = repo
        .consume_code(&hash)
        .await
        .unwrap()
        .expect("first consume");
    assert_eq!(consumed.user_id, user_id);
    assert_eq!(consumed.provider_id, provider.id);
    assert!(consumed.used_at.is_some());
    assert!(
        repo.consume_code(&hash).await.unwrap().is_none(),
        "second consume fails"
    );

    let expired = unique_slug("expired-code");
    repo.create_code(NewSsoLoginCode {
        code_hash: expired.clone(),
        user_id,
        provider_id: provider.id,
        expires_at: Utc::now() - Duration::seconds(1),
    })
    .await
    .unwrap();
    assert!(repo.consume_code(&expired).await.unwrap().is_none());

    // Cleanup removes only codes expired for over a day.
    let old = unique_slug("old-code");
    repo.create_code(NewSsoLoginCode {
        code_hash: old.clone(),
        user_id,
        provider_id: provider.id,
        expires_at: Utc::now() - Duration::days(2),
    })
    .await
    .unwrap();
    assert!(repo.delete_expired_codes().await.unwrap() >= 1);
    let remaining: Vec<String> =
        sqlx::query_scalar("SELECT code_hash FROM sso_login_codes WHERE code_hash = ANY($1)")
            .bind(vec![hash.clone(), expired.clone(), old.clone()])
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(remaining.contains(&hash));
    assert!(remaining.contains(&expired));
    assert!(!remaining.contains(&old));

    delete_provider(&pool, provider.id).await;
    delete_user(&pool, user_id).await;
}

#[tokio::test]
async fn test_login_code_consume_is_atomic_under_concurrency() {
    let pool = init_pg_pool().await;
    let provider = create_provider(&pool, "code-race").await;
    let user_id = create_user(&pool, None, "sso").await;
    let repo = sso_login_code_repository(pool.clone());
    let hash = unique_slug("race-code");
    repo.create_code(NewSsoLoginCode {
        code_hash: hash.clone(),
        user_id,
        provider_id: provider.id,
        expires_at: Utc::now() + Duration::seconds(60),
    })
    .await
    .unwrap();

    let attempts = (0..8).map(|_| {
        let repo = repo.clone();
        let hash = hash.clone();
        tokio::spawn(async move { repo.consume_code(&hash).await.unwrap() })
    });
    let mut winners = 0;
    for handle in attempts {
        if handle.await.unwrap().is_some() {
            winners += 1;
        }
    }
    assert_eq!(
        winners, 1,
        "exactly one concurrent exchange may redeem the code"
    );

    delete_provider(&pool, provider.id).await;
    delete_user(&pool, user_id).await;
}

// -------------------------------------------------------------- settings

/// Runs inside a rolled-back transaction because the settings row is global.
#[tokio::test]
async fn test_settings_enforce_sso_round_trip() {
    let pool = init_pg_pool().await;
    let repo = sso_settings_repository_tx(pool.clone());

    // Seeded default is not enforced (committed state).
    assert!(
        !sso_settings_repository(pool.clone())
            .get_enforce_sso()
            .await
            .unwrap()
    );

    let mut tx = pool.begin().await.unwrap();
    assert!(repo.set_enforce_sso_tx(&mut tx, true).await.unwrap());
    assert!(repo.get_enforce_sso_tx(&mut tx).await.unwrap());
    assert!(!repo.set_enforce_sso_tx(&mut tx, false).await.unwrap());
    assert!(!repo.get_enforce_sso_tx(&mut tx).await.unwrap());
    tx.rollback().await.unwrap();

    let singleton: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sso_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(singleton, 1);
}

// -------------------------------------------- manual versus SSO sources

#[tokio::test]
async fn test_manual_role_replace_keeps_other_sso_roles_and_converts_selected() {
    let pool = init_pg_pool().await;
    let user_id = create_user(&pool, Some("x"), "local").await;
    let roles_tx = role_repository_tx(pool.clone());
    let roles = role_repository(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    roles_tx
        .add_sso_user_roles_tx(&mut tx, user_id, vec![role(TEAM_ADMIN_ROLE_ID)])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Manual replace (pool path): Approver only.
    roles
        .assign_user_roles(user_id, vec![role(APPROVER_ROLE_ID)], None)
        .await
        .unwrap();
    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![
            (role(APPROVER_ROLE_ID), "manual".to_string()),
            (role(TEAM_ADMIN_ROLE_ID), "sso".to_string()),
        ]
    );

    // Manual replace (tx path): Requester only; the Approver manual row goes, SSO row stays.
    let mut tx = pool.begin().await.unwrap();
    roles_tx
        .assign_user_roles_tx(&mut tx, user_id, vec![role(REQUESTER_ROLE_ID)], None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![
            (role(REQUESTER_ROLE_ID), "manual".to_string()),
            (role(TEAM_ADMIN_ROLE_ID), "sso".to_string()),
        ]
    );

    // Selecting the role already held through SSO converts that row to manual.
    roles
        .assign_user_roles(user_id, vec![role(TEAM_ADMIN_ROLE_ID)], None)
        .await
        .unwrap();
    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![(role(TEAM_ADMIN_ROLE_ID), "manual".to_string())]
    );
    assert!(roles.list_sso_role_ids(user_id).await.unwrap().is_empty());

    // Group sync grants it again, but the manual row wins and stays manual.
    let mut tx = pool.begin().await.unwrap();
    roles_tx
        .add_sso_user_roles_tx(&mut tx, user_id, vec![role(TEAM_ADMIN_ROLE_ID)])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![(role(TEAM_ADMIN_ROLE_ID), "manual".to_string())]
    );

    // Tx path converts too: re-grant via SSO after clearing, then select by hand.
    roles
        .remove_user_role(user_id, role(TEAM_ADMIN_ROLE_ID))
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    roles_tx
        .add_sso_user_roles_tx(&mut tx, user_id, vec![role(TEAM_ADMIN_ROLE_ID)])
        .await
        .unwrap();
    roles_tx
        .assign_user_roles_tx(&mut tx, user_id, vec![role(TEAM_ADMIN_ROLE_ID)], None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![(role(TEAM_ADMIN_ROLE_ID), "manual".to_string())]
    );

    // Back to an SSO-sourced row for the removal and effective-role checks.
    roles
        .remove_user_role(user_id, role(TEAM_ADMIN_ROLE_ID))
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    roles_tx
        .add_sso_user_roles_tx(&mut tx, user_id, vec![role(TEAM_ADMIN_ROLE_ID)])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Removing an SSO-sourced role by hand is refused and leaves the row alone.
    let err = roles
        .remove_user_role(user_id, role(TEAM_ADMIN_ROLE_ID))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SsoManaged));
    let mut tx = pool.begin().await.unwrap();
    let err = roles_tx
        .remove_user_role_tx(&mut tx, user_id, role(TEAM_ADMIN_ROLE_ID))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::SsoManaged));
    tx.rollback().await.unwrap();
    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![(role(TEAM_ADMIN_ROLE_ID), "sso".to_string())]
    );

    // The user's effective roles include SSO-sourced ones.
    let effective = roles.get_user_roles(user_id).await.unwrap();
    assert_eq!(effective.len(), 1);
    assert_eq!(
        roles.list_sso_role_ids(user_id).await.unwrap(),
        vec![role(TEAM_ADMIN_ROLE_ID)]
    );

    delete_user(&pool, user_id).await;
}

#[tokio::test]
async fn test_sso_role_add_remove_list_respects_source() {
    let pool = init_pg_pool().await;
    let user_id = create_user(&pool, Some("x"), "local").await;
    let roles_tx = role_repository_tx(pool.clone());
    let roles = role_repository(pool.clone());

    roles
        .assign_user_roles(user_id, vec![role(APPROVER_ROLE_ID)], None)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    // Approver is already manual: manual wins, stays manual. Requester is new: sso.
    roles_tx
        .add_sso_user_roles_tx(
            &mut tx,
            user_id,
            vec![role(APPROVER_ROLE_ID), role(REQUESTER_ROLE_ID)],
        )
        .await
        .unwrap();
    // Idempotent.
    roles_tx
        .add_sso_user_roles_tx(&mut tx, user_id, vec![role(REQUESTER_ROLE_ID)])
        .await
        .unwrap();
    assert_eq!(
        roles_tx
            .list_sso_role_ids_tx(&mut tx, user_id)
            .await
            .unwrap(),
        vec![role(REQUESTER_ROLE_ID)]
    );
    // Removal only touches sso rows: asking to remove the manual Approver is a no-op.
    roles_tx
        .remove_sso_user_roles_tx(
            &mut tx,
            user_id,
            vec![role(APPROVER_ROLE_ID), role(REQUESTER_ROLE_ID)],
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        role_sources(&pool, user_id).await,
        vec![(role(APPROVER_ROLE_ID), "manual".to_string())]
    );
    delete_user(&pool, user_id).await;
}

#[tokio::test]
async fn test_manual_team_replace_keeps_other_sso_teams_and_converts_selected() {
    let pool = init_pg_pool().await;
    let user_id = create_user(&pool, Some("x"), "local").await;
    let (t1, t2, t3) = (
        create_team(&pool).await,
        create_team(&pool).await,
        create_team(&pool).await,
    );
    let users = user_repository(pool.clone());
    let users_tx = user_repository_tx(pool.clone());

    let mut tx = pool.begin().await.unwrap();
    users_tx
        .add_sso_user_teams_tx(&mut tx, user_id, vec![t1])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    users.set_user_teams(user_id, vec![t2]).await.unwrap();
    let mut expected = vec![(t1, "sso".to_string()), (t2, "manual".to_string())];
    expected.sort();
    assert_eq!(team_sources(&pool, user_id).await, expected);

    // Tx path replaces the manual set again; sso stays.
    let mut tx = pool.begin().await.unwrap();
    users_tx
        .set_user_teams_tx(&mut tx, user_id, vec![t3])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut expected = vec![(t1, "sso".to_string()), (t3, "manual".to_string())];
    expected.sort();
    assert_eq!(team_sources(&pool, user_id).await, expected);

    // Selecting the team already held through SSO converts that row to manual
    // (duplicates in the request are tolerated).
    users.set_user_teams(user_id, vec![t1, t1]).await.unwrap();
    assert_eq!(
        team_sources(&pool, user_id).await,
        vec![(t1, "manual".to_string())]
    );
    assert!(users.list_sso_team_ids(user_id).await.unwrap().is_empty());

    // Re-grant through SSO after a clear, then a tx-path manual select converts again.
    users.set_user_teams(user_id, vec![t2]).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    users_tx
        .add_sso_user_teams_tx(&mut tx, user_id, vec![t1])
        .await
        .unwrap();
    users_tx
        .set_user_teams_tx(&mut tx, user_id, vec![t1])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        team_sources(&pool, user_id).await,
        vec![(t1, "manual".to_string())]
    );

    // Restore an SSO-sourced membership for the clear check below.
    users.set_user_teams(user_id, vec![]).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    users_tx
        .add_sso_user_teams_tx(&mut tx, user_id, vec![t1])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Empty manual set clears manual rows only.
    users.set_user_teams(user_id, vec![]).await.unwrap();
    assert_eq!(
        team_sources(&pool, user_id).await,
        vec![(t1, "sso".to_string())]
    );
    assert_eq!(users.list_sso_team_ids(user_id).await.unwrap(), vec![t1]);
    assert_eq!(users.get_user_teams(user_id).await.unwrap().len(), 1);

    delete_user(&pool, user_id).await;
    for t in [t1, t2, t3] {
        delete_team(&pool, t).await;
    }
}

#[tokio::test]
async fn test_sso_team_add_remove_list_respects_source() {
    let pool = init_pg_pool().await;
    let user_id = create_user(&pool, Some("x"), "local").await;
    let (t1, t2) = (create_team(&pool).await, create_team(&pool).await);
    let users = user_repository(pool.clone());
    let users_tx = user_repository_tx(pool.clone());

    users.set_user_teams(user_id, vec![t1]).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    // t1 is already manual: manual wins. t2 is new: sso.
    users_tx
        .add_sso_user_teams_tx(&mut tx, user_id, vec![t1, t2])
        .await
        .unwrap();
    users_tx
        .add_sso_user_teams_tx(&mut tx, user_id, vec![t2])
        .await
        .unwrap();
    assert_eq!(
        users_tx
            .list_sso_team_ids_tx(&mut tx, user_id)
            .await
            .unwrap(),
        vec![t2]
    );
    users_tx
        .remove_sso_user_teams_tx(&mut tx, user_id, vec![t1, t2])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        team_sources(&pool, user_id).await,
        vec![(t1, "manual".to_string())]
    );

    delete_user(&pool, user_id).await;
    delete_team(&pool, t1).await;
    delete_team(&pool, t2).await;
}

// ------------------------------------------------------- password login

/// An SSO-only account (NULL password_hash) fails password login with the same
/// error as a wrong password.
#[tokio::test]
async fn test_null_password_login_fails_with_invalid_credentials() {
    let pool = init_pg_pool().await;
    let sso_user = create_user(&pool, None, "sso").await;
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(sso_user)
        .fetch_one(&pool)
        .await
        .unwrap();
    let logic = user_logic(
        user_repository(pool.clone()),
        activity_log_repository(pool.clone()),
    );

    let stored = user_repository(pool.clone())
        .get_user_by_id(sso_user)
        .await
        .unwrap();
    assert!(stored.password_hash.is_none());
    assert_eq!(stored.auth_source, "sso");

    let null_err = logic
        .authenticate_user(username.clone(), "anything".to_string())
        .await
        .expect_err("login must fail");
    let empty_err = logic
        .authenticate_user(username, String::new())
        .await
        .expect_err("login must fail");
    for err in [null_err, empty_err] {
        match err {
            Error::Unauthorized(msg) => assert_eq!(msg, "Invalid username or password"),
            other => panic!("expected Unauthorized, got {other:?}"),
        }
    }

    // And a wrong password for an ordinary user yields the identical error.
    let local = user_repository(pool.clone())
        .create_user(CreateUser {
            username: format!("sso-local-{}", Uuid::new_v4()),
            password_hash: {
                use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
                argon2::Argon2::default()
                    .hash_password(b"right", &SaltString::generate(&mut OsRng))
                    .unwrap()
                    .to_string()
            },
            first_name: "L".into(),
            last_name: "U".into(),
            email: format!("sso-local-{}@example.com", Uuid::new_v4()),
            mobile_number: None,
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .unwrap();
    assert_eq!(local.auth_source, "local");
    let wrong = logic
        .authenticate_user(local.username.clone(), "wrong".to_string())
        .await
        .err()
        .unwrap();
    match wrong {
        Error::Unauthorized(msg) => assert_eq!(msg, "Invalid username or password"),
        other => panic!("expected Unauthorized, got {other:?}"),
    }

    delete_user(&pool, sso_user).await;
    delete_user(&pool, local.id).await;
}
