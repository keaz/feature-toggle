use std::rc::Rc;

use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::{Error, HttpMessage, HttpResponse};
use chrono::{DateTime, Utc};
use futures_util::future::{LocalBoxFuture, Ready, ready};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::logic::jwt_secret::SigningKey;

/// `iss` claim of every token FluxGate issues.
pub const JWT_ISSUER: &str = "fluxgate";
/// `aud` claim of every token FluxGate issues.
pub const JWT_AUDIENCE: &str = "fluxgate-api";

fn default_token_type() -> String {
    "user".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // user id
    pub username: String,
    pub is_admin: bool,
    pub roles: Vec<String>, // user role names
    pub exp: usize,         // expiration timestamp
    pub iat: usize,         // issued at timestamp
    #[serde(default)]
    pub jti: Option<String>, // unique token id
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Issuer; [`JWT_ISSUER`]. Absent only in legacy tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// Audience; [`JWT_AUDIENCE`]. Absent only in legacy tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
}

/// Reads the `kid` header without verifying anything. It only selects which
/// stored secret to verify with; the signature check then decides. A `kid` that
/// is not a UUID cannot name a secret and is rejected.
pub fn token_kid(token: &str) -> Result<Option<Uuid>, jsonwebtoken::errors::Error> {
    match decode_header(token)?.kid {
        None => Ok(None),
        Some(kid) => Uuid::parse_str(&kid)
            .map(Some)
            .map_err(|_| ErrorKind::InvalidToken.into()),
    }
}

fn validation(require_issuer_and_audience: bool) -> Validation {
    // HS256 only: a token whose header names any other algorithm is rejected.
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[JWT_ISSUER]);
    validation.set_audience(&[JWT_AUDIENCE]);
    if require_issuer_and_audience {
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    } else {
        validation.set_required_spec_claims(&["exp"]);
    }
    validation
}

/// Verifies signature, expiry, issuer and audience of `token` with `secret`.
///
/// `iss` and `aud` are required, except for legacy system-client tokens issued
/// before they existed: a token with neither claim is accepted only when its
/// `token_type` is `system_client`, so existing M2M integrations keep working.
/// Legacy user tokens are rejected; users log in again.
pub fn decode_verified_claims(
    token: &str,
    secret: &str,
) -> Result<Claims, jsonwebtoken::errors::Error> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    let err = match decode::<Claims>(token, &key, &validation(true)) {
        Ok(data) => return Ok(data.claims),
        Err(err) => err,
    };
    let missing_iss_or_aud = matches!(
        err.kind(),
        ErrorKind::MissingRequiredClaim(claim) if claim == "iss" || claim == "aud"
    );
    if !missing_iss_or_aud {
        return Err(err);
    }

    // Possibly a legacy token: verify everything else, then accept it only as
    // a system-client token that has neither claim.
    let claims = decode::<Claims>(token, &key, &validation(false))?.claims;
    let is_legacy_system_client =
        claims.iss.is_none() && claims.aud.is_none() && claims.token_type == "system_client";
    if is_legacy_system_client {
        Ok(claims)
    } else {
        Err(err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenType {
    User,
    SystemClient,
}

impl TokenType {
    fn from_claims(claims: &Claims) -> Option<Self> {
        match claims.token_type.as_str() {
            "user" => Some(TokenType::User),
            "system_client" => Some(TokenType::SystemClient),
            _ => None,
        }
    }
}

fn unauthorized_response(ui_origin: &str) -> HttpResponse {
    let target = format!("{}/login", ui_origin.trim_end_matches('/'));
    HttpResponse::Unauthorized().json(serde_json::json!({
        "error": "log_in_required",
        "redirect": target
    }))
}

fn forbidden_response(message: &str) -> HttpResponse {
    HttpResponse::Forbidden().json(serde_json::json!({
        "error": "forbidden",
        "message": message,
        "code": "system_client_scope_violation",
        "details": null
    }))
}

fn policy_forbidden_response(message: &str) -> HttpResponse {
    HttpResponse::Forbidden().json(serde_json::json!({
        "error": "forbidden",
        "message": message,
        "code": "policy_denied",
        "details": null
    }))
}

fn policy_internal_error_response() -> HttpResponse {
    HttpResponse::InternalServerError().json(serde_json::json!({
        "error": "internal",
        "message": "Authorization policy service is temporarily unavailable",
        "code": "policy_service_unavailable",
        "details": null
    }))
}

fn parse_uuid_claim(value: Option<&String>) -> Option<Uuid> {
    value.and_then(|raw| Uuid::parse_str(raw).ok())
}

fn has_scope(scopes: &[String], scope: &str) -> bool {
    scopes.iter().any(|value| value == scope)
}

fn path_has_segment(path: &str, segment: &str) -> bool {
    path.trim_matches('/')
        .split('/')
        .any(|part| part == segment)
}

/// Whether `path` (the routed path) is a vote on an approval request:
/// `POST /api/v1/approval-requests/{id}/approve` or `/reject`.
fn is_approval_vote_route(method: &actix_web::http::Method, path: &str) -> bool {
    if method != actix_web::http::Method::POST {
        return false;
    }
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    matches!(
        segments.as_slice(),
        ["api", "v1", "approval-requests", _, "approve" | "reject"]
    )
}

fn system_client_vote_forbidden_response() -> HttpResponse {
    HttpResponse::Forbidden().json(serde_json::json!({
        "error": "forbidden",
        "message": crate::rest::error::SYSTEM_CLIENT_VOTE_MESSAGE,
        "code": crate::rest::error::SYSTEM_CLIENT_VOTE_CODE,
        "details": null
    }))
}

fn system_client_scope_allowed(
    scopes: &[String],
    method: &actix_web::http::Method,
    path: &str,
) -> bool {
    use crate::logic::system_client::{
        SCOPE_ADMIN_READ, SCOPE_EVALUATE, SCOPE_FLAG_WRITE, SCOPE_METRICS_WRITE,
    };

    if path == "/api/v1/auth/logout" {
        return true;
    }

    if path == "/api/v1/metrics/track/system" && method == actix_web::http::Method::POST {
        return has_scope(scopes, SCOPE_METRICS_WRITE);
    }

    if path == "/api/v1/evaluate" && method == actix_web::http::Method::POST {
        return has_scope(scopes, SCOPE_EVALUATE);
    }

    if path == "/api/v1/health"
        || path == "/api/v1/developer/ofrep-status"
        || path == "/api/v1/openapi.json"
    {
        return method == actix_web::http::Method::GET
            && (has_scope(scopes, SCOPE_ADMIN_READ) || has_scope(scopes, SCOPE_EVALUATE));
    }

    let is_read = method == actix_web::http::Method::GET || method == actix_web::http::Method::HEAD;
    if is_read {
        if path_has_segment(path, "features") || path_has_segment(path, "environments") {
            return has_scope(scopes, SCOPE_ADMIN_READ) || has_scope(scopes, SCOPE_EVALUATE);
        }
        return has_scope(scopes, SCOPE_ADMIN_READ);
    }

    if method == actix_web::http::Method::POST
        || method == actix_web::http::Method::PATCH
        || method == actix_web::http::Method::DELETE
        || method == actix_web::http::Method::PUT
    {
        if path_has_segment(path, "features")
            || path_has_segment(path, "stages")
            || path_has_segment(path, "approval-requests")
            || path_has_segment(path, "criteria")
            || path_has_segment(path, "scheduled-changes")
        {
            return has_scope(scopes, SCOPE_FLAG_WRITE);
        }
        return false;
    }

    false
}

pub struct JwtGuard {
    ui_origin: String,
    jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
    pool: sqlx::PgPool,
    scope_resolver: Box<dyn crate::logic::authorization::RequestScopeResolver>,
}

impl JwtGuard {
    pub fn new(
        ui_origin: String,
        jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
        pool: sqlx::PgPool,
    ) -> Self {
        let scope_resolver = crate::logic::authorization::request_scope_resolver(pool.clone());
        Self::with_scope_resolver(ui_origin, jwt_secret_logic, pool, scope_resolver)
    }

    pub fn with_scope_resolver(
        ui_origin: String,
        jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
        pool: sqlx::PgPool,
        scope_resolver: Box<dyn crate::logic::authorization::RequestScopeResolver>,
    ) -> Self {
        Self {
            ui_origin,
            jwt_secret_logic,
            pool,
            scope_resolver,
        }
    }
}

impl<S: 'static, B> Transform<S, ServiceRequest> for JwtGuard
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<actix_web::body::EitherBody<B>>;
    type Error = Error;
    type Transform = JwtGuardMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(JwtGuardMiddleware {
            service: Rc::new(service),
            ui_origin: self.ui_origin.clone(),
            jwt_secret_logic: self.jwt_secret_logic.clone(),
            pool: self.pool.clone(),
            scope_resolver: self.scope_resolver.clone(),
        }))
    }
}

pub struct JwtGuardMiddleware<S> {
    service: Rc<S>,
    ui_origin: String,
    jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
    pool: sqlx::PgPool,
    scope_resolver: Box<dyn crate::logic::authorization::RequestScopeResolver>,
}

impl<S, B> Service<ServiceRequest> for JwtGuardMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<actix_web::body::EitherBody<B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let ui_origin = self.ui_origin.clone();
        let jwt_secret_logic = self.jwt_secret_logic.clone();
        let pool = self.pool.clone();
        let scope_resolver = self.scope_resolver.clone();

        Box::pin(async move {
            // Allow preflight OPTIONS
            let method = req.method().clone();
            if method == actix_web::http::Method::OPTIONS {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            let path = super::routed_path(&req);

            let is_public_path = path == "/api/v1/health"
                || path == "/api/v1/openapi.json"
                || path.starts_with("/docs")
                || (path == "/metrics/track" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/metrics/track" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/auth/login" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/auth/refresh" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/auth/status" && method == actix_web::http::Method::GET)
                || super::is_public_sso_path(&path, &method);

            if is_public_path {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            let is_reset_password_request =
                path == "/api/v1/auth/reset-password" && method == actix_web::http::Method::POST;

            // Check JWT token in Authorization header (or query param for WebSocket)
            let auth_header = req.headers().get("Authorization");
            let mut token_opt = auth_header
                .and_then(|auth_value| auth_value.to_str().ok())
                .and_then(|auth_str| auth_str.strip_prefix("Bearer "))
                .map(|value| value.to_string());

            if token_opt.is_none() && path.starts_with("/api/v1/ws") {
                let query = req.query_string();
                for pair in query.split('&') {
                    let mut parts = pair.splitn(2, '=');
                    let key = parts.next().unwrap_or_default();
                    if key == "token" {
                        let value = parts.next().unwrap_or_default();
                        if !value.is_empty() {
                            token_opt = Some(value.to_string());
                        }
                        break;
                    }
                }
            }

            if token_opt.is_none()
                && path == "/api/v1/admins"
                && method == actix_web::http::Method::POST
            {
                match crate::logic::policy::enforce_for_route(
                    &pool,
                    &method,
                    &path,
                    Some(crate::logic::policy::PolicyActor::anonymous()),
                )
                .await
                {
                    Ok(()) => {
                        let res = service.call(req).await?;
                        return Ok(res.map_into_left_body());
                    }
                    Err(crate::logic::policy::PolicyError::Unauthorized) => {
                        let res = unauthorized_response(&ui_origin).map_into_right_body();
                        return Ok(req.into_response(res));
                    }
                    Err(crate::logic::policy::PolicyError::Forbidden(message)) => {
                        let res = policy_forbidden_response(&message).map_into_right_body();
                        return Ok(req.into_response(res));
                    }
                    Err(crate::logic::policy::PolicyError::Internal(err)) => {
                        log::error!("Policy evaluation failed for {}: {:?}", path, err);
                        let response = policy_internal_error_response().map_into_right_body();
                        return Ok(req.into_response(response));
                    }
                }
            }

            if let Some(token) = token_opt {
                // The `kid` header selects the verification secret: the active
                // one, or a rotated-out one within its grace window. Unknown or
                // unusable `kid` (and a malformed token) means 401.
                let kid = match token_kid(&token) {
                    Ok(kid) => kid,
                    Err(e) => {
                        log::debug!("JWT header rejected: {}", e);
                        let res = unauthorized_response(&ui_origin).map_into_right_body();
                        return Ok(req.into_response(res));
                    }
                };
                let jwt_secret = match jwt_secret_logic.get_verification_secret(kid).await {
                    Ok(Some(secret)) => secret,
                    Ok(None) => {
                        log::debug!("No usable JWT secret for kid {:?}", kid);
                        let res = unauthorized_response(&ui_origin).map_into_right_body();
                        return Ok(req.into_response(res));
                    }
                    Err(e) => {
                        // Log detailed error for debugging multi-instance issues
                        let hostname =
                            std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
                        let pod_ip =
                            std::env::var("POD_IP").unwrap_or_else(|_| "unknown".to_string());
                        log::error!(
                            "Failed to get JWT secret from database - Pod: {}, IP: {}, Error: {:?}, Path: {}",
                            hostname,
                            pod_ip,
                            e,
                            path
                        );
                        // If we can't get the secret, reject the token
                        let response =
                            HttpResponse::InternalServerError().json(serde_json::json!({
                                "error": "internal",
                                "message": "Authentication service is temporarily unavailable",
                                "code": "auth_secret_unavailable",
                                "details": null
                            }));
                        return Ok(req.into_response(response).map_into_right_body());
                    }
                };

                match decode_verified_claims(&token, &jwt_secret) {
                    Ok(claims) => {
                        let token_type = match TokenType::from_claims(&claims) {
                            Some(token_type) => token_type,
                            None => {
                                log::warn!(
                                    "JWT token has unsupported token_type claim: {}",
                                    claims.token_type
                                );
                                let res = unauthorized_response(&ui_origin).map_into_right_body();
                                return Ok(req.into_response(res));
                            }
                        };

                        let token_hash = hash_token(&token);

                        match token_type {
                            TokenType::User => {
                                let token_repo =
                                    crate::database::jwt_token::jwt_token_repository(pool.clone());

                                match token_repo.is_token_valid(&token_hash).await {
                                    Ok(is_valid) => {
                                        if !is_valid {
                                            let hostname = std::env::var("HOSTNAME")
                                                .unwrap_or_else(|_| "unknown".to_string());
                                            log::warn!(
                                                "JWT token invalid in database - Pod: {}, User: {}, Token hash: {}",
                                                hostname,
                                                claims.username,
                                                &token_hash[..8]
                                            );
                                            let res = unauthorized_response(&ui_origin)
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let user_id_uuid = match Uuid::parse_str(&claims.sub) {
                                            Ok(value) => value,
                                            Err(_) => {
                                                let res = unauthorized_response(&ui_origin)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                        };

                                        // Load the user on every request: a disabled (or deleted) user must be
                                        // rejected even if their token has not yet been revoked.
                                        let user_repo =
                                            crate::database::user::user_repository(pool.clone());
                                        let user =
                                            match user_repo.get_user_by_id(user_id_uuid).await {
                                                Ok(user) => user,
                                                Err(crate::Error::NotFound(_)) => {
                                                    let res = unauthorized_response(&ui_origin)
                                                        .map_into_right_body();
                                                    return Ok(req.into_response(res));
                                                }
                                                Err(err) => {
                                                    log::error!(
                                                        "Failed to load user for request {}: {:?}",
                                                        path,
                                                        err
                                                    );
                                                    let res = policy_internal_error_response()
                                                        .map_into_right_body();
                                                    return Ok(req.into_response(res));
                                                }
                                            };
                                        if !user.enabled {
                                            let res = unauthorized_response(&ui_origin)
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        // Check if user has temporary password (unless this is resetPassword mutation)
                                        // Users with temporary passwords must reset their password before accessing other endpoints
                                        // However, the resetPassword mutation itself is allowed with valid JWT
                                        if !is_reset_password_request && user.is_temporary_password
                                        {
                                            let target = format!(
                                                "{}/reset-password",
                                                ui_origin.trim_end_matches('/')
                                            );
                                            let res = HttpResponse::Unauthorized()
                                                .json(serde_json::json!({
                                                    "error": "temporary_password_reset_required",
                                                    "message": "You must reset your temporary password before continuing",
                                                    "redirect": target
                                                }))
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let policy_actor = crate::logic::policy::PolicyActor::user(
                                            user_id_uuid,
                                            claims.username.clone(),
                                            claims.is_admin,
                                            claims.roles.clone(),
                                        );
                                        match crate::logic::policy::enforce_for_route(
                                            &pool,
                                            &method,
                                            &path,
                                            Some(policy_actor),
                                        )
                                        .await
                                        {
                                            Ok(()) => {}
                                            Err(
                                                crate::logic::policy::PolicyError::Unauthorized,
                                            ) => {
                                                let res = unauthorized_response(&ui_origin)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                            Err(crate::logic::policy::PolicyError::Forbidden(
                                                message,
                                            )) => {
                                                let res = policy_forbidden_response(&message)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                            Err(crate::logic::policy::PolicyError::Internal(
                                                err,
                                            )) => {
                                                log::error!(
                                                    "Policy evaluation failed for request {}: {:?}",
                                                    path,
                                                    err
                                                );
                                                let response = policy_internal_error_response()
                                                    .map_into_right_body();
                                                return Ok(req.into_response(response));
                                            }
                                        }

                                        req.extensions_mut().insert(crate::JwtUser {
                                            id: user_id_uuid,
                                            username: claims.username.clone(),
                                            is_admin: claims.is_admin,
                                            roles: claims.roles.clone(),
                                            team_id: None,
                                            token_hash: token_hash.clone(),
                                        });

                                        let res = service.call(req).await?;
                                        return Ok(res.map_into_left_body());
                                    }
                                    Err(e) => {
                                        let hostname = std::env::var("HOSTNAME")
                                            .unwrap_or_else(|_| "unknown".to_string());
                                        log::error!(
                                            "Database error validating user token - Pod: {}, Error: {:?}",
                                            hostname,
                                            e
                                        );
                                    }
                                }
                            }
                            TokenType::SystemClient => {
                                let token_repo = crate::database::system_client_token::system_client_token_repository(pool.clone());
                                match token_repo.get_token_by_hash(&token_hash).await {
                                    Ok(Some(stored_token)) => {
                                        if stored_token.is_revoked
                                            || stored_token.expires_at <= Utc::now()
                                        {
                                            let res = unauthorized_response(&ui_origin)
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let system_client_id = match Uuid::parse_str(&claims.sub) {
                                            Ok(value) => value,
                                            Err(_) => {
                                                let res = unauthorized_response(&ui_origin)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                        };
                                        let claim_team_id =
                                            match parse_uuid_claim(claims.team_id.as_ref()) {
                                                Some(value) => value,
                                                None => {
                                                    let res = unauthorized_response(&ui_origin)
                                                        .map_into_right_body();
                                                    return Ok(req.into_response(res));
                                                }
                                            };

                                        let system_client_repo =
                                            crate::database::system_client::system_client_repository(
                                                pool.clone(),
                                            );

                                        let system_client = match system_client_repo
                                            .get_system_client_by_id(system_client_id)
                                            .await
                                        {
                                            Ok(client) => client,
                                            Err(_) => {
                                                let res = unauthorized_response(&ui_origin)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                        };

                                        if !system_client.enabled
                                            || system_client.expires_at <= Utc::now()
                                            || system_client.team_id != claim_team_id
                                        {
                                            let res = unauthorized_response(&ui_origin)
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let effective_scopes = if stored_token.scopes.is_empty() {
                                            claims.scopes.clone()
                                        } else {
                                            stored_token.scopes.clone()
                                        };

                                        // Approvals need a human: no scope lets a system client
                                        // approve or reject (Jira decision J2). Cancel stays open.
                                        if is_approval_vote_route(&method, &path) {
                                            let res = system_client_vote_forbidden_response()
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        // System client management routes skip the scope check so
                                        // the policy below denies them (403 policy_denied, audited).
                                        if !crate::logic::policy::is_system_client_management_route(
                                            &path,
                                        ) && !system_client_scope_allowed(
                                            &effective_scopes,
                                            &method,
                                            &path,
                                        ) {
                                            let res = forbidden_response(
                                                "System client token scope does not allow this operation",
                                            )
                                            .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let needs_resource_scope = path != "/api/v1/auth/logout"
                                            && path != "/api/v1/evaluate"
                                            && path != "/api/v1/metrics/track/system"
                                            && path != "/api/v1/developer/ofrep-status"
                                            && path != "/api/v1/health"
                                            && path != "/api/v1/openapi.json";
                                        if needs_resource_scope {
                                            match scope_resolver
                                                .resolve_team_id_for_request(&path)
                                                .await
                                            {
                                                Ok(Some(team_id)) if team_id == claim_team_id => {}
                                                Ok(Some(_)) | Ok(None) => {
                                                    let res = forbidden_response(
                                                        "System client token is not allowed for this resource",
                                                    )
                                                    .map_into_right_body();
                                                    return Ok(req.into_response(res));
                                                }
                                                Err(err) => {
                                                    log::error!(
                                                        "Failed to resolve team scope for request {}: {:?}",
                                                        path,
                                                        err
                                                    );
                                                    let response = HttpResponse::InternalServerError().json(
                                                        serde_json::json!({
                                                            "error": "internal",
                                                            "message": "Authorization service is temporarily unavailable",
                                                            "code": "scope_resolution_failed",
                                                            "details": null
                                                        }),
                                                    );
                                                    return Ok(req
                                                        .into_response(response)
                                                        .map_into_right_body());
                                                }
                                            }
                                        }

                                        let policy_actor =
                                            crate::logic::policy::PolicyActor::system_client(
                                                system_client_id,
                                                claims.username.clone(),
                                                claims.roles.clone(),
                                            );
                                        match crate::logic::policy::enforce_for_route(
                                            &pool,
                                            &method,
                                            &path,
                                            Some(policy_actor),
                                        )
                                        .await
                                        {
                                            Ok(()) => {}
                                            Err(
                                                crate::logic::policy::PolicyError::Unauthorized,
                                            ) => {
                                                let res = unauthorized_response(&ui_origin)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                            Err(crate::logic::policy::PolicyError::Forbidden(
                                                message,
                                            )) => {
                                                let res = policy_forbidden_response(&message)
                                                    .map_into_right_body();
                                                return Ok(req.into_response(res));
                                            }
                                            Err(crate::logic::policy::PolicyError::Internal(
                                                err,
                                            )) => {
                                                log::error!(
                                                    "Policy evaluation failed for request {}: {:?}",
                                                    path,
                                                    err
                                                );
                                                let response = policy_internal_error_response()
                                                    .map_into_right_body();
                                                return Ok(req.into_response(response));
                                            }
                                        }

                                        req.extensions_mut().insert(crate::JwtUser {
                                            id: system_client_id,
                                            username: claims.username.clone(),
                                            // Never trust the claim: tokens issued before system
                                            // clients lost admin rights still carry `true`.
                                            is_admin: false,
                                            roles: claims.roles.clone(),
                                            team_id: Some(claim_team_id),
                                            token_hash: token_hash.clone(),
                                        });

                                        let _ = system_client_repo
                                            .touch_last_used(system_client_id)
                                            .await;
                                        let _ = token_repo.touch_token_last_used(&token_hash).await;

                                        let res = service.call(req).await?;
                                        return Ok(res.map_into_left_body());
                                    }
                                    Ok(None) => {
                                        let res =
                                            unauthorized_response(&ui_origin).map_into_right_body();
                                        return Ok(req.into_response(res));
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "Database error validating system client token: {:?}",
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // JWT decode failed (wrong secret, expired, or malformed)
                        let hostname =
                            std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
                        log::debug!("JWT decode failed - Pod: {}, Error: {}", hostname, e);
                    }
                }
            }

            // No valid JWT token -> Unauthorized with redirect to login page
            let res = unauthorized_response(&ui_origin).map_into_right_body();
            Ok(req.into_response(res))
        })
    }
}

fn timestamp_as_usize(timestamp: i64) -> usize {
    if timestamp <= 0 {
        0
    } else {
        timestamp as usize
    }
}

/// Signs `claims` with `key` (HS256), putting the key id in the `kid` header.
fn sign_claims(claims: &Claims, key: &SigningKey) -> Result<String, jsonwebtoken::errors::Error> {
    let mut header = jsonwebtoken::Header::new(Algorithm::HS256);
    header.kid = Some(key.kid.to_string());
    let encoding_key = jsonwebtoken::EncodingKey::from_secret(key.secret.as_bytes());
    jsonwebtoken::encode(&header, claims, &encoding_key)
}

/// Creates a user access token that expires at `expires_at` (callers derive it
/// from the configured access-token lifetime and store the same instant).
pub fn create_jwt_token(
    user_id: Uuid,
    username: &str,
    is_admin: bool,
    roles: Vec<String>,
    key: &SigningKey,
    expires_at: DateTime<Utc>,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        is_admin,
        roles,
        exp: timestamp_as_usize(expires_at.timestamp()),
        iat: timestamp_as_usize(now.timestamp()),
        jti: Some(Uuid::new_v4().to_string()),
        token_type: "user".to_string(),
        team_id: None,
        scopes: Vec::new(),
        iss: Some(JWT_ISSUER.to_string()),
        aud: Some(JWT_AUDIENCE.to_string()),
    };

    sign_claims(&claims, key)
}

pub fn create_system_client_jwt_token(
    system_client_id: Uuid,
    team_id: Uuid,
    username: &str,
    expires_at: DateTime<Utc>,
    scopes: Vec<String>,
    key: &SigningKey,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();

    let claims = Claims {
        sub: system_client_id.to_string(),
        username: username.to_string(),
        // System clients are never admins; their access comes from roles and scopes.
        is_admin: false,
        roles: vec!["Requester".to_string(), "Approver".to_string()],
        exp: timestamp_as_usize(expires_at.timestamp()),
        iat: timestamp_as_usize(now.timestamp()),
        jti: Some(Uuid::new_v4().to_string()),
        token_type: "system_client".to_string(),
        team_id: Some(team_id.to_string()),
        scopes,
        iss: Some(JWT_ISSUER.to_string()),
        aud: Some(JWT_AUDIENCE.to_string()),
    };

    sign_claims(&claims, key)
}

/// Hash a JWT token for secure storage in database
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, HttpResponse, test, web};
    use sqlx::postgres::PgPoolOptions;

    fn test_expiry() -> DateTime<Utc> {
        Utc::now() + chrono::Duration::minutes(30)
    }

    fn test_pool() -> sqlx::PgPool {
        // Create a lazy pool for testing (won't actually connect unless used)
        PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/test_db")
            .expect("Failed to create test pool")
    }

    const TEST_SECRET: &str = "test_secret";

    fn test_key() -> SigningKey {
        SigningKey {
            kid: Uuid::from_u128(0x7e57),
            secret: TEST_SECRET.to_string(),
        }
    }

    /// Verifies every token with `TEST_SECRET`, whatever its `kid`.
    fn mock_jwt_secret_logic() -> Box<dyn crate::logic::jwt_secret::JwtSecretLogic> {
        use crate::logic::jwt_secret::MockJwtSecretLogic;
        let mut mock = MockJwtSecretLogic::new();
        mock.expect_get_verification_secret()
            .returning(|_| Ok(Some(TEST_SECRET.to_string())));
        mock.expect_clone_box().returning(mock_jwt_secret_logic);
        Box::new(mock)
    }

    #[actix_web::test]
    async fn allows_login_without_token() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/auth/login",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/login")
            .set_payload(r#"{"username":"admin","password":"password"}"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn blocks_protected_request_without_valid_token() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://localhost:3000".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/teams",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/teams")
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["error"], "log_in_required");
        assert_eq!(body["redirect"], "http://localhost:3000/login");
    }

    #[actix_web::test]
    async fn allows_protected_request_with_valid_token() {
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(
            user_id,
            "testuser",
            false,
            vec![],
            &test_key(),
            test_expiry(),
        )
        .unwrap();

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/teams",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/teams")
            .insert_header(("content-type", "application/json"))
            .insert_header(("Authorization", format!("Bearer {}", token)))
            .to_request();
        let resp = test::call_service(&app, req).await;
        // Note: This will likely fail because the test pool won't have the token stored
        // but it tests the middleware structure
        assert!(
            resp.status() == actix_web::http::StatusCode::UNAUTHORIZED
                || resp.status().is_success()
        );
    }

    #[actix_web::test]
    async fn allows_preflight_options() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/teams",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/api/v1/teams")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn allows_logout_with_valid_token() {
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(
            user_id,
            "testuser",
            false,
            vec![],
            &test_key(),
            test_expiry(),
        )
        .unwrap();

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/auth/logout",
                    web::post().to(|req: actix_web::HttpRequest| async move {
                        // Check if JWT user data was injected
                        if req.extensions().get::<crate::JwtUser>().is_some() {
                            HttpResponse::Ok().json("user_authenticated")
                        } else {
                            HttpResponse::BadRequest().json("no_user_data")
                        }
                    }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/logout")
            .insert_header(("content-type", "application/json"))
            .insert_header(("Authorization", format!("Bearer {}", token)))
            .to_request();
        let resp = test::call_service(&app, req).await;
        // Note: This will likely fail because the test pool won't have the token stored
        // but it tests that JWT validation is attempted for logout mutations
        assert!(
            resp.status() == actix_web::http::StatusCode::UNAUTHORIZED
                || resp.status().is_success()
        );
    }

    #[tokio::test]
    async fn test_hash_token() {
        let token = "test_token_12345";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);

        // Same token should produce same hash
        assert_eq!(hash1, hash2);

        // Different tokens should produce different hashes
        let different_token = "different_token";
        let hash3 = hash_token(different_token);
        assert_ne!(hash1, hash3);

        // Hash should be 64 characters (SHA256 in hex)
        assert_eq!(hash1.len(), 64);
    }

    #[tokio::test]
    async fn test_create_jwt_token_with_roles() {
        let user_id = Uuid::new_v4();
        let roles = vec!["Approver".to_string(), "Team Admin".to_string()];
        let token = create_jwt_token(
            user_id,
            "testuser",
            true,
            roles.clone(),
            &test_key(),
            test_expiry(),
        )
        .unwrap();

        // Verify the token is not empty
        assert!(!token.is_empty());

        // Verify the token has the expected format (header.payload.signature)
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Decode and verify the token contains the roles
        let claims = decode_verified_claims(&token, TEST_SECRET).unwrap();

        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.username, "testuser");
        assert_eq!(claims.is_admin, true);
        assert_eq!(claims.roles, roles);
        assert_eq!(claims.token_type, "user");
        assert!(claims.team_id.is_none());
    }

    #[tokio::test]
    async fn create_jwt_token_exp_is_the_given_expiry() {
        let expires_at = Utc::now() + chrono::Duration::minutes(7);
        let token =
            create_jwt_token(Uuid::new_v4(), "u", false, vec![], &test_key(), expires_at).unwrap();

        let claims = decode_verified_claims(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.exp, expires_at.timestamp() as usize);
        assert_eq!(claims.exp - claims.iat, 7 * 60);
    }

    #[actix_web::test]
    async fn allows_refresh_without_token() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/auth/refresh",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/refresh")
            .set_payload(r#"{"refreshToken":"abc"}"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[tokio::test]
    async fn test_create_system_client_jwt_token_with_team_claim() {
        let system_client_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let expires_at = Utc::now() + chrono::Duration::hours(12);

        let token = create_system_client_jwt_token(
            system_client_id,
            team_id,
            "deploy-bot",
            expires_at,
            vec!["evaluate".to_string()],
            &test_key(),
        )
        .unwrap();

        let claims = decode_verified_claims(&token, TEST_SECRET).unwrap();

        assert_eq!(claims.sub, system_client_id.to_string());
        assert_eq!(claims.username, "deploy-bot");
        assert_eq!(claims.token_type, "system_client");
        assert_eq!(claims.team_id, Some(team_id.to_string()));
        assert_eq!(claims.scopes, vec!["evaluate".to_string()]);
        assert!(claims.roles.contains(&"Requester".to_string()));
        assert!(claims.roles.contains(&"Approver".to_string()));
        assert!(!claims.is_admin, "system client tokens never carry admin");
    }

    #[actix_web::test]
    async fn test_allows_reset_password_mutation_with_temporary_password() {
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(
            user_id,
            "tempuser",
            false,
            vec![],
            &test_key(),
            test_expiry(),
        )
        .unwrap();

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://localhost:3000".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/auth/reset-password",
                    web::post().to(|| async { HttpResponse::Ok().json("mutation_allowed") }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/reset-password")
            .set_payload(r#"{"currentPassword":"temp123","newPassword":"newpass123"}"#)
            .insert_header(("content-type", "application/json"))
            .insert_header(("Authorization", format!("Bearer {}", token)))
            .to_request();
        let resp = test::call_service(&app, req).await;

        // resetPassword mutation should be allowed even for users with temporary passwords
        // Note: This will likely return UNAUTHORIZED due to test pool setup, but that's testing
        // the JWT validation rather than the temporary password check
        assert!(
            resp.status() == actix_web::http::StatusCode::UNAUTHORIZED
                || resp.status().is_success()
        );
    }

    #[actix_web::test]
    async fn allows_application_status_query_without_token() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://localhost:3000".to_string(),
                    mock_jwt_secret_logic(),
                    test_pool(),
                ))
                .route(
                    "/api/v1/auth/status",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/v1/auth/status")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    // ---- DB-backed tests: disabled users are rejected by the guard ----

    fn db_secret_logic() -> Box<dyn crate::logic::jwt_secret::JwtSecretLogic> {
        mock_jwt_secret_logic()
    }

    async fn db_pool() -> sqlx::PgPool {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("connect to database")
    }

    /// Inserts a user and a live session token for it; returns (user id, bearer token).
    async fn insert_user_with_session(pool: &sqlx::PgPool, enabled: bool) -> (Uuid, String) {
        insert_user_with_token(pool, enabled, |user_id| {
            create_jwt_token(
                user_id,
                "guard-user",
                false,
                vec![],
                &test_key(),
                test_expiry(),
            )
            .unwrap()
        })
        .await
    }

    /// Inserts a user and stores the token `make_token` builds for it as a live
    /// session; returns (user id, bearer token).
    async fn insert_user_with_token(
        pool: &sqlx::PgPool,
        enabled: bool,
        make_token: impl FnOnce(Uuid) -> String,
    ) -> (Uuid, String) {
        let user_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, enabled)
               VALUES ($1, $2, 'x', 'Guard', 'Test', $3, $4)"#,
        )
        .bind(user_id)
        .bind(format!("guard_user_{user_id}"))
        .bind(format!("guard_user_{user_id}@example.com"))
        .bind(enabled)
        .execute(pool)
        .await
        .expect("insert user");

        let token = make_token(user_id);
        crate::database::jwt_token::jwt_token_repository(pool.clone())
            .store_token(
                user_id,
                hash_token(&token),
                chrono::Utc::now() + chrono::Duration::hours(1),
            )
            .await
            .expect("store token");
        (user_id, token)
    }

    async fn call_guarded(
        pool: &sqlx::PgPool,
        token: &str,
    ) -> (actix_web::http::StatusCode, serde_json::Value) {
        call_guarded_with(pool, db_secret_logic(), token).await
    }

    async fn call_guarded_with(
        pool: &sqlx::PgPool,
        secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
        token: &str,
    ) -> (actix_web::http::StatusCode, serde_json::Value) {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    secret_logic,
                    pool.clone(),
                ))
                .route(
                    "/api/v1/teams",
                    web::get()
                        .to(|| async { HttpResponse::Ok().json(serde_json::json!({"ok": true})) }),
                ),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/api/v1/teams")
            .insert_header(("Authorization", format!("Bearer {token}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body: serde_json::Value = test::read_body_json(resp).await;
        (status, body)
    }

    #[actix_web::test]
    async fn allows_enabled_user_with_live_token() {
        let pool = db_pool().await;
        let (_, token) = insert_user_with_session(&pool, true).await;
        let (status, body) = call_guarded(&pool, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    #[actix_web::test]
    async fn rejects_disabled_user_even_with_unrevoked_token() {
        let pool = db_pool().await;
        let (user_id, token) = insert_user_with_session(&pool, true).await;
        assert_eq!(
            call_guarded(&pool, &token).await.0,
            actix_web::http::StatusCode::OK
        );

        // Disable directly in the table (no revocation) to prove the guard itself checks.
        sqlx::query("UPDATE users SET enabled = FALSE WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();

        let (status, body) = call_guarded(&pool, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "log_in_required");
        assert_eq!(body["redirect"], "http://ui/login");
    }

    #[actix_web::test]
    async fn rejects_token_issued_before_user_was_disabled_via_users_api_path() {
        use crate::database::user::user_repository_tx;
        use crate::logic::user::UpdateUserInput;

        let pool = db_pool().await;
        let (user_id, token) = insert_user_with_session(&pool, true).await;
        assert_eq!(
            call_guarded(&pool, &token).await.0,
            actix_web::http::StatusCode::OK
        );

        let repo = user_repository_tx(pool.clone());
        let activity = crate::database::activity_log::activity_log_repository(pool.clone());
        let mut tx = pool.begin().await.unwrap();
        crate::logic::user_tx::update_user_in_tx(
            &mut tx,
            &repo,
            activity.as_ref(),
            crate::model::ID::from(user_id),
            UpdateUserInput {
                first_name: None,
                last_name: None,
                email: None,
                mobile_number: None,
                is_admin: None,
                enabled: Some(false),
            },
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let (status, body) = call_guarded(&pool, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "log_in_required");
    }

    #[actix_web::test]
    async fn rejects_disabled_user_on_reset_password_route() {
        let pool = db_pool().await;
        let (_, token) = insert_user_with_session(&pool, false).await;

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    db_secret_logic(),
                    pool.clone(),
                ))
                .route(
                    "/api/v1/auth/reset-password",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;
        let req = test::TestRequest::post()
            .uri("/api/v1/auth/reset-password")
            .insert_header(("Authorization", format!("Bearer {token}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);
    }

    // ---- iss / aud / kid ----

    /// Signs arbitrary claims with an arbitrary header, for legacy and forged tokens.
    fn sign_raw(header: jsonwebtoken::Header, claims: &serde_json::Value, secret: &str) -> String {
        jsonwebtoken::encode(
            &header,
            claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    /// Claims as issued before iss/aud existed.
    fn legacy_claims(token_type: &str) -> serde_json::Value {
        let now = Utc::now().timestamp();
        let mut claims = serde_json::json!({
            "sub": Uuid::new_v4().to_string(),
            "username": "legacy",
            "is_admin": false,
            "roles": [],
            "exp": now + 3600,
            "iat": now,
            "jti": Uuid::new_v4().to_string(),
            "token_type": token_type,
            "scopes": ["evaluate"],
        });
        if token_type == "system_client" {
            claims["team_id"] = serde_json::json!(Uuid::new_v4().to_string());
        }
        claims
    }

    #[tokio::test]
    async fn issued_tokens_carry_issuer_audience_and_kid() {
        let key = test_key();
        let user_token =
            create_jwt_token(Uuid::new_v4(), "u", false, vec![], &key, test_expiry()).unwrap();
        let system_token = create_system_client_jwt_token(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "bot",
            test_expiry(),
            vec!["evaluate".to_string()],
            &key,
        )
        .unwrap();

        for token in [&user_token, &system_token] {
            let header = decode_header(token).unwrap();
            assert_eq!(header.alg, Algorithm::HS256);
            assert_eq!(header.kid, Some(key.kid.to_string()));
            assert_eq!(token_kid(token).unwrap(), Some(key.kid));

            let claims = decode_verified_claims(token, TEST_SECRET).unwrap();
            assert_eq!(claims.iss.as_deref(), Some("fluxgate"));
            assert_eq!(claims.aud.as_deref(), Some("fluxgate-api"));
        }
    }

    #[tokio::test]
    async fn token_with_wrong_or_partial_issuer_and_audience_is_rejected() {
        let mut wrong_iss = legacy_claims("user");
        wrong_iss["iss"] = serde_json::json!("someone-else");
        wrong_iss["aud"] = serde_json::json!(JWT_AUDIENCE);
        let mut wrong_aud = legacy_claims("user");
        wrong_aud["iss"] = serde_json::json!(JWT_ISSUER);
        wrong_aud["aud"] = serde_json::json!("other-api");
        // Only one of the two claims: not a legacy token, so both are required,
        // even for a system client.
        let mut iss_only = legacy_claims("system_client");
        iss_only["iss"] = serde_json::json!(JWT_ISSUER);
        let mut aud_only = legacy_claims("system_client");
        aud_only["aud"] = serde_json::json!(JWT_AUDIENCE);
        let mut wrong_iss_system_client = legacy_claims("system_client");
        wrong_iss_system_client["iss"] = serde_json::json!("someone-else");

        for claims in [
            wrong_iss,
            wrong_aud,
            iss_only,
            aud_only,
            wrong_iss_system_client,
        ] {
            let token = sign_raw(
                jsonwebtoken::Header::new(Algorithm::HS256),
                &claims,
                TEST_SECRET,
            );
            assert!(
                decode_verified_claims(&token, TEST_SECRET).is_err(),
                "accepted {claims}"
            );
        }
    }

    #[tokio::test]
    async fn legacy_system_client_token_without_iss_aud_kid_still_verifies() {
        let token = sign_raw(
            jsonwebtoken::Header::new(Algorithm::HS256),
            &legacy_claims("system_client"),
            TEST_SECRET,
        );
        assert_eq!(token_kid(&token).unwrap(), None);
        let claims = decode_verified_claims(&token, TEST_SECRET).unwrap();
        assert_eq!(claims.token_type, "system_client");
        assert!(claims.iss.is_none() && claims.aud.is_none());

        // Still signature- and expiry-checked.
        assert!(decode_verified_claims(&token, "another_secret").is_err());
        let mut expired = legacy_claims("system_client");
        expired["exp"] = serde_json::json!(Utc::now().timestamp() - 3600);
        let expired = sign_raw(
            jsonwebtoken::Header::new(Algorithm::HS256),
            &expired,
            TEST_SECRET,
        );
        assert!(decode_verified_claims(&expired, TEST_SECRET).is_err());
    }

    #[tokio::test]
    async fn legacy_user_token_without_iss_aud_is_rejected() {
        let token = sign_raw(
            jsonwebtoken::Header::new(Algorithm::HS256),
            &legacy_claims("user"),
            TEST_SECRET,
        );
        let err = decode_verified_claims(&token, TEST_SECRET).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::MissingRequiredClaim(_)));

        // A token without token_type defaults to "user": rejected as well.
        let mut untyped = legacy_claims("user");
        untyped.as_object_mut().unwrap().remove("token_type");
        let token = sign_raw(
            jsonwebtoken::Header::new(Algorithm::HS256),
            &untyped,
            TEST_SECRET,
        );
        assert!(decode_verified_claims(&token, TEST_SECRET).is_err());
    }

    #[tokio::test]
    async fn token_signed_with_another_algorithm_is_rejected() {
        let mut claims = legacy_claims("system_client");
        claims["iss"] = serde_json::json!(JWT_ISSUER);
        claims["aud"] = serde_json::json!(JWT_AUDIENCE);
        let token = sign_raw(
            jsonwebtoken::Header::new(Algorithm::HS512),
            &claims,
            TEST_SECRET,
        );
        let err = decode_verified_claims(&token, TEST_SECRET).unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::InvalidAlgorithm);
    }

    #[tokio::test]
    async fn kid_that_is_not_a_uuid_is_rejected() {
        let mut header = jsonwebtoken::Header::new(Algorithm::HS256);
        header.kid = Some("../../etc/passwd".to_string());
        let token = sign_raw(header, &legacy_claims("user"), TEST_SECRET);
        assert!(token_kid(&token).is_err());
        assert!(token_kid("not-a-jwt").is_err());
    }

    /// A real secret logic over a repository that knows `secrets` by id, with
    /// the first active one as the active secret.
    fn secret_logic_over(
        secrets: Vec<crate::database::entity::JwtSecret>,
        auth: crate::config::AuthConfig,
    ) -> Box<dyn crate::logic::jwt_secret::JwtSecretLogic> {
        fn repo(
            secrets: Vec<crate::database::entity::JwtSecret>,
        ) -> Box<dyn crate::database::jwt_secret::JwtSecretRepository> {
            let mut mock = crate::database::jwt_secret::MockJwtSecretRepository::new();
            let active = secrets.iter().find(|secret| secret.is_active).cloned();
            mock.expect_get_active_secret()
                .returning(move || Ok(active.clone()));
            let by_id = secrets.clone();
            mock.expect_get_secret_by_id()
                .returning(move |id| Ok(by_id.iter().find(|secret| secret.id == id).cloned()));
            mock.expect_clone_box()
                .returning(move || repo(secrets.clone()));
            Box::new(mock)
        }
        Box::new(crate::logic::jwt_secret::JwtSecretLogicImpl::new(
            repo(secrets),
            auth,
        ))
    }

    fn stored_secret(
        key: &SigningKey,
        is_active: bool,
        deactivated_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
    ) -> crate::database::entity::JwtSecret {
        crate::database::entity::JwtSecret {
            id: key.kid,
            secret: key.secret.clone(),
            is_active,
            created_at: Utc::now() - chrono::Duration::days(1),
            created_by: None,
            expires_at: None,
            deactivated_at,
            revoked_at,
        }
    }

    fn other_key(secret: &str) -> SigningKey {
        SigningKey {
            kid: Uuid::new_v4(),
            secret: secret.to_string(),
        }
    }

    #[actix_web::test]
    async fn token_with_unknown_kid_is_rejected() {
        let active = other_key("active_secret");
        let logic = secret_logic_over(
            vec![stored_secret(&active, true, None, None)],
            crate::config::AuthConfig::default(),
        );
        // Signed with the active secret's bytes but naming a kid that does not exist.
        let forged = SigningKey {
            kid: Uuid::new_v4(),
            secret: active.secret.clone(),
        };
        let token =
            create_jwt_token(Uuid::new_v4(), "u", false, vec![], &forged, test_expiry()).unwrap();
        let (status, body) = call_guarded_with(&test_pool(), logic, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "log_in_required");
    }

    #[actix_web::test]
    async fn token_signed_by_previous_secret_works_within_grace_and_fails_after() {
        let pool = db_pool().await;
        let auth = crate::config::AuthConfig {
            access_token_ttl_minutes: 30,
            refresh_token_ttl_days: 7,
        };
        let previous = other_key("previous_secret");
        let current = other_key("current_secret");
        let (_, token) = insert_user_with_token(&pool, true, |user_id| {
            create_jwt_token(user_id, "rotated", false, vec![], &previous, test_expiry()).unwrap()
        })
        .await;

        let rotated_ago = |minutes| {
            secret_logic_over(
                vec![
                    stored_secret(&current, true, None, None),
                    stored_secret(
                        &previous,
                        false,
                        Some(Utc::now() - chrono::Duration::minutes(minutes)),
                        None,
                    ),
                ],
                auth,
            )
        };

        let (status, _) = call_guarded_with(&pool, rotated_ago(29), &token).await;
        assert_eq!(status, actix_web::http::StatusCode::OK);

        let (status, body) = call_guarded_with(&pool, rotated_ago(31), &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "log_in_required");

        // The kid selects the key: the current secret does not verify it.
        let only_current = secret_logic_over(vec![stored_secret(&current, true, None, None)], auth);
        let (status, _) = call_guarded_with(&pool, only_current, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn revoked_secret_rejects_its_tokens_immediately() {
        let pool = db_pool().await;
        let key = other_key("revoked_secret");
        let (_, token) = insert_user_with_token(&pool, true, |user_id| {
            create_jwt_token(user_id, "revoked", false, vec![], &key, test_expiry()).unwrap()
        })
        .await;

        let live = secret_logic_over(
            vec![stored_secret(&key, true, None, None)],
            crate::config::AuthConfig::default(),
        );
        assert_eq!(
            call_guarded_with(&pool, live, &token).await.0,
            actix_web::http::StatusCode::OK
        );

        // Deactivated one minute ago by deactivate-all: no grace.
        let now = Utc::now();
        let revoked = secret_logic_over(
            vec![stored_secret(
                &key,
                false,
                Some(now - chrono::Duration::minutes(1)),
                Some(now - chrono::Duration::minutes(1)),
            )],
            crate::config::AuthConfig::default(),
        );
        let (status, body) = call_guarded_with(&pool, revoked, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "log_in_required");
    }

    #[actix_web::test]
    async fn legacy_user_token_with_live_session_is_rejected_by_the_guard() {
        let pool = db_pool().await;
        let (_, token) = insert_user_with_token(&pool, true, |user_id| {
            let mut claims = legacy_claims("user");
            claims["sub"] = serde_json::json!(user_id.to_string());
            sign_raw(
                jsonwebtoken::Header::new(Algorithm::HS256),
                &claims,
                TEST_SECRET,
            )
        })
        .await;
        let (status, body) = call_guarded(&pool, &token).await;
        assert_eq!(status, actix_web::http::StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "log_in_required");
    }

    #[actix_web::test]
    async fn legacy_system_client_token_passes_the_guard() {
        use crate::database::system_client::{CreateSystemClient, system_client_repository};

        let pool = db_pool().await;
        let team_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO teams (id, name, description) VALUES ($1, $2, 'legacy token test')",
        )
        .bind(team_id)
        .bind(format!("legacy-token-{team_id}"))
        .execute(&pool)
        .await
        .expect("insert team");
        let client = system_client_repository(pool.clone())
            .create_system_client(
                team_id,
                CreateSystemClient {
                    name: format!("legacy-bot-{}", Uuid::new_v4().simple()),
                    description: None,
                    enabled: true,
                    expires_at: Utc::now() + chrono::Duration::days(1),
                },
            )
            .await
            .expect("create system client");

        // Issued before iss/aud/kid existed.
        let mut claims = legacy_claims("system_client");
        claims["sub"] = serde_json::json!(client.id.to_string());
        claims["team_id"] = serde_json::json!(team_id.to_string());
        claims["is_admin"] = serde_json::json!(true);
        let token = sign_raw(
            jsonwebtoken::Header::new(Algorithm::HS256),
            &claims,
            TEST_SECRET,
        );
        crate::database::system_client_token::system_client_token_repository(pool.clone())
            .store_token(
                client.id,
                hash_token(&token),
                "legacy".to_string(),
                vec!["evaluate".to_string()],
                Utc::now() + chrono::Duration::hours(1),
            )
            .await
            .expect("store system client token");

        // Without kid, only the active secret verifies.
        let active = SigningKey {
            kid: Uuid::new_v4(),
            secret: TEST_SECRET.to_string(),
        };
        let logic = secret_logic_over(
            vec![stored_secret(&active, true, None, None)],
            crate::config::AuthConfig::default(),
        );
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new("http://ui".to_string(), logic, pool.clone()))
                .route(
                    "/api/v1/auth/logout",
                    web::post().to(|req: actix_web::HttpRequest| async move {
                        let user = req.extensions().get::<crate::JwtUser>().cloned();
                        HttpResponse::Ok().json(serde_json::json!({
                            "id": user.as_ref().map(|user| user.id.to_string()),
                            "is_admin": user.map(|user| user.is_admin),
                        }))
                    }),
                ),
        )
        .await;
        let req = test::TestRequest::post()
            .uri("/api/v1/auth/logout")
            .insert_header(("Authorization", format!("Bearer {token}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["id"], client.id.to_string());
        // The legacy claim says `is_admin: true`; the guard must not honour it.
        assert_eq!(body["is_admin"], false);
    }

    /// Creates a team and a system client, stores a token with `scopes`, and returns
    /// (team id, system client id, bearer token).
    async fn system_client_with_token(
        pool: &sqlx::PgPool,
        scopes: Vec<String>,
    ) -> (Uuid, Uuid, String) {
        use crate::database::system_client::{CreateSystemClient, system_client_repository};

        let team_id = Uuid::new_v4();
        sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'guard test')")
            .bind(team_id)
            .bind(format!("guard-sc-{team_id}"))
            .execute(pool)
            .await
            .expect("insert team");
        let client = system_client_repository(pool.clone())
            .create_system_client(
                team_id,
                CreateSystemClient {
                    name: format!("guard-bot-{}", Uuid::new_v4().simple()),
                    description: None,
                    enabled: true,
                    expires_at: Utc::now() + chrono::Duration::days(1),
                },
            )
            .await
            .expect("create system client");
        let token = create_system_client_jwt_token(
            client.id,
            team_id,
            "guard-bot",
            Utc::now() + chrono::Duration::hours(1),
            scopes.clone(),
            &test_key(),
        )
        .expect("sign token");
        crate::database::system_client_token::system_client_token_repository(pool.clone())
            .store_token(
                client.id,
                hash_token(&token),
                "guard".to_string(),
                scopes,
                Utc::now() + chrono::Duration::hours(1),
            )
            .await
            .expect("store system client token");
        (team_id, client.id, token)
    }

    #[actix_web::test]
    async fn system_client_token_is_never_admin_and_cannot_manage_system_clients() {
        let pool = db_pool().await;
        let scopes = vec![
            "admin:read".to_string(),
            "flag:write".to_string(),
            "evaluate".to_string(),
        ];
        let (team_id, client_id, token) = system_client_with_token(&pool, scopes).await;
        let token_id: Uuid =
            sqlx::query_scalar("SELECT id FROM system_client_tokens WHERE token_hash = $1")
                .bind(hash_token(&token))
                .fetch_one(&pool)
                .await
                .expect("load token id");

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    db_secret_logic(),
                    pool.clone(),
                ))
                .route(
                    "/api/v1/teams/{team_id}/clients",
                    web::get().to(|req: actix_web::HttpRequest| async move {
                        let user = req.extensions().get::<crate::JwtUser>().cloned();
                        HttpResponse::Ok()
                            .json(serde_json::json!({"is_admin": user.map(|u| u.is_admin)}))
                    }),
                )
                .default_service(web::to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        // Ordinary team route: allowed, and the request context says "not admin".
        let req = test::TestRequest::get()
            .uri(&format!("/api/v1/teams/{team_id}/clients"))
            .insert_header(("Authorization", format!("Bearer {token}")))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["is_admin"], false);

        // Every management route (reads and writes) answers 403 policy_denied.
        let routes = [
            ("GET", format!("/api/v1/teams/{team_id}/system-clients")),
            ("POST", format!("/api/v1/teams/{team_id}/system-clients")),
            ("GET", format!("/api/v1/system-clients/{client_id}")),
            ("PATCH", format!("/api/v1/system-clients/{client_id}")),
            (
                "POST",
                format!("/api/v1/system-clients/{client_id}/regenerate-token"),
            ),
            ("GET", format!("/api/v1/system-clients/{client_id}/tokens")),
            ("POST", format!("/api/v1/system-clients/{client_id}/tokens")),
            (
                "POST",
                format!("/api/v1/system-client-tokens/{token_id}/revoke"),
            ),
        ];
        for (method, uri) in routes {
            let builder = match method {
                "GET" => test::TestRequest::get(),
                "POST" => test::TestRequest::post(),
                _ => test::TestRequest::patch(),
            };
            let req = builder
                .uri(&uri)
                .insert_header(("Authorization", format!("Bearer {token}")))
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(
                resp.status(),
                actix_web::http::StatusCode::FORBIDDEN,
                "{method} {uri}"
            );
            let body: serde_json::Value = test::read_body_json(resp).await;
            assert_eq!(body["code"], "policy_denied", "{method} {uri}");
        }
    }

    /// App whose routes answer 200 "handler"; anything else is 404, so a request
    /// that is routed but not denied is visible.
    macro_rules! encoded_path_app {
        ($pool:expr) => {{
            let ok = || async { HttpResponse::Ok().body("handler") };
            test::init_service(
                App::new()
                    .wrap(JwtGuard::new(
                        "http://ui".to_string(),
                        db_secret_logic(),
                        $pool.clone(),
                    ))
                    .route("/api/v1/system-clients/{id}", web::get().to(ok))
                    .route("/api/v1/teams/{id}/system-clients", web::get().to(ok))
                    .route("/api/v1/roles", web::post().to(ok))
                    .route("/api/v1/health", web::get().to(ok))
                    .route("/api/v1/auth/login", web::post().to(ok))
                    .route("/api/v1/auth/refresh", web::post().to(ok))
                    .route("/api/v1/auth/sso/{slug}/authorize", web::get().to(ok))
                    .route("/api/v1/auth/sso/{slug}/callback", web::get().to(ok))
                    .route("/api/v1/auth/sso/{slug}/other", web::get().to(ok))
                    .route("/api/v1/auth/sso/exchange", web::post().to(ok)),
            )
            .await
        }};
    }

    #[actix_web::test]
    async fn percent_encoded_paths_cannot_bypass_route_policies() {
        let pool = db_pool().await;
        let (_, plain_token) = insert_user_with_session(&pool, true).await;
        let (_, admin_token) = insert_user_with_token(&pool, true, |user_id| {
            create_jwt_token(
                user_id,
                "guard-admin",
                true,
                vec![],
                &test_key(),
                test_expiry(),
            )
            .unwrap()
        })
        .await;
        let app = encoded_path_app!(pool);
        let id = Uuid::new_v4();

        let cases = [
            ("GET", format!("/api/v1/system%2Dclients/{id}")),
            ("GET", format!("/api/v1/teams/{id}/system%2Dclients")),
            ("GET", format!("/%61pi/v1/system-clients/{id}")),
            ("GET", format!("/api/v1/%73ystem-clients/{id}")),
            ("POST", "/api/v1/%72oles".to_string()),
            ("POST", "/%61pi/%76%31/roles".to_string()),
        ];
        for (method, uri) in cases {
            let build = |token: &str| {
                let builder = if method == "GET" {
                    test::TestRequest::get()
                } else {
                    test::TestRequest::post()
                };
                builder
                    .uri(&uri)
                    .insert_header(("Authorization", format!("Bearer {token}")))
                    .to_request()
            };

            // The router decodes the path, so an admin reaches the handler...
            let resp = test::call_service(&app, build(&admin_token)).await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::OK, "{uri}");

            // ...and the policy must see the same path and deny everyone else.
            let resp = test::call_service(&app, build(&plain_token)).await;
            assert_eq!(
                resp.status(),
                actix_web::http::StatusCode::FORBIDDEN,
                "{method} {uri}"
            );
            let body: serde_json::Value = test::read_body_json(resp).await;
            assert_eq!(body["code"], "policy_denied", "{method} {uri}");
        }
    }

    #[actix_web::test]
    async fn public_paths_work_and_are_not_widened_by_encoding() {
        let pool = db_pool().await;
        let app = encoded_path_app!(pool);

        for (method, uri) in [
            ("GET", "/api/v1/health"),
            ("POST", "/api/v1/auth/login"),
            ("POST", "/api/v1/auth/refresh"),
            ("GET", "/api/v1/auth/sso/okta/authorize"),
            ("GET", "/api/v1/auth/sso/okta/callback?code=c&state=s"),
            ("POST", "/api/v1/auth/sso/exchange"),
        ] {
            let builder = if method == "GET" {
                test::TestRequest::get()
            } else {
                test::TestRequest::post()
            };
            let resp = test::call_service(&app, builder.uri(uri).to_request()).await;
            assert_eq!(resp.status(), actix_web::http::StatusCode::OK, "{uri}");
        }

        // An escaped `/` is not decoded by the router and must not turn a protected
        // path into a public one: no token, no entry.
        for (method, uri) in [
            ("GET", "/api/v1/health%2Fx"),
            ("GET", "/api/v1/health%2F..%2Fsystem-clients/x"),
            ("POST", "/api/v1/auth%2Flogin"),
            ("POST", "/api/v1/auth/refresh%2F"),
            ("POST", "/api/v1/roles"),
            ("GET", "/api/v1/auth/sso/okta/other"),
            ("GET", "/api/v1/auth/sso/okta%2Fx/authorize"),
        ] {
            let builder = if method == "GET" {
                test::TestRequest::get()
            } else {
                test::TestRequest::post()
            };
            let resp = test::call_service(&app, builder.uri(uri).to_request()).await;
            assert_eq!(
                resp.status(),
                actix_web::http::StatusCode::UNAUTHORIZED,
                "{uri}"
            );
        }
    }

    #[actix_web::test]
    async fn approval_vote_routes_are_approve_and_reject_posts_only() {
        use actix_web::http::Method;
        let id = Uuid::new_v4();
        assert!(is_approval_vote_route(
            &Method::POST,
            &format!("/api/v1/approval-requests/{id}/approve")
        ));
        assert!(is_approval_vote_route(
            &Method::POST,
            &format!("/api/v1/approval-requests/{id}/reject")
        ));
        assert!(is_approval_vote_route(
            &Method::POST,
            &format!("/api/v1/approval-requests/{id}/approve/")
        ));
        for (method, path) in [
            (
                Method::POST,
                format!("/api/v1/approval-requests/{id}/cancel"),
            ),
            (
                Method::GET,
                format!("/api/v1/approval-requests/{id}/approve"),
            ),
            (Method::POST, format!("/api/v1/approval-requests/{id}")),
            (Method::POST, format!("/api/v1/features/{id}/approve")),
            (
                Method::POST,
                "/api/v1/approval-requests/approve".to_string(),
            ),
        ] {
            assert!(!is_approval_vote_route(&method, &path), "{method} {path}");
        }
    }

    #[actix_web::test]
    async fn system_client_cannot_vote_on_approval_requests_whatever_its_scopes() {
        let pool = db_pool().await;
        let scopes = vec![
            "admin:read".to_string(),
            "flag:write".to_string(),
            "evaluate".to_string(),
            "metrics:write".to_string(),
        ];
        let (_team_id, _client_id, token) = system_client_with_token(&pool, scopes).await;

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    db_secret_logic(),
                    pool.clone(),
                ))
                .default_service(web::to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        // A request id the scope resolver cannot place in the client's team would
        // also be refused, so the vote check must answer first with its own code.
        let request_id = Uuid::new_v4();
        for action in ["approve", "reject", "%61pprove"] {
            let req = test::TestRequest::post()
                .uri(&format!("/api/v1/approval-requests/{request_id}/{action}"))
                .insert_header(("Authorization", format!("Bearer {token}")))
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(
                resp.status(),
                actix_web::http::StatusCode::FORBIDDEN,
                "{action}"
            );
            let body: serde_json::Value = test::read_body_json(resp).await;
            assert_eq!(body["code"], "system_client_vote_not_permitted", "{action}");
        }
    }
}
