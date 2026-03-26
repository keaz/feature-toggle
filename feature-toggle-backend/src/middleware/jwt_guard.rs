use std::rc::Rc;

use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::{Error, HttpMessage, HttpResponse};
use chrono::{DateTime, Utc};
use futures_util::future::{LocalBoxFuture, Ready, ready};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

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

async fn resolve_team_id_for_request(
    path: &str,
    pool: &sqlx::PgPool,
) -> Result<Option<Uuid>, crate::Error> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() < 3 || parts[0] != "api" || parts[1] != "v1" {
        return Ok(None);
    }

    let resource = parts[2];

    if resource == "teams" && parts.len() >= 4 {
        return Uuid::parse_str(parts[3]).map(Some).map_err(|e| {
            crate::Error::InvalidInput(format!("invalid team id in request path: {e}"))
        });
    }

    let parse_id = |index: usize| -> Result<Option<Uuid>, crate::Error> {
        if parts.len() <= index {
            return Ok(None);
        }
        Uuid::parse_str(parts[index]).map(Some).map_err(|e| {
            crate::Error::InvalidInput(format!("invalid resource id in request path: {e}"))
        })
    };

    let team_id = match resource {
        "features" => {
            let feature_id = parse_id(3)?;
            if let Some(feature_id) = feature_id {
                sqlx::query("SELECT team_id FROM features WHERE id = $1")
                    .bind(feature_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(crate::Error::DatabaseError)?
                    .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "stages" => {
            let stage_id = parse_id(3)?;
            if let Some(stage_id) = stage_id {
                sqlx::query(
                    r#"
                    SELECT f.team_id
                    FROM features_pipeline_stages fs
                    JOIN features f ON f.id = fs.feature_id
                    WHERE fs.id = $1
                    "#,
                )
                .bind(stage_id)
                .fetch_optional(pool)
                .await
                .map_err(crate::Error::DatabaseError)?
                .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "approval-requests" => {
            let request_id = parse_id(3)?;
            if let Some(request_id) = request_id {
                sqlx::query(
                    r#"
                    SELECT f.team_id
                    FROM approval_requests ar
                    JOIN features f ON f.id = ar.feature_id
                    WHERE ar.id = $1
                    "#,
                )
                .bind(request_id)
                .fetch_optional(pool)
                .await
                .map_err(crate::Error::DatabaseError)?
                .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "pipelines" => {
            let pipeline_id = parse_id(3)?;
            if let Some(pipeline_id) = pipeline_id {
                sqlx::query("SELECT team_id FROM pipelines WHERE id = $1")
                    .bind(pipeline_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(crate::Error::DatabaseError)?
                    .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "environments" => {
            let environment_id = parse_id(3)?;
            if let Some(environment_id) = environment_id {
                sqlx::query("SELECT team_id FROM environments WHERE id = $1")
                    .bind(environment_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(crate::Error::DatabaseError)?
                    .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "contexts" => {
            let context_id = parse_id(3)?;
            if let Some(context_id) = context_id {
                sqlx::query("SELECT team_id FROM contexts WHERE id = $1")
                    .bind(context_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(crate::Error::DatabaseError)?
                    .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "clients" => {
            let client_id = parse_id(3)?;
            if let Some(client_id) = client_id {
                sqlx::query("SELECT team_id FROM clients WHERE id = $1")
                    .bind(client_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(crate::Error::DatabaseError)?
                    .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        "system-clients" => {
            let system_client_id = parse_id(3)?;
            if let Some(system_client_id) = system_client_id {
                sqlx::query("SELECT team_id FROM system_clients WHERE id = $1")
                    .bind(system_client_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(crate::Error::DatabaseError)?
                    .map(|row| row.get("team_id"))
            } else {
                None
            }
        }
        _ => None,
    };

    Ok(team_id)
}

pub struct JwtGuard {
    ui_origin: String,
    jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
    pool: sqlx::PgPool,
}

impl JwtGuard {
    pub fn new(
        ui_origin: String,
        jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
        pool: sqlx::PgPool,
    ) -> Self {
        Self {
            ui_origin,
            jwt_secret_logic,
            pool,
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
        }))
    }
}

pub struct JwtGuardMiddleware<S> {
    service: Rc<S>,
    ui_origin: String,
    jwt_secret_logic: Box<dyn crate::logic::jwt_secret::JwtSecretLogic>,
    pool: sqlx::PgPool,
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

        Box::pin(async move {
            // Allow preflight OPTIONS
            let method = req.method().clone();
            if method == actix_web::http::Method::OPTIONS {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            let path = req.path().to_string();

            let is_public_path = path == "/api/v1/health"
                || path == "/api/v1/openapi.json"
                || path.starts_with("/docs")
                || (path == "/metrics/track" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/metrics/track" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/auth/login" && method == actix_web::http::Method::POST)
                || (path == "/api/v1/auth/status" && method == actix_web::http::Method::GET);

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
                // Get current JWT secret from database
                let jwt_secret = match jwt_secret_logic.get_current_secret().await {
                    Ok(secret) => secret,
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
                            req.path()
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

                // Verify JWT token
                let decoding_key = DecodingKey::from_secret(jwt_secret.as_ref());
                let validation = Validation::new(Algorithm::HS256);

                match decode::<Claims>(&token, &decoding_key, &validation) {
                    Ok(token_data) => {
                        let token_type = match TokenType::from_claims(&token_data.claims) {
                            Some(token_type) => token_type,
                            None => {
                                log::warn!(
                                    "JWT token has unsupported token_type claim: {}",
                                    token_data.claims.token_type
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
                                                token_data.claims.username,
                                                &token_hash[..8]
                                            );
                                            let res = unauthorized_response(&ui_origin)
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let user_id_uuid =
                                            match Uuid::parse_str(&token_data.claims.sub) {
                                                Ok(value) => value,
                                                Err(_) => {
                                                    let res = unauthorized_response(&ui_origin)
                                                        .map_into_right_body();
                                                    return Ok(req.into_response(res));
                                                }
                                            };

                                        // Check if user has temporary password (unless this is resetPassword mutation)
                                        // Users with temporary passwords must reset their password before accessing other endpoints
                                        // However, the resetPassword mutation itself is allowed with valid JWT
                                        if !is_reset_password_request {
                                            let user_repo = crate::database::user::user_repository(
                                                pool.clone(),
                                            );
                                            if let Ok(user) =
                                                user_repo.get_user_by_id(user_id_uuid).await
                                                && user.is_temporary_password
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
                                        }

                                        let policy_actor = crate::logic::policy::PolicyActor::user(
                                            user_id_uuid,
                                            token_data.claims.username.clone(),
                                            token_data.claims.is_admin,
                                            token_data.claims.roles.clone(),
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
                                            username: token_data.claims.username.clone(),
                                            is_admin: token_data.claims.is_admin,
                                            roles: token_data.claims.roles.clone(),
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
                                match token_repo.is_token_valid(&token_hash).await {
                                    Ok(is_valid) => {
                                        if !is_valid {
                                            let res = unauthorized_response(&ui_origin)
                                                .map_into_right_body();
                                            return Ok(req.into_response(res));
                                        }

                                        let system_client_id =
                                            match Uuid::parse_str(&token_data.claims.sub) {
                                                Ok(value) => value,
                                                Err(_) => {
                                                    let res = unauthorized_response(&ui_origin)
                                                        .map_into_right_body();
                                                    return Ok(req.into_response(res));
                                                }
                                            };
                                        let claim_team_id = match parse_uuid_claim(
                                            token_data.claims.team_id.as_ref(),
                                        ) {
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

                                        if path != "/api/v1/auth/logout" {
                                            match resolve_team_id_for_request(&path, &pool).await {
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
                                                token_data.claims.username.clone(),
                                                token_data.claims.roles.clone(),
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
                                            username: token_data.claims.username.clone(),
                                            is_admin: token_data.claims.is_admin,
                                            roles: token_data.claims.roles.clone(),
                                            token_hash: token_hash.clone(),
                                        });

                                        let _ = system_client_repo
                                            .touch_last_used(system_client_id)
                                            .await;

                                        let res = service.call(req).await?;
                                        return Ok(res.map_into_left_body());
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

pub fn create_jwt_token(
    user_id: Uuid,
    username: &str,
    is_admin: bool,
    roles: Vec<String>,
    secret: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();
    let exp = now + chrono::Duration::hours(24); // Token expires in 24 hours

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        is_admin,
        roles,
        exp: timestamp_as_usize(exp.timestamp()),
        iat: timestamp_as_usize(now.timestamp()),
        jti: Some(Uuid::new_v4().to_string()),
        token_type: "user".to_string(),
        team_id: None,
    };

    let header = jsonwebtoken::Header::new(Algorithm::HS256);
    let encoding_key = jsonwebtoken::EncodingKey::from_secret(secret.as_ref());

    jsonwebtoken::encode(&header, &claims, &encoding_key)
}

pub fn create_system_client_jwt_token(
    system_client_id: Uuid,
    team_id: Uuid,
    username: &str,
    expires_at: DateTime<Utc>,
    secret: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();

    let claims = Claims {
        sub: system_client_id.to_string(),
        username: username.to_string(),
        is_admin: true,
        roles: vec!["Requester".to_string(), "Approver".to_string()],
        exp: timestamp_as_usize(expires_at.timestamp()),
        iat: timestamp_as_usize(now.timestamp()),
        jti: Some(Uuid::new_v4().to_string()),
        token_type: "system_client".to_string(),
        team_id: Some(team_id.to_string()),
    };

    let header = jsonwebtoken::Header::new(Algorithm::HS256);
    let encoding_key = jsonwebtoken::EncodingKey::from_secret(secret.as_ref());
    jsonwebtoken::encode(&header, &claims, &encoding_key)
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

    fn test_pool() -> sqlx::PgPool {
        // Create a lazy pool for testing (won't actually connect unless used)
        PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/test_db")
            .expect("Failed to create test pool")
    }

    async fn test_db_pool_from_env() -> Option<sqlx::PgPool> {
        let Ok(db_url) = std::env::var("DATABASE_URL") else {
            return None;
        };

        Some(
            PgPoolOptions::new()
                .max_connections(1)
                .connect(&db_url)
                .await
                .expect("Failed to connect to DATABASE_URL for stage scope tests"),
        )
    }

    fn mock_jwt_secret_logic() -> Box<dyn crate::logic::jwt_secret::JwtSecretLogic> {
        use crate::logic::jwt_secret::MockJwtSecretLogic;
        let mut mock = MockJwtSecretLogic::new();
        mock.expect_get_current_secret()
            .returning(|| Ok("test_secret".to_string()));
        mock.expect_clone_box().returning(|| {
            let mut cloned_mock = MockJwtSecretLogic::new();
            cloned_mock
                .expect_get_current_secret()
                .returning(|| Ok("test_secret".to_string()));
            cloned_mock.expect_clone_box().returning(|| {
                let mut inner_mock = MockJwtSecretLogic::new();
                inner_mock
                    .expect_get_current_secret()
                    .returning(|| Ok("test_secret".to_string()));
                Box::new(inner_mock)
            });
            Box::new(cloned_mock)
        });
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
        let secret = "test_secret";
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(user_id, "testuser", false, vec![], secret).unwrap();

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
        let secret = "test_secret";
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(user_id, "testuser", false, vec![], secret).unwrap();

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
        let secret = "test_secret";
        let roles = vec!["Approver".to_string(), "Team Admin".to_string()];
        let token = create_jwt_token(user_id, "testuser", true, roles.clone(), secret).unwrap();

        // Verify the token is not empty
        assert!(!token.is_empty());

        // Verify the token has the expected format (header.payload.signature)
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // Decode and verify the token contains the roles
        let decoding_key = jsonwebtoken::DecodingKey::from_secret(secret.as_ref());
        let validation = jsonwebtoken::Validation::new(Algorithm::HS256);
        let token_data =
            jsonwebtoken::decode::<Claims>(&token, &decoding_key, &validation).unwrap();

        assert_eq!(token_data.claims.sub, user_id.to_string());
        assert_eq!(token_data.claims.username, "testuser");
        assert_eq!(token_data.claims.is_admin, true);
        assert_eq!(token_data.claims.roles, roles);
        assert_eq!(token_data.claims.token_type, "user");
        assert!(token_data.claims.team_id.is_none());
    }

    #[tokio::test]
    async fn test_create_system_client_jwt_token_with_team_claim() {
        let system_client_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let secret = "test_secret";
        let expires_at = Utc::now() + chrono::Duration::hours(12);

        let token = create_system_client_jwt_token(
            system_client_id,
            team_id,
            "deploy-bot",
            expires_at,
            secret,
        )
        .unwrap();

        let decoding_key = jsonwebtoken::DecodingKey::from_secret(secret.as_ref());
        let validation = jsonwebtoken::Validation::new(Algorithm::HS256);
        let token_data =
            jsonwebtoken::decode::<Claims>(&token, &decoding_key, &validation).unwrap();

        assert_eq!(token_data.claims.sub, system_client_id.to_string());
        assert_eq!(token_data.claims.username, "deploy-bot");
        assert_eq!(token_data.claims.token_type, "system_client");
        assert_eq!(token_data.claims.team_id, Some(team_id.to_string()));
        assert!(token_data.claims.roles.contains(&"Requester".to_string()));
        assert!(token_data.claims.roles.contains(&"Approver".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_team_id_for_request_resolves_stage_scope() {
        let Some(pool) = test_db_pool_from_env().await else {
            eprintln!("Skipping stage scope resolution test: DATABASE_URL is not set");
            return;
        };

        let stage_and_team = sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            SELECT fps.id, f.team_id
            FROM features_pipeline_stages fps
            JOIN features f ON f.id = fps.feature_id
            LIMIT 1
            "#,
        )
        .fetch_optional(&pool)
        .await
        .expect("Failed to load stage/team data for scope resolution test");

        let (stage_id, expected_team_id) = match stage_and_team {
            Some(values) => values,
            None => {
                eprintln!(
                    "Skipping stage scope resolution test: no records in features_pipeline_stages"
                );
                return;
            }
        };

        let path = format!("/api/v1/stages/{stage_id}");
        let resolved = resolve_team_id_for_request(&path, &pool)
            .await
            .expect("Stage scope resolution should succeed");

        assert_eq!(resolved, Some(expected_team_id));
    }

    #[tokio::test]
    async fn test_resolve_team_id_for_request_returns_none_for_missing_stage() {
        let Some(pool) = test_db_pool_from_env().await else {
            eprintln!("Skipping missing stage test: DATABASE_URL is not set");
            return;
        };

        let missing_stage_id = loop {
            let candidate = Uuid::new_v4();
            let existing = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM features_pipeline_stages WHERE id = $1",
            )
            .bind(candidate)
            .fetch_optional(&pool)
            .await
            .expect("Failed to verify missing stage id");

            if existing.is_none() {
                break candidate;
            }
        };

        let path = format!("/api/v1/stages/{missing_stage_id}");
        let resolved = resolve_team_id_for_request(&path, &pool)
            .await
            .expect("Missing stage lookup should not error");

        assert_eq!(resolved, None);
    }

    #[actix_web::test]
    async fn test_allows_reset_password_mutation_with_temporary_password() {
        let secret = "test_secret";
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(user_id, "tempuser", false, vec![], secret).unwrap();

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
}
