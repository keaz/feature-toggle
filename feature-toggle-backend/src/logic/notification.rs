use crate::Error;
use crate::database::notification::{
    CreateNotificationDeliveryInput, NotificationChannelConfig, NotificationPreference,
    NotificationRecipient, NotificationRepository, UpsertNotificationChannelConfigInput,
    UpsertNotificationPreferenceInput,
};
use chrono::Utc;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use mockall::automock;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

pub const TEAM_ADMIN_ROLE: &str = "Team Admin";
pub const APPROVER_ROLE: &str = "Approver";
pub const REQUESTER_ROLE: &str = "Requester";

pub const NOTIFICATION_TYPE_FEATURE_CREATED: &str = "feature_created";
pub const NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED: &str = "stage_change_requested";
pub const NOTIFICATION_TYPE_STAGE_CHANGE_APPROVED: &str = "stage_change_approved";
pub const NOTIFICATION_TYPE_FEATURE_DEPLOYED: &str = "feature_deployed";
pub const NOTIFICATION_TYPE_FEATURE_ROLLED_BACK: &str = "feature_rolled_back";
pub const NOTIFICATION_TYPE_TEAM_CREATED: &str = "team_created";
pub const NOTIFICATION_TYPE_USER_ADDED_TO_TEAM: &str = "user_added_to_team";
pub const NOTIFICATION_TYPE_KILL_SWITCH_ACTIVATED: &str = "kill_switch_activated";

#[derive(Debug, Clone)]
pub struct NotificationDefinition {
    pub notification_type: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub recipient_scope: &'static str,
    pub default_enabled: bool,
    pub default_email_enabled: bool,
    pub default_sms_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct NotificationChannelConfigView {
    pub channel: String,
    pub enabled: bool,
    pub provider: String,
    pub settings: serde_json::Value,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NotificationPreferenceView {
    pub notification_type: String,
    pub label: String,
    pub description: String,
    pub recipient_scope: String,
    pub enabled: bool,
    pub email_enabled: bool,
    pub sms_enabled: bool,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NotificationSettingsView {
    pub channels: Vec<NotificationChannelConfigView>,
    pub preferences: Vec<NotificationPreferenceView>,
}

#[derive(Debug, Clone)]
pub struct UpdateNotificationChannelConfigInput {
    pub channel: String,
    pub enabled: bool,
    pub provider: String,
    pub settings: serde_json::Value,
    pub actor_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UpdateNotificationPreferenceInput {
    pub notification_type: String,
    pub enabled: Option<bool>,
    pub email_enabled: Option<bool>,
    pub sms_enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct NotificationEvent {
    pub notification_type: String,
    pub team_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub recipient_user_ids: Option<Vec<Uuid>>,
    pub subject: String,
    pub message: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct EffectivePreference {
    enabled: bool,
    email_enabled: bool,
    sms_enabled: bool,
}

#[derive(Debug, Clone)]
struct SmtpGatewaySettings {
    host: String,
    port: u16,
    username: String,
    password: String,
    from_email: String,
    from_name: Option<String>,
    secure: bool,
    start_tls: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum RecipientSelector {
    TeamRoles,
    SystemAdmins,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum DeliveryChannel {
    Email,
    Sms,
}

impl DeliveryChannel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Sms => "sms",
        }
    }
}

fn definitions() -> &'static [NotificationDefinition] {
    &[
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_FEATURE_CREATED,
            label: "Feature Created",
            description: "When a feature is created",
            recipient_scope: "Team Admin",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED,
            label: "Deployment/Rollback Requested",
            description: "When deployment or rollback is requested",
            recipient_scope: "Team Admin, Approver",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_STAGE_CHANGE_APPROVED,
            label: "Deployment/Rollback Approved",
            description: "When deployment or rollback request is approved",
            recipient_scope: "Team Admin, Requester",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_FEATURE_DEPLOYED,
            label: "Feature Deployed",
            description: "When a feature is deployed",
            recipient_scope: "Team Admin, Approver, Requester",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_FEATURE_ROLLED_BACK,
            label: "Feature Rolled Back",
            description: "When a feature is rolled back",
            recipient_scope: "Team Admin, Approver, Requester",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_TEAM_CREATED,
            label: "Team Added",
            description: "When a new team is created",
            recipient_scope: "System Admin",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_USER_ADDED_TO_TEAM,
            label: "User Added To Team",
            description: "When a user is added to a team",
            recipient_scope: "Team Admin",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
        NotificationDefinition {
            notification_type: NOTIFICATION_TYPE_KILL_SWITCH_ACTIVATED,
            label: "Kill Switch Activated",
            description: "When a feature kill switch is activated",
            recipient_scope: "Team Admin",
            default_enabled: true,
            default_email_enabled: true,
            default_sms_enabled: false,
        },
    ]
}

fn definition_for(notification_type: &str) -> Option<&'static NotificationDefinition> {
    definitions()
        .iter()
        .find(|item| item.notification_type == notification_type)
}

fn selector_for(notification_type: &str) -> Option<RecipientSelector> {
    match notification_type {
        NOTIFICATION_TYPE_TEAM_CREATED => Some(RecipientSelector::SystemAdmins),
        NOTIFICATION_TYPE_FEATURE_CREATED
        | NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED
        | NOTIFICATION_TYPE_STAGE_CHANGE_APPROVED
        | NOTIFICATION_TYPE_FEATURE_DEPLOYED
        | NOTIFICATION_TYPE_FEATURE_ROLLED_BACK
        | NOTIFICATION_TYPE_USER_ADDED_TO_TEAM
        | NOTIFICATION_TYPE_KILL_SWITCH_ACTIVATED => Some(RecipientSelector::TeamRoles),
        _ => None,
    }
}

fn role_names_for(notification_type: &str) -> Vec<String> {
    match notification_type {
        NOTIFICATION_TYPE_FEATURE_CREATED => vec![TEAM_ADMIN_ROLE.to_string()],
        NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED => {
            vec![TEAM_ADMIN_ROLE.to_string(), APPROVER_ROLE.to_string()]
        }
        NOTIFICATION_TYPE_STAGE_CHANGE_APPROVED => {
            vec![TEAM_ADMIN_ROLE.to_string(), REQUESTER_ROLE.to_string()]
        }
        NOTIFICATION_TYPE_FEATURE_DEPLOYED => vec![
            TEAM_ADMIN_ROLE.to_string(),
            APPROVER_ROLE.to_string(),
            REQUESTER_ROLE.to_string(),
        ],
        NOTIFICATION_TYPE_FEATURE_ROLLED_BACK => vec![
            TEAM_ADMIN_ROLE.to_string(),
            APPROVER_ROLE.to_string(),
            REQUESTER_ROLE.to_string(),
        ],
        NOTIFICATION_TYPE_USER_ADDED_TO_TEAM => vec![TEAM_ADMIN_ROLE.to_string()],
        NOTIFICATION_TYPE_KILL_SWITCH_ACTIVATED => vec![TEAM_ADMIN_ROLE.to_string()],
        _ => Vec::new(),
    }
}

#[automock]
#[async_trait::async_trait]
pub trait NotificationLogic: Send + Sync {
    async fn get_settings(&self) -> Result<NotificationSettingsView, Error>;

    async fn update_channel_config(
        &self,
        input: UpdateNotificationChannelConfigInput,
    ) -> Result<NotificationChannelConfigView, Error>;

    async fn update_preference(
        &self,
        input: UpdateNotificationPreferenceInput,
    ) -> Result<NotificationPreferenceView, Error>;

    async fn dispatch_event(&self, event: NotificationEvent) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn NotificationLogic>;
}

impl Clone for Box<dyn NotificationLogic> {
    fn clone(&self) -> Box<dyn NotificationLogic> {
        self.clone_box()
    }
}

pub fn notification_logic(
    repository: Box<dyn NotificationRepository>,
) -> Box<dyn NotificationLogic> {
    Box::new(NotificationLogicImpl { repository })
}

#[derive(Clone)]
struct NotificationLogicImpl {
    repository: Box<dyn NotificationRepository>,
}

impl NotificationLogicImpl {
    fn required_string_setting(
        map: &serde_json::Map<String, Value>,
        key: &str,
    ) -> Result<String, String> {
        match map.get(key) {
            Some(Value::String(value)) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    Err(format!("smtp_settings_invalid_{key}"))
                } else {
                    Ok(trimmed.to_string())
                }
            }
            Some(_) => Err(format!("smtp_settings_invalid_{key}")),
            None => Err(format!("smtp_settings_missing_{key}")),
        }
    }

    fn optional_bool_setting(
        map: &serde_json::Map<String, Value>,
        key: &str,
        default_value: bool,
    ) -> Result<bool, String> {
        match map.get(key) {
            None => Ok(default_value),
            Some(Value::Bool(value)) => Ok(*value),
            Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(format!("smtp_settings_invalid_{key}")),
            },
            Some(_) => Err(format!("smtp_settings_invalid_{key}")),
        }
    }

    fn required_port_setting(map: &serde_json::Map<String, Value>) -> Result<u16, String> {
        match map.get("port") {
            Some(Value::Number(value)) => value
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port > 0)
                .ok_or_else(|| "smtp_settings_invalid_port".to_string()),
            Some(Value::String(value)) => value
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .ok_or_else(|| "smtp_settings_invalid_port".to_string()),
            Some(_) => Err("smtp_settings_invalid_port".to_string()),
            None => Err("smtp_settings_missing_port".to_string()),
        }
    }

    fn parse_smtp_settings(settings: &Value) -> Result<SmtpGatewaySettings, String> {
        let map = settings
            .as_object()
            .ok_or_else(|| "smtp_settings_invalid_json_object".to_string())?;

        let host = Self::required_string_setting(map, "host")?;
        let port = Self::required_port_setting(map)?;
        let username = Self::required_string_setting(map, "username")?;
        let password = Self::required_string_setting(map, "password")?;
        let from_email = Self::required_string_setting(map, "fromEmail")?;
        let from_name = map
            .get("fromName")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        let secure = Self::optional_bool_setting(map, "secure", false)?;
        let start_tls = Self::optional_bool_setting(map, "startTls", true)?;

        Ok(SmtpGatewaySettings {
            host,
            port,
            username,
            password,
            from_email,
            from_name,
            secure,
            start_tls,
        })
    }

    fn parse_mailbox(
        email: &str,
        name: Option<&str>,
        invalid_reason: &'static str,
    ) -> Result<Mailbox, String> {
        let address = email.parse().map_err(|_| invalid_reason.to_string())?;
        Ok(Mailbox::new(name.map(|value| value.to_string()), address))
    }

    async fn send_email_via_smtp(
        config: &NotificationChannelConfig,
        recipient: &NotificationRecipient,
        event: &NotificationEvent,
    ) -> Result<(), String> {
        let smtp = Self::parse_smtp_settings(&config.settings)?;
        let from_mailbox = Self::parse_mailbox(
            &smtp.from_email,
            smtp.from_name.as_deref(),
            "smtp_settings_invalid_fromEmail",
        )?;
        let to_mailbox =
            Self::parse_mailbox(&recipient.email, None, "recipient_invalid_email_address")?;

        let message = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(event.subject.clone())
            .body(event.message.clone())
            .map_err(|error| format!("smtp_message_build_failed:{error}"))?;

        let builder = if smtp.secure {
            SmtpTransport::relay(&smtp.host)
                .map_err(|error| format!("smtp_transport_configuration_failed:{error}"))?
        } else if smtp.start_tls {
            SmtpTransport::starttls_relay(&smtp.host)
                .map_err(|error| format!("smtp_transport_configuration_failed:{error}"))?
        } else {
            SmtpTransport::builder_dangerous(smtp.host.clone())
        };

        let mailer = builder
            .port(smtp.port)
            .credentials(Credentials::new(smtp.username, smtp.password))
            .build();

        tokio::task::spawn_blocking(move || mailer.send(&message))
            .await
            .map_err(|error| format!("smtp_send_join_failed:{error}"))?
            .map_err(|error| format!("smtp_send_failed:{error}"))?;

        Ok(())
    }

    fn default_channel_settings(channel: &str) -> serde_json::Value {
        match channel {
            "email" => serde_json::json!({
                "host": "smtp.gmail.com",
                "port": 587,
                "secure": false,
                "startTls": true,
                "username": "no-reply@yourdomain.com",
                "password": "replace-with-app-password",
                "fromEmail": "no-reply@yourdomain.com",
                "fromName": "FluxGate Notifications",
            }),
            "sms" => serde_json::json!({
                "providerAccountSid": "ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                "providerAuthToken": "replace-with-auth-token",
                "fromNumber": "+15551234567",
            }),
            _ => serde_json::json!({}),
        }
    }

    fn is_empty_settings(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Null => true,
            serde_json::Value::Object(map) => map.is_empty(),
            _ => false,
        }
    }

    fn map_channel_config(
        &self,
        row: crate::database::notification::NotificationChannelConfig,
    ) -> NotificationChannelConfigView {
        let settings = if Self::is_empty_settings(&row.settings) {
            Self::default_channel_settings(&row.channel)
        } else {
            row.settings
        };

        NotificationChannelConfigView {
            channel: row.channel,
            enabled: row.enabled,
            provider: row.provider,
            settings,
            updated_by: row.updated_by.map(|id| id.to_string()),
            updated_at: row.updated_at.to_rfc3339(),
        }
    }

    fn map_preference(
        &self,
        definition: &NotificationDefinition,
        row: Option<&NotificationPreference>,
    ) -> NotificationPreferenceView {
        let enabled = row
            .map(|pref| pref.enabled)
            .unwrap_or(definition.default_enabled);
        let email_enabled = row
            .map(|pref| pref.email_enabled)
            .unwrap_or(definition.default_email_enabled);
        let sms_enabled = row
            .map(|pref| pref.sms_enabled)
            .unwrap_or(definition.default_sms_enabled);

        NotificationPreferenceView {
            notification_type: definition.notification_type.to_string(),
            label: definition.label.to_string(),
            description: definition.description.to_string(),
            recipient_scope: definition.recipient_scope.to_string(),
            enabled,
            email_enabled,
            sms_enabled,
            updated_at: row.map(|pref| pref.updated_at.to_rfc3339()),
        }
    }

    async fn resolve_recipients(
        &self,
        event: &NotificationEvent,
    ) -> Result<Vec<NotificationRecipient>, Error> {
        if let Some(user_ids) = event
            .recipient_user_ids
            .as_ref()
            .filter(|ids| !ids.is_empty())
        {
            return self
                .repository
                .list_recipients_by_ids(user_ids.clone())
                .await;
        }

        match selector_for(&event.notification_type) {
            Some(RecipientSelector::SystemAdmins) => {
                self.repository.list_system_admin_recipients().await
            }
            Some(RecipientSelector::TeamRoles) => {
                let team_id = event.team_id.ok_or_else(|| {
                    Error::InvalidInput(
                        "team_id is required for this notification type".to_string(),
                    )
                })?;
                let roles = role_names_for(&event.notification_type);
                self.repository
                    .list_team_recipients_by_roles(team_id, roles)
                    .await
            }
            None => Err(Error::InvalidInput(
                "Unsupported notification type".to_string(),
            )),
        }
    }

    fn effective_preference(
        &self,
        definition: &NotificationDefinition,
        stored: Option<NotificationPreference>,
    ) -> EffectivePreference {
        EffectivePreference {
            enabled: stored
                .as_ref()
                .map(|pref| pref.enabled)
                .unwrap_or(definition.default_enabled),
            email_enabled: stored
                .as_ref()
                .map(|pref| pref.email_enabled)
                .unwrap_or(definition.default_email_enabled),
            sms_enabled: stored
                .as_ref()
                .map(|pref| pref.sms_enabled)
                .unwrap_or(definition.default_sms_enabled),
        }
    }

    async fn write_delivery(
        &self,
        event: &NotificationEvent,
        channel: DeliveryChannel,
        recipient: &NotificationRecipient,
        status: &str,
        failure_reason: Option<String>,
    ) -> Result<(), Error> {
        let now = Utc::now();
        let destination_email = if channel == DeliveryChannel::Email {
            Some(recipient.email.clone())
        } else {
            None
        };
        let destination_mobile = if channel == DeliveryChannel::Sms {
            recipient.mobile_number.clone()
        } else {
            None
        };

        let mut metadata = event
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = metadata.as_object_mut() {
            obj.insert(
                "actor_id".to_string(),
                event
                    .actor_id
                    .map(|id| serde_json::json!(id.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
            obj.insert(
                "recipient_username".to_string(),
                serde_json::json!(recipient.username),
            );
        }

        self.repository
            .create_delivery(CreateNotificationDeliveryInput {
                notification_type: event.notification_type.clone(),
                channel: channel.as_str().to_string(),
                team_id: event.team_id,
                recipient_user_id: Some(recipient.id),
                recipient_email: destination_email,
                recipient_mobile: destination_mobile,
                subject: event.subject.clone(),
                message: event.message.clone(),
                status: status.to_string(),
                failure_reason,
                metadata: Some(metadata),
                sent_at: if status == "sent" { Some(now) } else { None },
            })
            .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl NotificationLogic for NotificationLogicImpl {
    async fn get_settings(&self) -> Result<NotificationSettingsView, Error> {
        let channels = self.repository.list_channel_configs().await?;
        let channel_views = channels
            .into_iter()
            .map(|row| self.map_channel_config(row))
            .collect();

        let preferences = self.repository.list_preferences().await?;
        let preference_map: HashMap<String, NotificationPreference> = preferences
            .into_iter()
            .map(|pref| (pref.notification_type.clone(), pref))
            .collect();

        let preference_views = definitions()
            .iter()
            .map(|definition| {
                self.map_preference(definition, preference_map.get(definition.notification_type))
            })
            .collect();

        Ok(NotificationSettingsView {
            channels: channel_views,
            preferences: preference_views,
        })
    }

    async fn update_channel_config(
        &self,
        input: UpdateNotificationChannelConfigInput,
    ) -> Result<NotificationChannelConfigView, Error> {
        let channel = input.channel.trim().to_lowercase();
        if channel != "email" && channel != "sms" {
            return Err(Error::InvalidInput(
                "channel must be either 'email' or 'sms'".to_string(),
            ));
        }

        let provider = input.provider.trim().to_string();
        if input.enabled && provider.is_empty() {
            return Err(Error::InvalidInput(
                "provider is required when channel is enabled".to_string(),
            ));
        }

        let updated = self
            .repository
            .upsert_channel_config(UpsertNotificationChannelConfigInput {
                channel,
                enabled: input.enabled,
                provider,
                settings: input.settings,
                updated_by: input.actor_id,
            })
            .await?;

        Ok(self.map_channel_config(updated))
    }

    async fn update_preference(
        &self,
        input: UpdateNotificationPreferenceInput,
    ) -> Result<NotificationPreferenceView, Error> {
        let notification_type = input.notification_type.trim().to_string();
        let definition = definition_for(&notification_type)
            .ok_or_else(|| Error::InvalidInput("Unsupported notification type".to_string()))?;

        let existing = self.repository.get_preference(&notification_type).await?;
        let effective = self.effective_preference(definition, existing.clone());

        let updated = self
            .repository
            .upsert_preference(UpsertNotificationPreferenceInput {
                notification_type,
                enabled: input.enabled.unwrap_or(effective.enabled),
                email_enabled: input.email_enabled.unwrap_or(effective.email_enabled),
                sms_enabled: input.sms_enabled.unwrap_or(effective.sms_enabled),
            })
            .await?;

        Ok(self.map_preference(definition, Some(&updated)))
    }

    async fn dispatch_event(&self, event: NotificationEvent) -> Result<(), Error> {
        let definition = definition_for(&event.notification_type)
            .ok_or_else(|| Error::InvalidInput("Unsupported notification type".to_string()))?;

        let preference = self
            .repository
            .get_preference(&event.notification_type)
            .await?;
        let effective = self.effective_preference(definition, preference);

        if !effective.enabled {
            return Ok(());
        }

        let recipients = self.resolve_recipients(&event).await?;
        if recipients.is_empty() {
            return Ok(());
        }

        let channel_configs = self.repository.list_channel_configs().await?;
        let mut channels_by_name = HashMap::new();
        for channel in channel_configs {
            channels_by_name.insert(channel.channel.to_lowercase(), channel);
        }

        let email_active = effective.email_enabled
            && channels_by_name
                .get("email")
                .map(|config| config.enabled && !config.provider.trim().is_empty())
                .unwrap_or(false);

        let sms_active = effective.sms_enabled
            && channels_by_name
                .get("sms")
                .map(|config| config.enabled && !config.provider.trim().is_empty())
                .unwrap_or(false);

        if !email_active && !sms_active {
            return Ok(());
        }

        let email_channel_config = channels_by_name.get("email");

        for recipient in &recipients {
            if email_active {
                if recipient.email.trim().is_empty() {
                    self.write_delivery(
                        &event,
                        DeliveryChannel::Email,
                        recipient,
                        "skipped",
                        Some("recipient_missing_email".to_string()),
                    )
                    .await?;
                } else {
                    let status = if let Some(config) = email_channel_config {
                        let provider = config.provider.trim().to_ascii_lowercase();
                        if provider == "smtp" {
                            match Self::send_email_via_smtp(config, recipient, &event).await {
                                Ok(()) => ("sent", None),
                                Err(reason) => ("failed", Some(reason)),
                            }
                        } else {
                            (
                                "failed",
                                Some(format!("unsupported_email_provider:{provider}")),
                            )
                        }
                    } else {
                        ("failed", Some("email_channel_not_configured".to_string()))
                    };

                    self.write_delivery(
                        &event,
                        DeliveryChannel::Email,
                        recipient,
                        status.0,
                        status.1,
                    )
                    .await?;
                }
            }

            if sms_active {
                if recipient
                    .mobile_number
                    .as_ref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                {
                    self.write_delivery(
                        &event,
                        DeliveryChannel::Sms,
                        recipient,
                        "skipped",
                        Some("recipient_missing_mobile_number".to_string()),
                    )
                    .await?;
                } else {
                    self.write_delivery(&event, DeliveryChannel::Sms, recipient, "queued", None)
                        .await?;
                }
            }
        }

        Ok(())
    }

    fn clone_box(&self) -> Box<dyn NotificationLogic> {
        Box::new(self.clone())
    }
}

pub fn spawn_notification_dispatch(logic: Box<dyn NotificationLogic>, event: NotificationEvent) {
    tokio::spawn(async move {
        if let Err(error) = logic.dispatch_event(event).await {
            log::error!("notification dispatch failed: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::notification::{
        CreateNotificationDeliveryInput, MockNotificationRepository, NotificationChannelConfig,
        NotificationDelivery,
    };
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn sample_channel(channel: &str, enabled: bool) -> NotificationChannelConfig {
        NotificationChannelConfig {
            channel: channel.to_string(),
            enabled,
            provider: "test-provider".to_string(),
            settings: serde_json::json!({}),
            updated_by: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_delivery(input: CreateNotificationDeliveryInput) -> NotificationDelivery {
        NotificationDelivery {
            id: Uuid::new_v4(),
            notification_type: input.notification_type,
            channel: input.channel,
            team_id: input.team_id,
            recipient_user_id: input.recipient_user_id,
            recipient_email: input.recipient_email,
            recipient_mobile: input.recipient_mobile,
            subject: input.subject,
            message: input.message,
            status: input.status,
            failure_reason: input.failure_reason,
            metadata: input.metadata,
            created_at: Utc::now(),
            sent_at: input.sent_at,
        }
    }

    #[tokio::test]
    async fn dispatch_event_sends_email_and_sms_to_team_admins() {
        let team_id = Uuid::new_v4();
        let recipient = NotificationRecipient {
            id: Uuid::new_v4(),
            username: "team-admin".to_string(),
            first_name: "Team".to_string(),
            last_name: "Admin".to_string(),
            email: "admin@example.com".to_string(),
            mobile_number: Some("+15551234567".to_string()),
        };

        let mut mock_repo = MockNotificationRepository::new();

        mock_repo.expect_get_preference().times(1).returning(|_| {
            Ok(Some(NotificationPreference {
                notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
                enabled: true,
                email_enabled: true,
                sms_enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }))
        });

        mock_repo
            .expect_list_team_recipients_by_roles()
            .times(1)
            .returning(move |_, _| Ok(vec![recipient.clone()]));

        mock_repo
            .expect_list_channel_configs()
            .times(1)
            .returning(|| {
                Ok(vec![
                    sample_channel("email", true),
                    sample_channel("sms", true),
                ])
            });

        mock_repo
            .expect_create_delivery()
            .times(2)
            .returning(|input| Ok(sample_delivery(input)));

        let logic = notification_logic(Box::new(mock_repo));

        logic
            .dispatch_event(NotificationEvent {
                notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
                team_id: Some(team_id),
                actor_id: None,
                recipient_user_ids: None,
                subject: "Feature created".to_string(),
                message: "A feature was created".to_string(),
                metadata: None,
            })
            .await
            .expect("dispatch should succeed");
    }

    #[tokio::test]
    async fn dispatch_event_uses_explicit_recipient_user_ids() {
        let team_id = Uuid::new_v4();
        let recipient_id = Uuid::new_v4();
        let recipient = NotificationRecipient {
            id: recipient_id,
            username: "target-approver".to_string(),
            first_name: "Target".to_string(),
            last_name: "Approver".to_string(),
            email: "target@example.com".to_string(),
            mobile_number: None,
        };

        let mut mock_repo = MockNotificationRepository::new();

        mock_repo.expect_get_preference().times(1).returning(|_| {
            Ok(Some(NotificationPreference {
                notification_type: NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED.to_string(),
                enabled: true,
                email_enabled: true,
                sms_enabled: false,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }))
        });

        mock_repo
            .expect_list_recipients_by_ids()
            .times(1)
            .withf(move |ids| ids == &vec![recipient_id])
            .returning(move |_| Ok(vec![recipient.clone()]));

        mock_repo.expect_list_team_recipients_by_roles().times(0);

        mock_repo
            .expect_list_channel_configs()
            .times(1)
            .returning(|| Ok(vec![sample_channel("email", true)]));

        mock_repo
            .expect_create_delivery()
            .times(1)
            .returning(move |input| {
                assert_eq!(input.recipient_user_id, Some(recipient_id));
                Ok(sample_delivery(input))
            });

        let logic = notification_logic(Box::new(mock_repo));

        logic
            .dispatch_event(NotificationEvent {
                notification_type: NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED.to_string(),
                team_id: Some(team_id),
                actor_id: None,
                recipient_user_ids: Some(vec![recipient_id]),
                subject: "Requested".to_string(),
                message: "Deployment requested".to_string(),
                metadata: None,
            })
            .await
            .expect("dispatch should succeed");
    }

    #[tokio::test]
    async fn dispatch_event_skips_sms_when_mobile_missing() {
        let team_id = Uuid::new_v4();
        let recipient = NotificationRecipient {
            id: Uuid::new_v4(),
            username: "approver".to_string(),
            first_name: "App".to_string(),
            last_name: "Rover".to_string(),
            email: "approver@example.com".to_string(),
            mobile_number: None,
        };

        let mut mock_repo = MockNotificationRepository::new();

        mock_repo.expect_get_preference().times(1).returning(|_| {
            Ok(Some(NotificationPreference {
                notification_type: NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED.to_string(),
                enabled: true,
                email_enabled: false,
                sms_enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }))
        });

        mock_repo
            .expect_list_team_recipients_by_roles()
            .times(1)
            .returning(move |_, _| Ok(vec![recipient.clone()]));

        mock_repo
            .expect_list_channel_configs()
            .times(1)
            .returning(|| Ok(vec![sample_channel("sms", true)]));

        mock_repo
            .expect_create_delivery()
            .times(1)
            .returning(|input| {
                assert_eq!(input.status, "skipped");
                assert_eq!(
                    input.failure_reason.as_deref(),
                    Some("recipient_missing_mobile_number")
                );
                Ok(sample_delivery(input))
            });

        let logic = notification_logic(Box::new(mock_repo));

        logic
            .dispatch_event(NotificationEvent {
                notification_type: NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED.to_string(),
                team_id: Some(team_id),
                actor_id: None,
                recipient_user_ids: None,
                subject: "Requested".to_string(),
                message: "Deployment requested".to_string(),
                metadata: None,
            })
            .await
            .expect("dispatch should succeed");
    }

    #[derive(Clone)]
    struct TestNotificationLogic {
        delay: Duration,
        sender: mpsc::UnboundedSender<String>,
    }

    #[async_trait::async_trait]
    impl NotificationLogic for TestNotificationLogic {
        async fn get_settings(&self) -> Result<NotificationSettingsView, Error> {
            Err(Error::InvalidInput("not_implemented_for_test".to_string()))
        }

        async fn update_channel_config(
            &self,
            _input: UpdateNotificationChannelConfigInput,
        ) -> Result<NotificationChannelConfigView, Error> {
            Err(Error::InvalidInput("not_implemented_for_test".to_string()))
        }

        async fn update_preference(
            &self,
            _input: UpdateNotificationPreferenceInput,
        ) -> Result<NotificationPreferenceView, Error> {
            Err(Error::InvalidInput("not_implemented_for_test".to_string()))
        }

        async fn dispatch_event(&self, event: NotificationEvent) -> Result<(), Error> {
            tokio::time::sleep(self.delay).await;
            let _ = self.sender.send(event.subject);
            Ok(())
        }

        fn clone_box(&self) -> Box<dyn NotificationLogic> {
            Box::new(self.clone())
        }
    }

    #[tokio::test]
    async fn spawn_notification_dispatch_runs_in_background() {
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let logic = Box::new(TestNotificationLogic {
            delay: Duration::from_millis(250),
            sender,
        });

        let subject = format!("bg-dispatch-{}", Uuid::new_v4());
        let start = tokio::time::Instant::now();
        spawn_notification_dispatch(
            logic,
            NotificationEvent {
                notification_type: NOTIFICATION_TYPE_FEATURE_CREATED.to_string(),
                team_id: Some(Uuid::new_v4()),
                actor_id: None,
                recipient_user_ids: None,
                subject: subject.clone(),
                message: "background dispatch test".to_string(),
                metadata: None,
            },
        );

        assert!(start.elapsed() < Duration::from_millis(100));

        let observed = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("background task did not complete")
            .expect("channel unexpectedly closed");
        assert_eq!(observed, subject);
    }
}
