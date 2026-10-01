//! SSO group sync: modes, manual rows, admin source, last-admin safety, activity.
//!
//! Every test runs inside a transaction that is rolled back, so provider, mappings
//! and users never leak. Admin tests share the process-wide admin lock with the
//! last-admin tests because they disable other admins inside their transaction.

use super::last_admin_test::ADMIN_STATE_LOCK;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::entity::SsoProvider;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::role::role_repository_tx;
use feature_toggle_backend::database::sso_group_mapping::{
    NewSsoGroupMapping, SsoGroupMappingRepositoryTx, sso_group_mapping_repository_tx,
};
use feature_toggle_backend::database::sso_provider::{
    CreateSsoProvider, SsoProviderRepositoryTx, sso_provider_repository_tx,
};
use feature_toggle_backend::database::user::{CreateUser, UserRepositoryTx, user_repository_tx};
use feature_toggle_backend::logic::sso_role_sync::{SyncRepos, sync_roles_from_claims};
use feature_toggle_backend::logic::user::UpdateUserInput;
use feature_toggle_backend::logic::user_tx::update_user_in_tx;
use feature_toggle_backend::model::ID;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

const ROLE_A: &str = "00000000-0000-0000-0000-000000000001";
const ROLE_B: &str = "00000000-0000-0000-0000-000000000002";
const ROLE_C: &str = "00000000-0000-0000-0000-000000000003";

fn id(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

async fn provider(conn: &mut PgConnection, pool: &PgPool, mode: &str) -> SsoProvider {
    sso_provider_repository_tx(pool.clone())
        .create_provider_tx(
            conn,
            CreateSsoProvider {
                slug: format!("sync-{}", &Uuid::new_v4().simple().to_string()[..12]),
                display_name: "Sync IdP".to_string(),
                issuer_url: "https://idp.example.com".to_string(),
                client_id: "client".to_string(),
                client_secret_enc: None,
                scopes: vec!["openid".into()],
                groups_claim: "groups".to_string(),
                allowed_email_domains: vec![],
                jit_provisioning: true,
                allow_email_linking: false,
                role_sync_mode: mode.to_string(),
                enabled: true,
            },
        )
        .await
        .expect("create provider")
}

async fn user(conn: &mut PgConnection, pool: &PgPool, is_admin: bool) -> Uuid {
    let suffix = Uuid::new_v4();
    user_repository_tx(pool.clone())
        .create_user_tx(
            conn,
            CreateUser {
                username: format!("sync-{suffix}"),
                password_hash: "x".into(),
                first_name: "Sync".into(),
                last_name: "User".into(),
                email: format!("sync-{suffix}@example.com"),
                mobile_number: None,
                is_admin,
                is_temporary_password: false,
            },
        )
        .await
        .expect("create user")
        .id
}

async fn team(conn: &mut PgConnection) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sync')")
        .bind(team_id)
        .bind(format!("sync-team-{team_id}"))
        .execute(conn)
        .await
        .unwrap();
    team_id
}

fn mapping(group: &str, kind: &str, target: Option<Uuid>) -> NewSsoGroupMapping {
    NewSsoGroupMapping {
        group_value: group.to_string(),
        target_type: kind.to_string(),
        target_id: target,
    }
}

async fn set_mappings(
    conn: &mut PgConnection,
    pool: &PgPool,
    provider: &SsoProvider,
    mappings: Vec<NewSsoGroupMapping>,
) {
    sso_group_mapping_repository_tx(pool.clone())
        .replace_mappings_tx(conn, provider.id, mappings)
        .await
        .expect("replace mappings");
}

async fn sync(
    conn: &mut PgConnection,
    pool: &PgPool,
    provider: &SsoProvider,
    user_id: Uuid,
    claims: Value,
    userinfo: Option<&Value>,
) {
    let users = user_repository_tx(pool.clone());
    let roles = role_repository_tx(pool.clone());
    let mappings = sso_group_mapping_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    sync_roles_from_claims(
        conn,
        SyncRepos {
            users: &users,
            roles: &roles,
            mappings: &mappings,
            activity: activity.as_ref(),
        },
        provider,
        user_id,
        &claims,
        userinfo,
    )
    .await
    .expect("sync");
}

async fn role_rows(conn: &mut PgConnection, user_id: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT role_id, source FROM user_roles WHERE user_id = $1 ORDER BY role_id")
        .bind(user_id)
        .fetch_all(conn)
        .await
        .unwrap()
}

async fn team_rows(conn: &mut PgConnection, user_id: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT team_id, source FROM user_teams WHERE user_id = $1 ORDER BY team_id")
        .bind(user_id)
        .fetch_all(conn)
        .await
        .unwrap()
}

async fn admin_state(conn: &mut PgConnection, user_id: Uuid) -> (bool, Option<String>) {
    sqlx::query_as("SELECT is_admin, admin_source FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(conn)
        .await
        .unwrap()
}

async fn activities(conn: &mut PgConnection, user_id: Uuid, kind: &str) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT metadata FROM activity_log WHERE activity_type = $1 AND entity_id = $2
         ORDER BY created_at",
    )
    .bind(kind)
    .bind(user_id.to_string())
    .fetch_all(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn additive_adds_and_never_removes() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "additive").await;
    let u = user(&mut tx, &pool, false).await;
    let t = team(&mut tx).await;
    set_mappings(
        &mut tx,
        &pool,
        &p,
        vec![
            mapping("devs", "role", Some(id(ROLE_A))),
            mapping("devs", "team", Some(t)),
            mapping("ops", "role", Some(id(ROLE_B))),
        ],
    )
    .await;

    sync(
        &mut tx,
        &pool,
        &p,
        u,
        json!({"groups": ["devs", "ops"]}),
        None,
    )
    .await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![(id(ROLE_A), "sso".into()), (id(ROLE_B), "sso".into())]
    );
    assert_eq!(team_rows(&mut tx, u).await, vec![(t, "sso".into())]);

    // Groups vanish: additive keeps everything.
    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    assert_eq!(role_rows(&mut tx, u).await.len(), 2);
    assert_eq!(team_rows(&mut tx, u).await.len(), 1);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn off_mode_changes_nothing_and_logs_nothing() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "off").await;
    let u = user(&mut tx, &pool, false).await;
    set_mappings(
        &mut tx,
        &pool,
        &p,
        vec![
            mapping("devs", "role", Some(id(ROLE_A))),
            mapping("devs", "admin", None),
        ],
    )
    .await;
    sync(&mut tx, &pool, &p, u, json!({"groups": ["devs"]}), None).await;
    assert!(role_rows(&mut tx, u).await.is_empty());
    assert_eq!(admin_state(&mut tx, u).await, (false, None));
    assert!(activities(&mut tx, u, "sso_role_sync").await.is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn authoritative_removes_sso_rows_but_keeps_manual_ones() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    let u = user(&mut tx, &pool, false).await;
    let (t_sso, t_manual) = (team(&mut tx).await, team(&mut tx).await);
    set_mappings(
        &mut tx,
        &pool,
        &p,
        vec![
            mapping("devs", "role", Some(id(ROLE_A))),
            mapping("devs", "team", Some(t_sso)),
            mapping("devs", "role", Some(id(ROLE_C))),
        ],
    )
    .await;
    // ROLE_B and t_manual are manual; ROLE_C is manual and also mapped (stays manual).
    sqlx::query("INSERT INTO user_roles (user_id, role_id, source) VALUES ($1, $2, 'manual'), ($1, $3, 'manual')")
        .bind(u)
        .bind(id(ROLE_B))
        .bind(id(ROLE_C))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_teams (user_id, team_id, source) VALUES ($1, $2, 'manual')")
        .bind(u)
        .bind(t_manual)
        .execute(&mut *tx)
        .await
        .unwrap();

    sync(&mut tx, &pool, &p, u, json!({"groups": ["devs"]}), None).await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![
            (id(ROLE_A), "sso".into()),
            (id(ROLE_B), "manual".into()),
            (id(ROLE_C), "manual".into())
        ]
    );
    let teams = team_rows(&mut tx, u).await;
    assert!(teams.contains(&(t_sso, "sso".into())));
    assert!(teams.contains(&(t_manual, "manual".into())));

    // Group removed at the IdP: only SSO rows go.
    sync(
        &mut tx,
        &pool,
        &p,
        u,
        json!({"groups": ["unrelated"]}),
        None,
    )
    .await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![(id(ROLE_B), "manual".into()), (id(ROLE_C), "manual".into())]
    );
    assert_eq!(
        team_rows(&mut tx, u).await,
        vec![(t_manual, "manual".into())]
    );

    // Missing claim = no groups, same result; mapping match is case sensitive.
    sync(&mut tx, &pool, &p, u, json!({"groups": ["DEVS"]}), None).await;
    assert_eq!(role_rows(&mut tx, u).await.len(), 2);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn nested_string_userinfo_and_overage_claims() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    sqlx::query("UPDATE sso_providers SET groups_claim = 'realm_access.roles' WHERE id = $1")
        .bind(p.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let p = SsoProvider {
        groups_claim: "realm_access.roles".to_string(),
        ..p
    };
    let u = user(&mut tx, &pool, false).await;
    set_mappings(
        &mut tx,
        &pool,
        &p,
        vec![mapping("kc-devs", "role", Some(id(ROLE_A)))],
    )
    .await;

    sync(
        &mut tx,
        &pool,
        &p,
        u,
        json!({"realm_access": {"roles": ["kc-devs"]}}),
        None,
    )
    .await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![(id(ROLE_A), "sso".into())]
    );

    // Overage: no groups, authoritative removes (and userinfo is not consulted).
    let userinfo = json!({"realm_access": {"roles": ["kc-devs"]}});
    sync(
        &mut tx,
        &pool,
        &p,
        u,
        json!({"_claim_names": {"groups": "src1"}}),
        Some(&userinfo),
    )
    .await;
    assert!(role_rows(&mut tx, u).await.is_empty());

    // Userinfo fallback when the id_token has no claim.
    sync(&mut tx, &pool, &p, u, json!({}), Some(&userinfo)).await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![(id(ROLE_A), "sso".into())]
    );

    // A string claim is a single group.
    let flat = SsoProvider {
        groups_claim: "groups".to_string(),
        ..p.clone()
    };
    sync(&mut tx, &pool, &flat, u, json!({"groups": "kc-devs"}), None).await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![(id(ROLE_A), "sso".into())]
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn mapping_to_a_missing_target_is_skipped() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    let u = user(&mut tx, &pool, false).await;
    set_mappings(
        &mut tx,
        &pool,
        &p,
        vec![
            mapping("devs", "role", Some(Uuid::new_v4())),
            mapping("devs", "team", Some(Uuid::new_v4())),
            mapping("devs", "role", Some(id(ROLE_A))),
        ],
    )
    .await;
    sync(&mut tx, &pool, &p, u, json!({"groups": ["devs"]}), None).await;
    assert_eq!(
        role_rows(&mut tx, u).await,
        vec![(id(ROLE_A), "sso".into())]
    );
    assert!(team_rows(&mut tx, u).await.is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn activity_log_records_the_diff_only_when_something_changed() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    let u = user(&mut tx, &pool, false).await;
    let t = team(&mut tx).await;
    set_mappings(
        &mut tx,
        &pool,
        &p,
        vec![
            mapping("devs", "role", Some(id(ROLE_A))),
            mapping("devs", "team", Some(t)),
        ],
    )
    .await;

    sync(&mut tx, &pool, &p, u, json!({"groups": ["devs"]}), None).await;
    sync(&mut tx, &pool, &p, u, json!({"groups": ["devs"]}), None).await;
    let logged = activities(&mut tx, u, "sso_role_sync").await;
    assert_eq!(logged.len(), 1, "unchanged sync logs nothing");
    assert_eq!(logged[0]["added_roles"], json!([ROLE_A]));
    assert_eq!(logged[0]["added_teams"], json!([t.to_string()]));
    assert_eq!(logged[0]["removed_roles"], json!([]));
    assert_eq!(logged[0]["removed_teams"], json!([]));
    assert!(logged[0]["admin_change"].is_null());

    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    let logged = activities(&mut tx, u, "sso_role_sync").await;
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[1]["removed_roles"], json!([ROLE_A]));
    assert_eq!(logged[1]["removed_teams"], json!([t.to_string()]));
    assert_eq!(logged[1]["added_roles"], json!([]));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn sso_admin_is_granted_then_revoked_in_authoritative_mode() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    let u = user(&mut tx, &pool, false).await;
    // Another admin exists so the revoke is allowed.
    let _other = user(&mut tx, &pool, true).await;
    set_mappings(&mut tx, &pool, &p, vec![mapping("admins", "admin", None)]).await;

    sync(&mut tx, &pool, &p, u, json!({"groups": ["admins"]}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("sso".into())));
    let logged = activities(&mut tx, u, "sso_role_sync").await;
    assert_eq!(logged[0]["admin_change"], json!("granted"));

    // Still in the group: nothing to log.
    sync(&mut tx, &pool, &p, u, json!({"groups": ["admins"]}), None).await;
    assert_eq!(activities(&mut tx, u, "sso_role_sync").await.len(), 1);

    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (false, None));
    let logged = activities(&mut tx, u, "sso_role_sync").await;
    assert_eq!(logged[1]["admin_change"], json!("revoked"));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn additive_mode_grants_admin_but_never_revokes() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "additive").await;
    let u = user(&mut tx, &pool, false).await;
    let _other = user(&mut tx, &pool, true).await;
    set_mappings(&mut tx, &pool, &p, vec![mapping("admins", "admin", None)]).await;
    sync(&mut tx, &pool, &p, u, json!({"groups": ["admins"]}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("sso".into())));
    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("sso".into())));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn manual_admin_is_never_revoked_or_taken_over() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    let u = user(&mut tx, &pool, true).await;
    let _other = user(&mut tx, &pool, true).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("manual".into())));
    set_mappings(&mut tx, &pool, &p, vec![mapping("admins", "admin", None)]).await;

    // Matching and non-matching groups both leave the manual grant alone.
    sync(&mut tx, &pool, &p, u, json!({"groups": ["admins"]}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("manual".into())));
    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("manual".into())));

    // An admin row with a NULL source (raw insert, seed) counts as manual.
    sqlx::query("UPDATE users SET admin_source = NULL WHERE id = $1")
        .bind(u)
        .execute(&mut *tx)
        .await
        .unwrap();
    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, None));
    assert!(activities(&mut tx, u, "sso_role_sync").await.is_empty());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn last_enabled_admin_keeps_sso_admin_and_logs_a_warning() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let p = provider(&mut tx, &pool, "authoritative").await;
    let u = user(&mut tx, &pool, false).await;
    set_mappings(&mut tx, &pool, &p, vec![mapping("admins", "admin", None)]).await;
    sync(&mut tx, &pool, &p, u, json!({"groups": ["admins"]}), None).await;
    // Make this the only enabled admin.
    sqlx::query(
        "UPDATE users SET enabled = FALSE WHERE is_admin = TRUE AND enabled = TRUE AND id <> $1
           AND id NOT IN (SELECT id FROM system_clients)",
    )
    .bind(u)
    .execute(&mut *tx)
    .await
    .unwrap();

    sync(&mut tx, &pool, &p, u, json!({"groups": []}), None).await;
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("sso".into())));
    let warnings = activities(&mut tx, u, "sso_role_sync_warning").await;
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["reason"], json!("last_admin"));
    // A kept admin is not a change.
    assert_eq!(activities(&mut tx, u, "sso_role_sync").await.len(), 1);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn manual_admin_paths_keep_admin_source_consistent() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let users = user_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let _other = user(&mut tx, &pool, true).await;

    let plain = user(&mut tx, &pool, false).await;
    assert_eq!(admin_state(&mut tx, plain).await, (false, None));

    let update = |is_admin| UpdateUserInput {
        first_name: None,
        last_name: None,
        email: None,
        mobile_number: None,
        is_admin,
        enabled: None,
    };
    // Manual grant, then manual revoke.
    update_user_in_tx(
        &mut tx,
        &users,
        activity.as_ref(),
        ID::from(plain),
        update(Some(true)),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        admin_state(&mut tx, plain).await,
        (true, Some("manual".into()))
    );
    update_user_in_tx(
        &mut tx,
        &users,
        activity.as_ref(),
        ID::from(plain),
        update(Some(false)),
        None,
    )
    .await
    .unwrap();
    assert_eq!(admin_state(&mut tx, plain).await, (false, None));

    // Unrelated edits and a repeated isAdmin=true keep an SSO grant SSO.
    users
        .set_admin_with_source_tx(&mut tx, plain, true, Some("sso"))
        .await
        .unwrap();
    update_user_in_tx(
        &mut tx,
        &users,
        activity.as_ref(),
        ID::from(plain),
        update(None),
        None,
    )
    .await
    .unwrap();
    update_user_in_tx(
        &mut tx,
        &users,
        activity.as_ref(),
        ID::from(plain),
        update(Some(true)),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        admin_state(&mut tx, plain).await,
        (true, Some("sso".into()))
    );
    // A manual revoke clears the source.
    update_user_in_tx(
        &mut tx,
        &users,
        activity.as_ref(),
        ID::from(plain),
        update(Some(false)),
        None,
    )
    .await
    .unwrap();
    assert_eq!(admin_state(&mut tx, plain).await, (false, None));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn migration_backfills_existing_admins_as_manual() {
    let pool = init_pg_pool().await;
    let mut tx = pool.begin().await.unwrap();
    let u = user(&mut tx, &pool, false).await;
    sqlx::query("UPDATE users SET is_admin = TRUE, admin_source = NULL WHERE id = $1")
        .bind(u)
        .execute(&mut *tx)
        .await
        .unwrap();
    // The backfill statement of the migration.
    sqlx::query("UPDATE users SET admin_source = 'manual' WHERE is_admin = TRUE")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(admin_state(&mut tx, u).await, (true, Some("manual".into())));
    let bad = sqlx::query("UPDATE users SET admin_source = 'other' WHERE id = $1")
        .bind(u)
        .execute(&mut *tx)
        .await;
    assert!(bad.is_err(), "admin_source is limited to manual and sso");
    tx.rollback().await.unwrap();
}
