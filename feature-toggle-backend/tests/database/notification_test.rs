use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::notification::{
    UpsertNotificationChannelConfigInput, UpsertNotificationPreferenceInput,
    notification_repository,
};
use feature_toggle_backend::logic::notification::{
    NOTIFICATION_TYPE_FEATURE_CREATED, NotificationEvent, notification_logic,
    spawn_notification_dispatch,
};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

const TEAM_ADMIN_ROLE_ID: &str = "00000000-0000-0000-0000-000000000003";

fn notification_test_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[tokio::test]
async fn dispatch_feature_created_creates_email_and_sms_deliveries_for_team_admin() {
    let _guard = notification_test_mutex().lock().await;

    let pool = init_pg_pool().await;

    let team_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let unique_suffix = Uuid::new_v4().simple().to_string();

    let team_name = format!("Notification Team {unique_suffix}");
    let username = format!("notif-admin-{unique_suffix}");
    let email = format!("notif-admin-{unique_suffix}@example.com");

    sqlx::query(
        r#"
        INSERT INTO teams (id, name, description)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(team_id)
    .bind(team_name)
    .bind("notification integration test team")
    .execute(&pool)
    .await
    .expect("failed to insert team");

    sqlx::query(
        r#"
        INSERT INTO users (
            id,
            username,
            password_hash,
            first_name,
            last_name,
            email,
            mobile_number,
            is_admin,
            enabled,
            is_temporary_password
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, TRUE, FALSE)
        "#,
    )
    .bind(user_id)
    .bind(username)
    .bind("hash")
    .bind("Notification")
    .bind("Admin")
    .bind(email.clone())
    .bind("+15551239876")
    .execute(&pool)
    .await
    .expect("failed to insert user");

    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("failed to assign team");

    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(Uuid::parse_str(TEAM_ADMIN_ROLE_ID).unwrap())
        .execute(&pool)
        .await
        .expect("failed to assign role");

    let repo = notification_repository(pool.clone());
    let logic = notification_logic(repo.clone_box());

    repo.upsert_channel_config(UpsertNotificationChannelConfigInput {
        channel: "email".to_string(),
        enabled: true,
        provider: "smtp".to_string(),
        settings: serde_json::json!({"fromEmail": "no-reply@fluxgate.dev"}),
        updated_by: None,
    })
    .await
    .expect("failed to configure email");

    repo.upsert_channel_config(UpsertNotificationChannelConfigInput {
        channel: "sms".to_string(),
        enabled: true,
        provider: "twilio".to_string(),
        settings: serde_json::json!({"sender": "FluxGate"}),
        updated_by: None,
    })
    .await
    .expect("failed to configure sms");

    repo.upsert_preference(UpsertNotificationPreferenceInput {
        notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
        enabled: true,
        email_enabled: true,
        sms_enabled: true,
    })
    .await
    .expect("failed to configure preference");

    logic
        .dispatch_event(NotificationEvent {
            notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
            team_id: Some(team_id),
            actor_id: None,
            subject: "Feature created: test-feature".to_string(),
            message: "Feature 'test-feature' was created.".to_string(),
            metadata: Some(serde_json::json!({"feature_key": "test-feature"})),
        })
        .await
        .expect("dispatch should succeed");

    let rows = sqlx::query(
        r#"
        SELECT channel, status, failure_reason, recipient_user_id
        FROM notification_deliveries
        WHERE notification_type = $1
          AND team_id = $2
          AND recipient_user_id = $3
        ORDER BY channel
        "#,
    )
    .bind(NOTIFICATION_TYPE_FEATURE_CREATED)
    .bind(team_id)
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .expect("failed to fetch deliveries");

    assert_eq!(rows.len(), 2);

    let statuses: HashMap<String, (String, Option<String>)> = rows
        .iter()
        .map(|row| {
            (
                row.get::<String, _>("channel"),
                (
                    row.get::<String, _>("status"),
                    row.get::<Option<String>, _>("failure_reason"),
                ),
            )
        })
        .collect();

    assert_eq!(statuses.len(), 2);
    assert_eq!(
        statuses.get("email"),
        Some(&(
            "failed".to_string(),
            Some("smtp_settings_missing_host".to_string())
        ))
    );
    assert_eq!(statuses.get("sms"), Some(&("queued".to_string(), None)));
}

#[tokio::test]
async fn explicit_async_notification_dispatch_creates_delivery_async() {
    let _guard = notification_test_mutex().lock().await;

    let pool = init_pg_pool().await;

    let team_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let unique_suffix = Uuid::new_v4().simple().to_string();
    let subject = format!("Async dispatch test {unique_suffix}");

    let team_name = format!("Async Notification Team {unique_suffix}");
    let username = format!("async-notif-admin-{unique_suffix}");
    let email = format!("async-notif-admin-{unique_suffix}@example.com");

    sqlx::query(
        r#"
        INSERT INTO teams (id, name, description)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(team_id)
    .bind(team_name)
    .bind("async notification integration test team")
    .execute(&pool)
    .await
    .expect("failed to insert team");

    sqlx::query(
        r#"
        INSERT INTO users (
            id,
            username,
            password_hash,
            first_name,
            last_name,
            email,
            mobile_number,
            is_admin,
            enabled,
            is_temporary_password
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, TRUE, FALSE)
        "#,
    )
    .bind(user_id)
    .bind(username)
    .bind("hash")
    .bind("Async")
    .bind("Admin")
    .bind(email)
    .bind("+15551230001")
    .execute(&pool)
    .await
    .expect("failed to insert user");

    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("failed to assign team");

    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(Uuid::parse_str(TEAM_ADMIN_ROLE_ID).unwrap())
        .execute(&pool)
        .await
        .expect("failed to assign role");

    let repo = notification_repository(pool.clone());
    let logic = notification_logic(repo.clone_box());

    repo.upsert_channel_config(UpsertNotificationChannelConfigInput {
        channel: "email".to_string(),
        enabled: false,
        provider: "smtp".to_string(),
        settings: serde_json::json!({}),
        updated_by: None,
    })
    .await
    .expect("failed to disable email");

    repo.upsert_channel_config(UpsertNotificationChannelConfigInput {
        channel: "sms".to_string(),
        enabled: true,
        provider: "twilio".to_string(),
        settings: serde_json::json!({"sender": "FluxGate"}),
        updated_by: None,
    })
    .await
    .expect("failed to configure sms");

    repo.upsert_preference(UpsertNotificationPreferenceInput {
        notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
        enabled: true,
        email_enabled: false,
        sms_enabled: true,
    })
    .await
    .expect("failed to configure preference");

    spawn_notification_dispatch(
        logic.clone_box(),
        NotificationEvent {
            notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
            team_id: Some(team_id),
            actor_id: None,
            subject: subject.clone(),
            message: "Async global dispatch message.".to_string(),
            metadata: Some(serde_json::json!({"kind": "async_global_dispatch"})),
        },
    );

    let start = tokio::time::Instant::now();
    loop {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)::BIGINT
            FROM notification_deliveries
            WHERE team_id = $1
              AND recipient_user_id = $2
              AND channel = 'sms'
              AND subject = $3
              AND status = 'queued'
            "#,
        )
        .bind(team_id)
        .bind(user_id)
        .bind(subject.as_str())
        .fetch_one(&pool)
        .await
        .expect("failed to count async deliveries");

        if count > 0 {
            break;
        }

        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for async notification delivery"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
