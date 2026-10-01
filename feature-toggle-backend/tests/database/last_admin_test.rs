//! Disabling or demoting the last enabled admin must be rejected, otherwise the
//! anonymous admin bootstrap endpoint would open again.
//!
//! The test database is shared with other tests and contains other admins, so
//! tests that need "the only admin" disable every other admin first. The
//! rollback-based tests do that inside a transaction that is never committed;
//! the concurrency test commits and restores. A process-wide lock keeps these
//! tests from interfering with each other.

use feature_toggle_backend::Error;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::user::{CreateUser, user_repository, user_repository_tx};
use feature_toggle_backend::logic::user::UpdateUserInput;
use feature_toggle_backend::logic::user_tx::update_user_in_tx;
use feature_toggle_backend::model::ID;
use sqlx::{PgConnection, PgPool};
use tokio::sync::Mutex;
use uuid::Uuid;

pub static ADMIN_STATE_LOCK: Mutex<()> = Mutex::const_new(());

async fn create_admin(pool: &PgPool) -> Uuid {
    let suffix = Uuid::new_v4();
    user_repository(pool.clone())
        .create_user(CreateUser {
            username: format!("last-admin-{suffix}"),
            password_hash: "x".into(),
            first_name: "Last".into(),
            last_name: "Admin".into(),
            email: format!("last-admin-{suffix}@example.com"),
            mobile_number: None,
            is_admin: true,
            is_temporary_password: false,
        })
        .await
        .expect("create admin")
        .id
}

/// Disables every enabled human admin except `keep` on `conn`. Returns the disabled ids.
async fn disable_other_admins(conn: &mut PgConnection, keep: &[Uuid]) -> Vec<Uuid> {
    sqlx::query_scalar(
        "UPDATE users SET enabled = FALSE \
         WHERE is_admin = TRUE AND enabled = TRUE AND id <> ALL($1) \
           AND id NOT IN (SELECT id FROM system_clients) \
         RETURNING id",
    )
    .bind(keep)
    .fetch_all(conn)
    .await
    .expect("disable other admins")
}

async fn update(
    pool: &PgPool,
    conn: &mut PgConnection,
    user_id: Uuid,
    is_admin: Option<bool>,
    enabled: Option<bool>,
) -> Result<(), Error> {
    update_user_in_tx(
        conn,
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
    .map(|_| ())
}

async fn is_enabled_admin(conn: &mut PgConnection, user_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT is_admin AND enabled FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(conn)
        .await
        .expect("read user")
}

#[tokio::test]
async fn disabling_the_last_enabled_admin_is_rejected() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let admin = create_admin(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    disable_other_admins(&mut tx, &[admin]).await;

    let err = update(&pool, &mut tx, admin, None, Some(false))
        .await
        .expect_err("last admin must not be disabled");
    assert!(matches!(err, Error::LastAdminRequired), "got {err:?}");
    assert!(is_enabled_admin(&mut tx, admin).await);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn demoting_the_last_enabled_admin_is_rejected() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let admin = create_admin(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    disable_other_admins(&mut tx, &[admin]).await;

    let err = update(&pool, &mut tx, admin, Some(false), None)
        .await
        .expect_err("last admin must not be demoted");
    assert!(matches!(err, Error::LastAdminRequired), "got {err:?}");
    assert!(is_enabled_admin(&mut tx, admin).await);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn disabling_one_of_two_admins_is_allowed_but_not_the_second() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let first = create_admin(&pool).await;
    let second = create_admin(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    disable_other_admins(&mut tx, &[first, second]).await;

    update(&pool, &mut tx, first, None, Some(false))
        .await
        .expect("one of two admins can be disabled");
    assert!(!is_enabled_admin(&mut tx, first).await);

    let err = update(&pool, &mut tx, second, Some(false), Some(false))
        .await
        .expect_err("the remaining admin must stay");
    assert!(matches!(err, Error::LastAdminRequired), "got {err:?}");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn non_admin_updates_never_trip_the_guard() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let admin = create_admin(&pool).await;
    let suffix = Uuid::new_v4();
    let plain = user_repository(pool.clone())
        .create_user(CreateUser {
            username: format!("last-admin-plain-{suffix}"),
            password_hash: "x".into(),
            first_name: "Plain".into(),
            last_name: "User".into(),
            email: format!("last-admin-plain-{suffix}@example.com"),
            mobile_number: None,
            is_admin: false,
            is_temporary_password: false,
        })
        .await
        .expect("create user")
        .id;

    let mut tx = pool.begin().await.unwrap();
    disable_other_admins(&mut tx, &[admin]).await;
    update(&pool, &mut tx, plain, None, Some(false))
        .await
        .expect("disabling a non-admin is allowed");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn concurrent_disable_of_the_last_two_admins_lets_at_most_one_succeed() {
    let _guard = ADMIN_STATE_LOCK.lock().await;
    let pool = init_pg_pool().await;
    let first = create_admin(&pool).await;
    let second = create_admin(&pool).await;

    // Commit "exactly two enabled admins", remembering what to restore.
    let mut conn = pool.acquire().await.unwrap();
    let disabled_others = disable_other_admins(&mut conn, &[first, second]).await;
    drop(conn);

    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let pool_a = pool.clone();
    let task_a = tokio::spawn(async move {
        let mut tx = pool_a.begin().await.unwrap();
        let result = update(&pool_a, &mut tx, first, None, Some(false)).await;
        // Hold the admin row locks while the other update starts.
        locked_tx.send(()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        tx.commit().await.unwrap();
        result
    });
    let pool_b = pool.clone();
    let task_b = tokio::spawn(async move {
        locked_rx.await.unwrap();
        let mut tx = pool_b.begin().await.unwrap();
        let result = update(&pool_b, &mut tx, second, None, Some(false)).await;
        match &result {
            Ok(_) => tx.commit().await.unwrap(),
            Err(_) => tx.rollback().await.unwrap(),
        }
        result
    });
    let result_a = task_a.await.unwrap();
    let result_b = task_b.await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE id = ANY($1) AND is_admin = TRUE AND enabled = TRUE",
    )
    .bind([first, second].as_slice())
    .fetch_one(&mut *conn)
    .await
    .unwrap();

    // Restore shared state before asserting.
    sqlx::query("UPDATE users SET enabled = TRUE WHERE id = ANY($1)")
        .bind(disabled_others.as_slice())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET is_admin = FALSE WHERE id = ANY($1)")
        .bind([first, second].as_slice())
        .execute(&mut *conn)
        .await
        .unwrap();

    let successes = [&result_a, &result_b].iter().filter(|r| r.is_ok()).count();
    assert_eq!(successes, 1, "a: {result_a:?}, b: {result_b:?}");
    assert_eq!(remaining, 1, "one enabled admin must remain");
    let failure = [result_a, result_b]
        .into_iter()
        .find_map(Result::err)
        .expect("one update fails");
    assert!(
        matches!(failure, Error::LastAdminRequired),
        "got {failure:?}"
    );
}
