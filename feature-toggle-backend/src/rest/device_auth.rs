//! Device-code login for the CLI. `authorize` and `token` are public and rate
//! limited; `approve` needs a signed-in person (the UI's `/device` page).

use std::num::NonZeroU32;

use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, post, web};
use chrono::{Duration, Utc};
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::JwtUser;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::cli_device_authorization::{
    CliDeviceAuthorizationRepositoryTx, NewCliDeviceAuthorization,
    cli_device_authorization_repository, cli_device_authorization_repository_tx,
};
use crate::database::user::user_repository;
use crate::logic::device_login::{
    DEVICE_CODE_TTL_MINUTES, POLL_INTERVAL_SECS, PollOutcome, new_device_code, new_user_code,
    normalize_user_code, poll_outcome,
};
use crate::logic::jwt_token::JwtTokenLogic;
use crate::logic::user::ApiUser;
use crate::middleware::jwt_guard::hash_token;
use crate::rest::auth::{LoginResponse, login_response};
use crate::rest::error::{ErrorResponse, RestError};
use crate::rest::sso_auth::SsoLoginConfig;
use crate::utils::activity_logger::{activity_types, entity_types};

/// Tries to find a free user code before giving up.
const USER_CODE_ATTEMPTS: usize = 5;

/// In-process limits for the public device routes. The backend runs as one
/// instance, so the state is in memory.
pub struct DeviceAuthLimiter {
    authorize: DefaultDirectRateLimiter,
    token: DefaultDirectRateLimiter,
    clock: DefaultClock,
}

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value.max(1)).expect("value is at least 1")
}

impl DeviceAuthLimiter {
    /// Requests per minute for `authorize` and for `token`; bursts are a sixth
    /// of the rate (at least 1).
    pub fn new(authorize_per_minute: u32, token_per_minute: u32) -> Self {
        let quota = |per_minute: u32| {
            Quota::per_minute(non_zero(per_minute)).allow_burst(non_zero(per_minute / 6))
        };
        Self {
            authorize: RateLimiter::direct(quota(authorize_per_minute)),
            token: RateLimiter::direct(quota(token_per_minute)),
            clock: DefaultClock::default(),
        }
    }

    fn admit(&self, limiter: &DefaultDirectRateLimiter) -> Result<(), RestError> {
        limiter.check().map_err(|not_until| {
            let wait = not_until.wait_time_from(self.clock.now());
            RestError::too_many_requests(wait.as_secs_f64().ceil().max(1.0) as u64)
        })
    }
}

impl Default for DeviceAuthLimiter {
    fn default() -> Self {
        Self::new(60, 600)
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorizeResponse {
    /// Secret the CLI polls `token` with.
    pub device_code: String,
    /// Code the person confirms in the UI, `XXXX-XXXX`.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    /// Seconds until both codes expire.
    pub expires_in: i64,
    /// Minimum seconds between polls.
    pub interval: i32,
}

/// Body of `POST /auth/device/token`. `Debug` redacts the device code.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTokenRequest {
    pub device_code: String,
}

impl std::fmt::Debug for DeviceTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceTokenRequest")
            .field("device_code", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceApproveRequest {
    /// The code shown by the CLI; case, spaces and the hyphen do not matter.
    pub user_code: String,
    /// `true` approves the login, `false` denies it.
    pub approve: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceApproveResponse {
    /// `approved` or `denied`.
    pub status: String,
}

fn poll_error(code: &str, message: &str) -> RestError {
    RestError::InvalidInput {
        message: message.to_string(),
        code: Some(code.to_string()),
        details: None,
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/device/authorize",
    responses(
        (status = 200, description = "Device and user codes for a CLI login", body = DeviceAuthorizeResponse),
        (status = 429, description = "Rate limited; see Retry-After", body = ErrorResponse)
    ),
    security(()),
    tag = "Auth"
)]
#[post("/auth/device/authorize")]
pub(crate) async fn device_authorize(
    db_pool: web::Data<sqlx::PgPool>,
    limiter: web::Data<DeviceAuthLimiter>,
    config: web::Data<SsoLoginConfig>,
) -> Result<impl Responder, RestError> {
    limiter.admit(&limiter.authorize)?;
    let repo = cli_device_authorization_repository(db_pool.get_ref().clone());
    let device_code = new_device_code();
    let expires_at = Utc::now() + Duration::minutes(DEVICE_CODE_TTL_MINUTES);
    let mut created = None;
    for _ in 0..USER_CODE_ATTEMPTS {
        let input = NewCliDeviceAuthorization {
            device_code_hash: hash_token(&device_code),
            user_code: new_user_code(),
            interval_secs: POLL_INTERVAL_SECS,
            expires_at,
        };
        match repo.create(input).await {
            Ok(row) => {
                created = Some(row);
                break;
            }
            // A user code still in use: draw another.
            Err(crate::Error::DatabaseError(sqlx::Error::Database(err)))
                if err.is_unique_violation() => {}
            Err(err) => return Err(RestError::from(err)),
        }
    }
    let row = created.ok_or_else(|| RestError::internal("no free user code"))?;
    let verification_uri = format!("{}/device", config.ui_origin.trim_end_matches('/'));
    Ok(HttpResponse::Ok().json(DeviceAuthorizeResponse {
        device_code,
        verification_uri_complete: format!("{verification_uri}?code={}", row.user_code),
        user_code: row.user_code,
        verification_uri,
        expires_in: DEVICE_CODE_TTL_MINUTES * 60,
        interval: POLL_INTERVAL_SECS,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/device/token",
    request_body = DeviceTokenRequest,
    responses(
        (status = 200, description = "The login was approved; a new session (released once)", body = LoginResponse),
        (status = 400, description = "`code` is authorization_pending, slow_down (polled faster than `interval`), access_denied or expired_token (expired, already used or unknown)", body = ErrorResponse),
        (status = 429, description = "Rate limited; see Retry-After", body = ErrorResponse)
    ),
    security(()),
    tag = "Auth"
)]
#[post("/auth/device/token")]
pub(crate) async fn device_token(
    db_pool: web::Data<sqlx::PgPool>,
    limiter: web::Data<DeviceAuthLimiter>,
    tokens: web::Data<Box<dyn JwtTokenLogic>>,
    payload: web::Json<DeviceTokenRequest>,
) -> Result<impl Responder, RestError> {
    limiter.admit(&limiter.token)?;
    let expired = || {
        poll_error(
            "expired_token",
            "device code expired, already used or unknown",
        )
    };
    let repo = cli_device_authorization_repository(db_pool.get_ref().clone());
    let Some(row) = repo
        .find_by_device_code_hash(&hash_token(&payload.device_code))
        .await?
    else {
        return Err(expired());
    };
    let now = Utc::now();
    match poll_outcome(&row, now) {
        PollOutcome::Expired => Err(expired()),
        PollOutcome::Denied => Err(poll_error("access_denied", "the login was denied")),
        PollOutcome::SlowDown => {
            repo.record_poll(row.id, now).await?;
            Err(poll_error(
                "slow_down",
                "polling too fast; add 5 seconds to the interval",
            ))
        }
        PollOutcome::Pending => {
            repo.record_poll(row.id, now).await?;
            Err(poll_error(
                "authorization_pending",
                "waiting for approval in the browser",
            ))
        }
        PollOutcome::Approved => {
            let Some(consumed) = repo.consume_approved(row.id).await? else {
                return Err(expired());
            };
            let user_id = consumed.user_id.ok_or_else(expired)?;
            let user = match user_repository(db_pool.get_ref().clone())
                .get_user_by_id(user_id)
                .await
            {
                Ok(user) => user,
                Err(crate::Error::NotFound(_)) => return Err(expired()),
                Err(err) => return Err(RestError::from(err)),
            };
            if !user.enabled {
                return Err(expired());
            }
            let result = tokens
                .issue_session(ApiUser::from(user))
                .await
                .map_err(RestError::from)?;
            Ok(HttpResponse::Ok().json(login_response(db_pool.get_ref(), result).await?))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/device/approve",
    request_body = DeviceApproveRequest,
    responses(
        (status = 200, description = "Decision recorded", body = DeviceApproveResponse),
        (status = 400, description = "Not a user code", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 403, description = "System clients cannot approve logins", body = ErrorResponse),
        (status = 404, description = "No pending, unexpired login with this code", body = ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/device/approve")]
pub(crate) async fn device_approve(
    req: HttpRequest,
    db_pool: web::Data<sqlx::PgPool>,
    activity: web::Data<Box<dyn ActivityLogRepository>>,
    payload: web::Json<DeviceApproveRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    if jwt.team_id.is_some() {
        return Err(RestError::forbidden(
            "system clients cannot approve CLI logins",
        ));
    }
    let user_code = normalize_user_code(&payload.user_code)
        .ok_or_else(|| RestError::invalid_input("userCode must look like XXXX-XXXX"))?;

    let repo = cli_device_authorization_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::from(crate::Error::DatabaseError(e)))?;
    let decided = repo
        .decide_tx(&mut tx, &user_code, jwt.id, payload.approve)
        .await?;
    let Some(decided) = decided else {
        let _ = tx.rollback().await;
        return Err(RestError::not_found(
            "no pending login with this code; it may have expired",
        ));
    };
    let (activity_type, verb) = if payload.approve {
        (activity_types::CLI_LOGIN_APPROVED, "approved")
    } else {
        (activity_types::CLI_LOGIN_DENIED, "denied")
    };
    activity
        .create_activity_tx(
            &mut tx,
            CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: entity_types::USER.to_string(),
                entity_id: jwt.id.to_string(),
                actor_id: Some(jwt.id),
                actor_name: Some(jwt.username.clone()),
                description: format!(
                    "{} {verb} a CLI login ({})",
                    jwt.username, decided.user_code
                ),
                metadata: Some(serde_json::json!({ "userCode": decided.user_code })),
            },
        )
        .await
        .map_err(|e| RestError::from(crate::Error::DatabaseError(e)))?;
    tx.commit()
        .await
        .map_err(|e| RestError::from(crate::Error::DatabaseError(e)))?;
    Ok(HttpResponse::Ok().json(DeviceApproveResponse {
        status: decided.status,
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(device_authorize)
        .service(device_token)
        .service(device_approve);
}
