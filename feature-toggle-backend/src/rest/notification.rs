use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, put, web};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::JwtUser;
use crate::logic::notification::{
    NotificationLogic, NotificationSettingsView, UpdateNotificationChannelConfigInput,
    UpdateNotificationPreferenceInput,
};
use crate::rest::error::RestError;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationChannelConfigResponse {
    pub channel: String,
    pub enabled: bool,
    pub provider: String,
    pub settings: serde_json::Value,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPreferenceResponse {
    pub notification_type: String,
    pub label: String,
    pub description: String,
    pub recipient_scope: String,
    pub enabled: bool,
    pub email_enabled: bool,
    pub sms_enabled: bool,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettingsResponse {
    pub channels: Vec<NotificationChannelConfigResponse>,
    pub preferences: Vec<NotificationPreferenceResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotificationChannelConfigRequest {
    pub enabled: bool,
    pub provider: String,
    pub settings: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotificationPreferenceRequest {
    pub enabled: Option<bool>,
    pub email_enabled: Option<bool>,
    pub sms_enabled: Option<bool>,
}

impl From<crate::logic::notification::NotificationChannelConfigView>
    for NotificationChannelConfigResponse
{
    fn from(value: crate::logic::notification::NotificationChannelConfigView) -> Self {
        Self {
            channel: value.channel,
            enabled: value.enabled,
            provider: value.provider,
            settings: value.settings,
            updated_by: value.updated_by,
            updated_at: value.updated_at,
        }
    }
}

impl From<crate::logic::notification::NotificationPreferenceView>
    for NotificationPreferenceResponse
{
    fn from(value: crate::logic::notification::NotificationPreferenceView) -> Self {
        Self {
            notification_type: value.notification_type,
            label: value.label,
            description: value.description,
            recipient_scope: value.recipient_scope,
            enabled: value.enabled,
            email_enabled: value.email_enabled,
            sms_enabled: value.sms_enabled,
            updated_at: value.updated_at,
        }
    }
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn ensure_admin(req: &HttpRequest) -> Result<JwtUser, RestError> {
    let jwt = jwt_user(req)?;
    if !jwt.is_admin {
        return Err(RestError::forbidden(
            "Only system administrators can manage notification settings",
        ));
    }
    Ok(jwt)
}

fn map_settings(settings: NotificationSettingsView) -> NotificationSettingsResponse {
    NotificationSettingsResponse {
        channels: settings
            .channels
            .into_iter()
            .map(NotificationChannelConfigResponse::from)
            .collect(),
        preferences: settings
            .preferences
            .into_iter()
            .map(NotificationPreferenceResponse::from)
            .collect(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/notifications/settings",
    responses(
        (status = 200, description = "Notification settings", body = NotificationSettingsResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Notifications"
)]
#[get("/notifications/settings")]
pub(crate) async fn get_notification_settings(
    logic: web::Data<Box<dyn NotificationLogic>>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    ensure_admin(&req)?;
    let settings = logic.get_settings().await.map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(map_settings(settings)))
}

#[utoipa::path(
    put,
    path = "/api/v1/notifications/channels/{channel}",
    request_body = UpdateNotificationChannelConfigRequest,
    params(
        ("channel" = String, Path, description = "Channel identifier: email or sms")
    ),
    responses(
        (status = 200, description = "Updated channel config", body = NotificationChannelConfigResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Notifications"
)]
#[put("/notifications/channels/{channel}")]
pub(crate) async fn update_notification_channel(
    logic: web::Data<Box<dyn NotificationLogic>>,
    req: HttpRequest,
    channel: web::Path<String>,
    payload: web::Json<UpdateNotificationChannelConfigRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = ensure_admin(&req)?;
    let updated = logic
        .update_channel_config(UpdateNotificationChannelConfigInput {
            channel: channel.into_inner(),
            enabled: payload.enabled,
            provider: payload.provider.clone(),
            settings: payload.settings.clone(),
            actor_id: Some(jwt.id),
        })
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(NotificationChannelConfigResponse::from(updated)))
}

#[utoipa::path(
    put,
    path = "/api/v1/notifications/preferences/{notificationType}",
    request_body = UpdateNotificationPreferenceRequest,
    params(
        ("notificationType" = String, Path, description = "Predefined notification type")
    ),
    responses(
        (status = 200, description = "Updated preference", body = NotificationPreferenceResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Notifications"
)]
#[put("/notifications/preferences/{notificationType}")]
pub(crate) async fn update_notification_preference(
    logic: web::Data<Box<dyn NotificationLogic>>,
    req: HttpRequest,
    notification_type: web::Path<String>,
    payload: web::Json<UpdateNotificationPreferenceRequest>,
) -> Result<impl Responder, RestError> {
    ensure_admin(&req)?;
    let updated = logic
        .update_preference(UpdateNotificationPreferenceInput {
            notification_type: notification_type.into_inner(),
            enabled: payload.enabled,
            email_enabled: payload.email_enabled,
            sms_enabled: payload.sms_enabled,
        })
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(NotificationPreferenceResponse::from(updated)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_notification_settings)
        .service(update_notification_channel)
        .service(update_notification_preference);
}
