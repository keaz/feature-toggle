//! Device-code login for the CLI. `authorize` and `token` are public and rate
//! limited; `approve` needs a signed-in person (the UI's `/device` page).

use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};

use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, post, web};
use chrono::{Duration, Utc};
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Quota, RateLimiter};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::JwtUser;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::cli_device_authorization::{
    CliDeviceAuthorizationRepository, CliDeviceAuthorizationRepositoryTx,
    NewCliDeviceAuthorization, cli_device_authorization_repository,
    cli_device_authorization_repository_tx,
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

/// In-process limits for the public device routes: one budget per client
/// address, so one noisy client cannot lock others out, and a total per route.
/// The backend runs as one instance, so the state is in memory. Requests from
/// a trusted reverse proxy are counted by their forwarded client address.
pub struct DeviceAuthLimiter {
    authorize: RouteLimit,
    token: RouteLimit,
    trusted_proxies: Vec<IpAddr>,
    /// Client buckets kept per route; past it, new requests count only
    /// against the total (a flood of addresses cannot grow the map).
    client_cap: usize,
    admits: AtomicUsize,
    clock: DefaultClock,
}

struct RouteLimit {
    per_client: DefaultKeyedRateLimiter<IpAddr>,
    total: DefaultDirectRateLimiter,
}

/// Default for [`DeviceAuthLimiter::with_client_cap`].
const MAX_TRACKED_CLIENTS: usize = 10_000;
/// Idle buckets are swept at most once per this many admissions over the cap.
const SWEEP_EVERY: usize = 256;

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value.max(1)).expect("value is at least 1")
}

/// `per_minute` requests a minute, all of which may come at once.
fn quota(per_minute: u32) -> Quota {
    Quota::per_minute(non_zero(per_minute)).allow_burst(non_zero(per_minute))
}

impl DeviceAuthLimiter {
    /// Requests per minute and client for `authorize` and for `token`. The
    /// totals per route default to 30 times that; see [`Self::with_totals`].
    pub fn new(authorize_per_minute: u32, token_per_minute: u32) -> Self {
        let route = |per_minute: u32| RouteLimit {
            per_client: RateLimiter::keyed(quota(per_minute)),
            total: RateLimiter::direct(quota(
                per_minute.saturating_mul(crate::config::DEFAULT_DEVICE_TOTAL_FACTOR),
            )),
        };
        Self {
            authorize: route(authorize_per_minute),
            token: route(token_per_minute),
            trusted_proxies: Vec::new(),
            client_cap: MAX_TRACKED_CLIENTS,
            admits: AtomicUsize::new(0),
            clock: DefaultClock::default(),
        }
    }

    /// From the `[device_login]` configuration.
    pub fn from_config(config: &crate::config::DeviceLoginConfig) -> Self {
        Self::new(config.authorize_per_minute, config.token_per_minute)
            .with_totals(
                config
                    .authorize_per_minute
                    .saturating_mul(config.total_factor),
                config.token_per_minute.saturating_mul(config.total_factor),
            )
            .with_trusted_proxies(config.trusted_proxies.clone())
    }

    /// Requests per minute for each route across all clients.
    pub fn with_totals(mut self, authorize_per_minute: u32, token_per_minute: u32) -> Self {
        self.authorize.total = RateLimiter::direct(quota(authorize_per_minute));
        self.token.total = RateLimiter::direct(quota(token_per_minute));
        self
    }

    /// Proxies whose `Forwarded` / `X-Forwarded-For` client address is used.
    pub fn with_trusted_proxies(mut self, proxies: Vec<IpAddr>) -> Self {
        self.trusted_proxies = proxies;
        self
    }

    /// Client buckets kept per route (default 10 000).
    pub fn with_client_cap(mut self, cap: usize) -> Self {
        self.client_cap = cap.max(1);
        self
    }

    /// The address to count a request against: the connection's peer, or the
    /// forwarded client address when the peer is a trusted proxy.
    fn client_address(&self, req: &HttpRequest) -> Option<IpAddr> {
        let peer = req.peer_addr()?.ip();
        if !self.trusted_proxies.contains(&peer) {
            return Some(peer);
        }
        let info = req.connection_info();
        let forwarded = info.realip_remote_addr()?;
        forwarded
            .parse::<IpAddr>()
            .or_else(|_| {
                forwarded
                    .parse::<std::net::SocketAddr>()
                    .map(|addr| addr.ip())
            })
            .ok()
            .or(Some(peer))
    }

    fn admit(&self, route: &RouteLimit, client: Option<IpAddr>) -> Result<(), RestError> {
        let limited = |wait: std::time::Duration| {
            RestError::too_many_requests(wait.as_secs_f64().ceil().max(1.0) as u64)
        };
        if let Some(client) = client {
            let mut tracked = route.per_client.len() < self.client_cap;
            if !tracked
                && self
                    .admits
                    .fetch_add(1, Ordering::Relaxed)
                    .is_multiple_of(SWEEP_EVERY)
            {
                route.per_client.retain_recent();
                tracked = route.per_client.len() < self.client_cap;
            }
            if tracked {
                route
                    .per_client
                    .check_key(&client)
                    .map_err(|not_until| limited(not_until.wait_time_from(self.clock.now())))?;
            }
        }
        route
            .total
            .check()
            .map_err(|not_until| limited(not_until.wait_time_from(self.clock.now())))
    }
}

impl Default for DeviceAuthLimiter {
    fn default() -> Self {
        Self::from_config(&crate::config::DeviceLoginConfig::default())
    }
}

// `Debug` redacts the device code (a plain comment: doc comments become OpenAPI text).
#[derive(Serialize, ToSchema)]
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

impl std::fmt::Debug for DeviceAuthorizeResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuthorizeResponse")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
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
    req: HttpRequest,
    db_pool: web::Data<sqlx::PgPool>,
    limiter: web::Data<DeviceAuthLimiter>,
    config: web::Data<SsoLoginConfig>,
) -> Result<impl Responder, RestError> {
    limiter.admit(&limiter.authorize, limiter.client_address(&req))?;
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
    req: HttpRequest,
    db_pool: web::Data<sqlx::PgPool>,
    limiter: web::Data<DeviceAuthLimiter>,
    tokens: web::Data<Box<dyn JwtTokenLogic>>,
    payload: web::Json<DeviceTokenRequest>,
) -> Result<impl Responder, RestError> {
    limiter.admit(&limiter.token, limiter.client_address(&req))?;
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
                Err(err) => return Err(restore(&repo, row.id, RestError::from(err)).await),
            };
            if !user.enabled {
                return Err(expired());
            }
            let issued = match tokens.issue_session(ApiUser::from(user)).await {
                Ok(result) => login_response(db_pool.get_ref(), result).await,
                Err(err) => Err(RestError::from(err)),
            };
            match issued {
                Ok(response) => Ok(HttpResponse::Ok().json(response)),
                Err(err) => Err(restore(&repo, row.id, err).await),
            }
        }
    }
}

/// Puts a consumed row back to approved after a failure while issuing the
/// session, so the next poll gets the session without a new approval.
async fn restore(
    repo: &Box<dyn CliDeviceAuthorizationRepository>,
    id: uuid::Uuid,
    err: RestError,
) -> RestError {
    if let Err(restore_err) = repo.restore_approved(id).await {
        log::error!(
            "could not restore CLI device login {id} after a failed session issue: {restore_err}"
        );
    }
    err
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_response_debug_hides_the_device_code() {
        let response = DeviceAuthorizeResponse {
            device_code: "device-secret-value".into(),
            user_code: "BCDF-GHJK".into(),
            verification_uri: "http://ui/device".into(),
            verification_uri_complete: "http://ui/device?code=BCDF-GHJK".into(),
            expires_in: 600,
            interval: 5,
        };
        let shown = format!("{response:?}");
        assert!(!shown.contains("device-secret-value"), "{shown}");
        assert!(shown.contains("BCDF-GHJK"));
    }

    #[test]
    fn past_the_client_cap_new_clients_fall_back_to_the_totals() {
        let limiter = DeviceAuthLimiter::new(1, 1)
            .with_totals(100, 100)
            .with_client_cap(2);
        let ip = |n: u8| Some(IpAddr::from([10, 0, 0, n]));
        assert!(limiter.admit(&limiter.authorize, ip(1)).is_ok());
        assert!(limiter.admit(&limiter.authorize, ip(2)).is_ok());
        for n in 3..50 {
            assert!(
                limiter.admit(&limiter.authorize, ip(n)).is_ok(),
                "client {n}"
            );
        }
        assert!(
            limiter.authorize.per_client.len() <= 2,
            "the map stays bounded"
        );
        assert!(limiter.admit(&limiter.authorize, None).is_ok());
    }
}
