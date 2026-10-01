use chrono::{Duration, Utc};
use feature_toggle_backend::Error;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::system_client::{
    CreateSystemClient, system_client_repository,
};
use feature_toggle_backend::database::user::{
    UpdateUser, UserRepositoryTx, user_repository, user_repository_tx,
};
use uuid::Uuid;

const MIGRATION: &str =
    include_str!("../../migrations/20261001020000_system_client_shadow_users_not_admin.sql");

async fn insert_team(pool: &sqlx::PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'sc admin test')")
        .bind(team_id)
        .bind(format!("sc-admin-{team_id}"))
        .execute(pool)
        .await
        .expect("insert team");
    team_id
}

/// New shadow users are not admins and keep exactly the Requester and Approver roles.
#[tokio::test]
async fn test_new_system_client_shadow_user_is_not_admin() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let client = system_client_repository(pool.clone())
        .create_system_client(
            team_id,
            CreateSystemClient {
                name: format!("sc-admin-{}", Uuid::new_v4().simple()),
                description: None,
                enabled: true,
                expires_at: Utc::now() + Duration::days(1),
            },
        )
        .await
        .expect("create system client");

    let is_admin: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(client.id)
        .fetch_one(&pool)
        .await
        .expect("load shadow user");
    let mut roles: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM user_roles ur JOIN roles r ON r.id = ur.role_id WHERE ur.user_id = $1",
    )
    .bind(client.id)
    .fetch_all(&pool)
    .await
    .expect("load roles");
    roles.sort();

    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("delete team");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(client.id)
        .execute(&pool)
        .await
        .expect("delete shadow user");

    assert!(!is_admin);
    assert_eq!(roles, vec!["Approver".to_string(), "Requester".to_string()]);
}

/// The migration clears the admin flag of existing shadow users and leaves real admins alone.
#[tokio::test]
async fn test_migration_clears_admin_flag_on_existing_shadow_users_only() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let shadow_id = Uuid::new_v4();
    let admin_id = Uuid::new_v4();

    sqlx::query(
        "INSERT INTO system_clients (id, team_id, name, enabled, expires_at)
         VALUES ($1, $2, $3, TRUE, NOW() + INTERVAL '1 day')",
    )
    .bind(shadow_id)
    .bind(team_id)
    .bind(format!("sc-mig-{shadow_id}"))
    .execute(&pool)
    .await
    .expect("insert system client");
    for (id, label) in [(shadow_id, "shadow"), (admin_id, "admin")] {
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin)
             VALUES ($1, $2, 'x', 'A', 'B', $3, TRUE)",
        )
        .bind(id)
        .bind(format!("sc_mig_{label}_{id}"))
        .bind(format!("sc_mig_{label}_{id}@example.com"))
        .execute(&pool)
        .await
        .expect("insert user");
    }

    // The migration is a plain UPDATE; run it the way sqlx does.
    sqlx::raw_sql(MIGRATION)
        .execute(&pool)
        .await
        .expect("run migration");

    let shadow_admin: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(shadow_id)
        .fetch_one(&pool)
        .await
        .expect("load shadow");
    let real_admin: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(admin_id)
        .fetch_one(&pool)
        .await
        .expect("load admin");

    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("delete team");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![shadow_id, admin_id])
        .execute(&pool)
        .await
        .expect("delete users");

    assert!(!shadow_admin);
    assert!(real_admin);
}

/// The users API must not be able to turn a shadow user into an admin again.
#[tokio::test]
async fn test_users_api_cannot_update_system_client_shadow_user() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let client = system_client_repository(pool.clone())
        .create_system_client(
            team_id,
            CreateSystemClient {
                name: format!("sc-admin-{}", Uuid::new_v4().simple()),
                description: None,
                enabled: true,
                expires_at: Utc::now() + Duration::days(1),
            },
        )
        .await
        .expect("create system client");

    let update = || UpdateUser {
        id: client.id,
        first_name: None,
        last_name: None,
        email: None,
        mobile_number: None,
        is_admin: Some(true),
        enabled: None,
    };

    let pool_result = user_repository(pool.clone()).update_user(update()).await;

    let mut tx = pool.begin().await.expect("begin");
    let tx_result = user_repository_tx(pool.clone())
        .update_user_tx(&mut tx, update())
        .await;
    tx.rollback().await.expect("rollback");

    let is_admin: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(client.id)
        .fetch_one(&pool)
        .await
        .expect("load shadow user");

    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("delete team");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(client.id)
        .execute(&pool)
        .await
        .expect("delete shadow user");

    assert!(matches!(pool_result, Err(Error::InvalidInput(_))));
    assert!(matches!(tx_result, Err(Error::InvalidInput(_))));
    assert!(!is_admin);
}
