use crate::Error;
use crate::database::handle_error;
use chrono::{DateTime, Utc};
use mockall::automock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationChannelConfig {
    pub channel: String,
    pub enabled: bool,
    pub provider: String,
    pub settings: serde_json::Value,
    pub updated_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationPreference {
    pub notification_type: String,
    pub enabled: bool,
    pub email_enabled: bool,
    pub sms_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationDelivery {
    pub id: Uuid,
    pub notification_type: String,
    pub channel: String,
    pub team_id: Option<Uuid>,
    pub recipient_user_id: Option<Uuid>,
    pub recipient_email: Option<String>,
    pub recipient_mobile: Option<String>,
    pub subject: String,
    pub message: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NotificationRecipient {
    pub id: Uuid,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub mobile_number: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpsertNotificationChannelConfigInput {
    pub channel: String,
    pub enabled: bool,
    pub provider: String,
    pub settings: serde_json::Value,
    pub updated_by: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpsertNotificationPreferenceInput {
    pub notification_type: String,
    pub enabled: bool,
    pub email_enabled: bool,
    pub sms_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct CreateNotificationDeliveryInput {
    pub notification_type: String,
    pub channel: String,
    pub team_id: Option<Uuid>,
    pub recipient_user_id: Option<Uuid>,
    pub recipient_email: Option<String>,
    pub recipient_mobile: Option<String>,
    pub subject: String,
    pub message: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub sent_at: Option<DateTime<Utc>>,
}

#[automock]
#[async_trait::async_trait]
pub trait NotificationRepository: Send + Sync {
    async fn list_channel_configs(&self) -> Result<Vec<NotificationChannelConfig>, Error>;
    async fn upsert_channel_config(
        &self,
        input: UpsertNotificationChannelConfigInput,
    ) -> Result<NotificationChannelConfig, Error>;

    async fn list_preferences(&self) -> Result<Vec<NotificationPreference>, Error>;
    async fn get_preference(
        &self,
        notification_type: &str,
    ) -> Result<Option<NotificationPreference>, Error>;
    async fn upsert_preference(
        &self,
        input: UpsertNotificationPreferenceInput,
    ) -> Result<NotificationPreference, Error>;

    async fn list_system_admin_recipients(&self) -> Result<Vec<NotificationRecipient>, Error>;
    async fn list_team_recipients_by_roles(
        &self,
        team_id: Uuid,
        role_names: Vec<String>,
    ) -> Result<Vec<NotificationRecipient>, Error>;

    async fn create_delivery(
        &self,
        input: CreateNotificationDeliveryInput,
    ) -> Result<NotificationDelivery, Error>;

    fn clone_box(&self) -> Box<dyn NotificationRepository>;
}

impl Clone for Box<dyn NotificationRepository> {
    fn clone(&self) -> Box<dyn NotificationRepository> {
        self.clone_box()
    }
}

pub fn notification_repository(pool: sqlx::PgPool) -> Box<dyn NotificationRepository> {
    Box::new(NotificationRepositoryImpl::new(pool))
}

#[derive(Clone)]
pub struct NotificationRepositoryImpl {
    pool: sqlx::PgPool,
}

impl NotificationRepositoryImpl {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl NotificationRepository for NotificationRepositoryImpl {
    async fn list_channel_configs(&self) -> Result<Vec<NotificationChannelConfig>, Error> {
        let result = sqlx::query_as::<_, NotificationChannelConfig>(
            r#"
            SELECT channel, enabled, provider, settings, updated_by, created_at, updated_at
            FROM notification_channel_configs
            ORDER BY channel
            "#,
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn upsert_channel_config(
        &self,
        input: UpsertNotificationChannelConfigInput,
    ) -> Result<NotificationChannelConfig, Error> {
        let result = sqlx::query_as::<_, NotificationChannelConfig>(
            r#"
            INSERT INTO notification_channel_configs (
                channel,
                enabled,
                provider,
                settings,
                updated_by,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, NOW())
            ON CONFLICT (channel)
            DO UPDATE SET
                enabled = EXCLUDED.enabled,
                provider = EXCLUDED.provider,
                settings = EXCLUDED.settings,
                updated_by = EXCLUDED.updated_by,
                updated_at = NOW()
            RETURNING channel, enabled, provider, settings, updated_by, created_at, updated_at
            "#,
        )
        .bind(input.channel)
        .bind(input.enabled)
        .bind(input.provider)
        .bind(input.settings)
        .bind(input.updated_by)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn list_preferences(&self) -> Result<Vec<NotificationPreference>, Error> {
        let result = sqlx::query_as::<_, NotificationPreference>(
            r#"
            SELECT notification_type, enabled, email_enabled, sms_enabled, created_at, updated_at
            FROM notification_preferences
            ORDER BY notification_type
            "#,
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn get_preference(
        &self,
        notification_type: &str,
    ) -> Result<Option<NotificationPreference>, Error> {
        let result = sqlx::query_as::<_, NotificationPreference>(
            r#"
            SELECT notification_type, enabled, email_enabled, sms_enabled, created_at, updated_at
            FROM notification_preferences
            WHERE notification_type = $1
            "#,
        )
        .bind(notification_type)
        .fetch_optional(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn upsert_preference(
        &self,
        input: UpsertNotificationPreferenceInput,
    ) -> Result<NotificationPreference, Error> {
        let result = sqlx::query_as::<_, NotificationPreference>(
            r#"
            INSERT INTO notification_preferences (
                notification_type,
                enabled,
                email_enabled,
                sms_enabled,
                updated_at
            ) VALUES ($1, $2, $3, $4, NOW())
            ON CONFLICT (notification_type)
            DO UPDATE SET
                enabled = EXCLUDED.enabled,
                email_enabled = EXCLUDED.email_enabled,
                sms_enabled = EXCLUDED.sms_enabled,
                updated_at = NOW()
            RETURNING notification_type, enabled, email_enabled, sms_enabled, created_at, updated_at
            "#,
        )
        .bind(input.notification_type)
        .bind(input.enabled)
        .bind(input.email_enabled)
        .bind(input.sms_enabled)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn list_system_admin_recipients(&self) -> Result<Vec<NotificationRecipient>, Error> {
        let result = sqlx::query_as::<_, NotificationRecipient>(
            r#"
            SELECT u.id, u.username, u.first_name, u.last_name, u.email, u.mobile_number
            FROM users u
            WHERE u.is_admin = TRUE
              AND u.enabled = TRUE
            ORDER BY u.username
            "#,
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn list_team_recipients_by_roles(
        &self,
        team_id: Uuid,
        role_names: Vec<String>,
    ) -> Result<Vec<NotificationRecipient>, Error> {
        if role_names.is_empty() {
            return Ok(Vec::new());
        }

        let result = sqlx::query_as::<_, NotificationRecipient>(
            r#"
            SELECT DISTINCT u.id, u.username, u.first_name, u.last_name, u.email, u.mobile_number
            FROM users u
            INNER JOIN user_teams ut ON ut.user_id = u.id
            INNER JOIN user_roles ur ON ur.user_id = u.id
            INNER JOIN roles r ON r.id = ur.role_id
            WHERE ut.team_id = $1
              AND r.name = ANY($2)
              AND u.enabled = TRUE
            ORDER BY u.username
            "#,
        )
        .bind(team_id)
        .bind(role_names)
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn create_delivery(
        &self,
        input: CreateNotificationDeliveryInput,
    ) -> Result<NotificationDelivery, Error> {
        let result = sqlx::query_as::<_, NotificationDelivery>(
            r#"
            INSERT INTO notification_deliveries (
                notification_type,
                channel,
                team_id,
                recipient_user_id,
                recipient_email,
                recipient_mobile,
                subject,
                message,
                status,
                failure_reason,
                metadata,
                sent_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING
                id,
                notification_type,
                channel,
                team_id,
                recipient_user_id,
                recipient_email,
                recipient_mobile,
                subject,
                message,
                status,
                failure_reason,
                metadata,
                created_at,
                sent_at
            "#,
        )
        .bind(input.notification_type)
        .bind(input.channel)
        .bind(input.team_id)
        .bind(input.recipient_user_id)
        .bind(input.recipient_email)
        .bind(input.recipient_mobile)
        .bind(input.subject)
        .bind(input.message)
        .bind(input.status)
        .bind(input.failure_reason)
        .bind(input.metadata)
        .bind(input.sent_at)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    fn clone_box(&self) -> Box<dyn NotificationRepository> {
        Box::new(self.clone())
    }
}
