//! Public SSO login endpoints: start the OIDC authorization request, handle the IdP
//! callback, and exchange the one-time code for a FluxGate session.
//!
//! Failures of `authorize` and `callback` never produce an error body: the browser
//! is redirected to `<ui>/login?ssoError=<code>`. Details are logged server side,
//! without codes, tokens, verifiers or secrets.

use actix_web::cookie::time::Duration as CookieDuration;
use actix_web::cookie::{Cookie, SameSite};
use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, Responder, get, post, web};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use utoipa::{IntoParams, ToSchema};

use crate::database::activity_log::ActivityLogRepository;
use crate::database::role::role_repository_tx;
use crate::database::sso_group_mapping::sso_group_mapping_repository_tx;
use crate::database::sso_login_code::{sso_login_code_repository, sso_login_code_repository_tx};
use crate::database::sso_login_state::{NewSsoLoginState, sso_login_state_repository};
use crate::database::sso_provider::sso_provider_repository;
use crate::database::user::{user_repository, user_repository_tx};
use crate::database::user_identity::user_identity_repository_tx;
use crate::logic::jwt_token::JwtTokenLogic;
use crate::logic::oidc_client::{
    CodeExchange, OidcClient, OidcError, pkce_challenge, random_token,
};
use crate::logic::sso_login::{
    AuthorizeParams, LOGIN_STATE_TTL_MINUTES, SsoLoginError, authorize_url, callback_url,
    complete_url, error_url, sanitize_redirect,
};
use crate::logic::sso_login_tx::{SsoLoginRepos, complete_sso_login_in_tx};
use crate::logic::sso_provider::{SsoSecrets, resolve_client_secret, validate_slug};
use crate::logic::user::ApiUser;
use crate::middleware::jwt_guard::hash_token;
use crate::rest::auth::{LoginResponse, login_response};
use crate::rest::error::{ErrorResponse, RestError};

/// Settings of the SSO login flow, taken from the backend configuration.
#[derive(Debug, Clone)]
pub struct SsoLoginConfig {
    /// The UI origin (`allowed_origin`); callbacks redirect there.
    pub ui_origin: String,
    /// `public_base_url`; when `None` the callback URL is derived from the request.
    pub public_base_url: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct SsoAuthorizeQuery {
    /// Local path to open after login. Ignored unless it starts with `/` (and not
    /// `//` or `/\`).
    pub redirect: Option<String>,
}

/// Query of the IdP callback. Deliberately not `Debug`: it carries the
/// authorization code.
#[derive(Deserialize, IntoParams)]
pub struct SsoCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Body of `POST /auth/sso/exchange`. `Debug` redacts the code.
#[derive(Deserialize, ToSchema)]
pub struct SsoExchangeRequest {
    /// The one-time code from `<ui>/auth/sso/complete?code=...`.
    pub code: String,
}

impl std::fmt::Debug for SsoExchangeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoExchangeRequest")
            .field("code", &"<redacted>")
            .finish()
    }
}

/// Cookie that binds an authorization request to the browser that started it
/// (login CSRF protection). Holds base64url(SHA-256(state)), never the state.
pub const STATE_COOKIE: &str = "fluxgate_sso_state";
const STATE_COOKIE_PATH: &str = "/api/v1/auth/sso/";

fn redirect(location: &str, cookie: Option<Cookie<'static>>) -> HttpResponse {
    let mut response = HttpResponse::Found();
    response
        .insert_header((header::LOCATION, location))
        .insert_header((header::CACHE_CONTROL, "no-store"));
    if let Some(cookie) = cookie {
        response.cookie(cookie);
    }
    response.finish()
}

/// base64url(SHA-256(state)), the value of [`STATE_COOKIE`].
fn state_cookie_value(state: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(state.as_bytes()))
}

fn state_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build(STATE_COOKIE, value)
        .path(STATE_COOKIE_PATH)
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(CookieDuration::minutes(LOGIN_STATE_TTL_MINUTES))
        .finish()
}

/// Clears [`STATE_COOKIE`] (same name, path and attributes, expired).
fn clear_state_cookie(secure: bool) -> Cookie<'static> {
    let mut cookie = state_cookie(String::new(), secure);
    cookie.make_removal();
    cookie
}

/// Whether the browser-facing backend URL is https (cookies then get `Secure`).
fn is_https(req: &HttpRequest, config: &SsoLoginConfig) -> bool {
    public_base(req, config).starts_with("https://")
}

/// The backend base URL browsers and the IdP use: `public_base_url`, else the
/// request's scheme and host.
fn public_base(req: &HttpRequest, config: &SsoLoginConfig) -> String {
    match config.public_base_url.as_deref() {
        Some(base) => base.trim_end_matches('/').to_string(),
        None => {
            let info = req.connection_info();
            format!("{}://{}", info.scheme(), info.host())
        }
    }
}

/// Keeps an IdP `error` value printable and short for the log.
fn loggable_idp_error(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect()
}

fn provider_error(err: OidcError) -> SsoLoginError {
    match err {
        OidcError::InvalidToken(detail) => SsoLoginError::TokenInvalid(detail),
        other => SsoLoginError::ProviderError(other.to_string()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/sso/{slug}/authorize",
    params(
        ("slug" = String, Path, description = "Provider slug"),
        SsoAuthorizeQuery
    ),
    responses(
        (status = 302, description = "Redirect to the identity provider and set the HttpOnly `fluxgate_sso_state` cookie (SameSite=Lax, Path=/api/v1/auth/sso/, 10 minutes) that binds the request to this browser; or redirect to `<ui>/login?ssoError=sso_provider_error` when the provider is unknown, disabled or unreachable")
    ),
    security(()),
    tag = "Auth"
)]
#[get("/auth/sso/{slug}/authorize")]
pub(crate) async fn sso_authorize(
    req: HttpRequest,
    slug: web::Path<String>,
    query: web::Query<SsoAuthorizeQuery>,
    db_pool: web::Data<sqlx::PgPool>,
    oidc: web::Data<OidcClient>,
    config: web::Data<SsoLoginConfig>,
) -> impl Responder {
    let slug = slug.into_inner();
    match start_login(
        &req,
        &slug,
        query.redirect.as_deref(),
        &db_pool,
        &oidc,
        &config,
    )
    .await
    {
        Ok((location, state)) => redirect(
            &location,
            Some(state_cookie(
                state_cookie_value(&state),
                is_https(&req, &config),
            )),
        ),
        Err(err) => {
            log::warn!("SSO authorize for provider '{slug}' failed: {err}");
            redirect(&error_url(&config.ui_origin, &err), None)
        }
    }
}

async fn start_login(
    req: &HttpRequest,
    slug: &str,
    redirect_path: Option<&str>,
    pool: &sqlx::PgPool,
    oidc: &OidcClient,
    config: &SsoLoginConfig,
) -> Result<(String, String), SsoLoginError> {
    validate_slug(slug).map_err(|_| SsoLoginError::ProviderError("invalid slug".into()))?;
    let provider = sso_provider_repository(pool.clone())
        .find_provider_by_slug(slug)
        .await?
        .filter(|p| p.enabled)
        .ok_or_else(|| SsoLoginError::ProviderError("provider unknown or disabled".into()))?;
    let metadata = oidc
        .metadata(&provider.issuer_url)
        .await
        .map_err(provider_error)?;

    let state = random_token();
    let nonce = random_token();
    let verifier = random_token();
    let redirect_uri = callback_url(&public_base(req, config), slug);
    let location = authorize_url(
        &metadata.authorization_endpoint,
        &AuthorizeParams {
            client_id: &provider.client_id,
            redirect_uri: &redirect_uri,
            scopes: &provider.scopes,
            state: &state,
            nonce: &nonce,
            code_challenge: &pkce_challenge(&verifier),
        },
    )
    .map_err(SsoLoginError::ProviderError)?;

    sso_login_state_repository(pool.clone())
        .create_state(NewSsoLoginState {
            state_hash: hash_token(&state),
            provider_id: provider.id,
            nonce,
            pkce_verifier: verifier,
            redirect_path: sanitize_redirect(redirect_path),
            expires_at: Utc::now() + Duration::minutes(LOGIN_STATE_TTL_MINUTES),
        })
        .await?;
    Ok((location, state))
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/sso/{slug}/callback",
    params(
        ("slug" = String, Path, description = "Provider slug"),
        SsoCallbackQuery
    ),
    responses(
        (status = 302, description = "Requires the `fluxgate_sso_state` cookie set by authorize (missing or not matching the state gives sso_state_invalid) and always clears it. Redirect to `<ui>/auth/sso/complete?code=<one-time>[&redirect=<path>]`, or to `<ui>/login?ssoError=<code>` with one of sso_state_invalid, sso_provider_error, sso_token_invalid, sso_email_missing, sso_email_domain_not_allowed, sso_user_not_provisioned, sso_linking_not_allowed, sso_account_disabled")
    ),
    security(()),
    tag = "Auth"
)]
#[get("/auth/sso/{slug}/callback")]
pub(crate) async fn sso_callback(
    req: HttpRequest,
    slug: web::Path<String>,
    query: web::Query<SsoCallbackQuery>,
    db_pool: web::Data<sqlx::PgPool>,
    oidc: web::Data<OidcClient>,
    secrets: web::Data<SsoSecrets>,
    config: web::Data<SsoLoginConfig>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
) -> impl Responder {
    let slug = slug.into_inner();
    let result = finish_login(
        &req,
        &slug,
        &query,
        &db_pool,
        &oidc,
        &secrets,
        &config,
        activity_repo.as_ref().as_ref(),
    )
    .await;
    // The state cookie is single use like the state: cleared on every outcome.
    let clear = clear_state_cookie(is_https(&req, &config));
    match result {
        Ok(location) => redirect(&location, Some(clear)),
        Err(err) => {
            log::warn!("SSO callback for provider '{slug}' failed: {err}");
            redirect(&error_url(&config.ui_origin, &err), Some(clear))
        }
    }
}

async fn finish_login(
    req: &HttpRequest,
    slug: &str,
    query: &SsoCallbackQuery,
    pool: &sqlx::PgPool,
    oidc: &OidcClient,
    secrets: &SsoSecrets,
    config: &SsoLoginConfig,
    activity: &dyn ActivityLogRepository,
) -> Result<String, SsoLoginError> {
    // The state is consumed in its own committed statement, before anything else
    // can fail: a replayed callback finds it gone even if this one fails later.
    let state_param = query
        .state
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(SsoLoginError::StateInvalid)?;
    // Login CSRF: the callback must come from the browser that started the
    // request, which holds the hash of this state in its cookie.
    let bound = req.cookie(STATE_COOKIE).is_some_and(|cookie| {
        bool::from(
            cookie
                .value()
                .as_bytes()
                .ct_eq(state_cookie_value(state_param).as_bytes()),
        )
    });
    if !bound {
        return Err(SsoLoginError::StateInvalid);
    }
    let state = sso_login_state_repository(pool.clone())
        .consume_state(&hash_token(state_param))
        .await?
        .ok_or(SsoLoginError::StateInvalid)?;

    let provider = sso_provider_repository(pool.clone())
        .find_provider_by_slug(slug)
        .await?
        .filter(|p| p.id == state.provider_id)
        .ok_or(SsoLoginError::StateInvalid)?;

    if let Some(error) = query.error.as_deref() {
        return Err(SsoLoginError::ProviderError(format!(
            "identity provider returned error '{}'",
            loggable_idp_error(error)
        )));
    }
    if !provider.enabled {
        return Err(SsoLoginError::ProviderError("provider is disabled".into()));
    }
    let code = query
        .code
        .as_deref()
        .filter(|c| !c.is_empty())
        .ok_or_else(|| SsoLoginError::ProviderError("callback without code".into()))?;

    let metadata = oidc
        .metadata(&provider.issuer_url)
        .await
        .map_err(provider_error)?;
    let client_secret = resolve_client_secret(&provider, secrets)
        .map_err(|err| SsoLoginError::ProviderError(format!("client secret unavailable: {err}")))?;
    let redirect_uri = callback_url(&public_base(req, config), slug);
    let tokens = oidc
        .exchange_code(
            &metadata,
            CodeExchange {
                client_id: &provider.client_id,
                client_secret: client_secret.as_deref(),
                code,
                pkce_verifier: &state.pkce_verifier,
                redirect_uri: &redirect_uri,
            },
        )
        .await
        .map_err(provider_error)?;
    let id_token = tokens
        .id_token
        .as_deref()
        .ok_or_else(|| SsoLoginError::ProviderError("no id_token".into()))?;
    let mut claims = oidc
        .validate_id_token(
            &provider.issuer_url,
            id_token,
            &provider.client_id,
            &state.nonce,
        )
        .await
        .map_err(provider_error)?;

    // Userinfo is needed for a missing email, or for groups when the id_token has
    // none and role sync is on.
    let needs_groups = provider.role_sync_mode != "off"
        && !crate::logic::sso_role_sync::has_group_overage(&claims.raw)
        && crate::logic::sso_role_sync::claim_at_path(&claims.raw, &provider.groups_claim)
            .is_none();
    if (claims.email.is_none() || needs_groups)
        && metadata.userinfo_endpoint.is_some()
        && let Some(access_token) = tokens.access_token.as_deref()
    {
        match oidc.userinfo(&metadata, access_token).await {
            Ok(userinfo) => {
                if let Err(err) = claims.merge_userinfo(&userinfo) {
                    log::warn!("SSO userinfo for provider '{slug}' ignored: {err}");
                }
            }
            Err(err) => log::warn!("SSO userinfo for provider '{slug}' failed: {err}"),
        }
    }

    let users = user_repository_tx(pool.clone());
    let identities = user_identity_repository_tx(pool.clone());
    let codes = sso_login_code_repository_tx(pool.clone());
    let roles = role_repository_tx(pool.clone());
    let mappings = sso_group_mapping_repository_tx(pool.clone());
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| SsoLoginError::from(crate::Error::DatabaseError(e)))?;
    let completed = match complete_sso_login_in_tx(
        &mut tx,
        SsoLoginRepos {
            users: &users,
            identities: &identities,
            codes: &codes,
            roles: &roles,
            mappings: &mappings,
            activity,
        },
        &provider,
        &claims,
    )
    .await
    {
        Ok(completed) => completed,
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(err);
        }
    };
    tx.commit()
        .await
        .map_err(|e| SsoLoginError::from(crate::Error::DatabaseError(e)))?;

    Ok(complete_url(
        &config.ui_origin,
        &completed.code,
        state.redirect_path.as_deref(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/sso/exchange",
    request_body = SsoExchangeRequest,
    responses(
        (status = 200, description = "Session issued; same body as POST /api/v1/auth/login", body = LoginResponse),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 401, description = "invalid_sso_code: unknown, expired or already used code, or the user is disabled", body = ErrorResponse)
    ),
    security(()),
    tag = "Auth"
)]
#[post("/auth/sso/exchange")]
pub(crate) async fn sso_exchange(
    db_pool: web::Data<sqlx::PgPool>,
    logic: web::Data<Box<dyn JwtTokenLogic>>,
    payload: web::Json<SsoExchangeRequest>,
) -> Result<impl Responder, RestError> {
    if payload.code.is_empty() {
        return Err(RestError::invalid_sso_code());
    }
    // Single use: the code is marked used in one atomic statement.
    let Some(login_code) = sso_login_code_repository(db_pool.get_ref().clone())
        .consume_code(&hash_token(&payload.code))
        .await?
    else {
        return Err(RestError::invalid_sso_code());
    };

    let user = match user_repository(db_pool.get_ref().clone())
        .get_user_by_id(login_code.user_id)
        .await
    {
        Ok(user) => user,
        Err(crate::Error::NotFound(_)) => return Err(RestError::invalid_sso_code()),
        Err(err) => return Err(RestError::from(err)),
    };
    if !user.enabled {
        return Err(RestError::invalid_sso_code());
    }

    let mut result = logic
        .issue_session(ApiUser::from(user))
        .await
        .map_err(RestError::from)?;
    // The user signed in at the IdP; no local password change is pending here.
    result.is_temporary = false;
    Ok(HttpResponse::Ok().json(login_response(db_pool.get_ref(), result).await?))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(sso_authorize)
        .service(sso_callback)
        .service(sso_exchange);
}
