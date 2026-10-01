use crate::model::ID;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, post, web};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::config::AuthConfig;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::role::role_repository_tx;
use crate::database::user::user_repository_tx;
use crate::logic::ActorContext;
use crate::logic::jwt_secret::JwtSecretLogic;
use crate::logic::jwt_token::JwtTokenLogic;
use crate::logic::jwt_token_tx::{RefreshOutcome, RefreshRejection, refresh_session_in_tx};
use crate::logic::user::UserLogic;
use crate::logic::user_tx;
use crate::rest::error::RestError;
use crate::rest::user::UserResponse;

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub user: UserResponse,
    pub token: String,
    pub is_temporary: bool,
    /// Opaque refresh token for `POST /api/v1/auth/refresh`.
    pub refresh_token: String,
    /// Access token lifetime in seconds.
    pub expires_in: i64,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RefreshResponse {
    pub token: String,
    /// The new refresh token, which replaces the presented one.
    pub refresh_token: String,
    /// Access token lifetime in seconds.
    pub expires_in: i64,
    pub user: UserResponse,
}

#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogoutRequest {
    /// When present, the refresh token's whole family is revoked.
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetTemporaryPasswordRequest {
    pub temporary_password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatusResponse {
    pub admin_configured: bool,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    security(()),
    tag = "Auth"
)]
#[post("/auth/login")]
pub(crate) async fn login(
    logic: web::Data<Box<dyn JwtTokenLogic>>,
    payload: web::Json<LoginRequest>,
) -> Result<impl Responder, RestError> {
    let result = logic
        .login_user(payload.username.clone(), payload.password.clone())
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(LoginResponse {
        user: UserResponse::from(result.user),
        token: result.token,
        is_temporary: result.is_temporary,
        refresh_token: result.refresh_token,
        expires_in: result.expires_in,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Refresh token rotated; new access and refresh tokens issued", body = RefreshResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "invalid_refresh_token (unknown, expired, or user disabled) or refresh_token_reused (token family revoked)", body = crate::rest::error::ErrorResponse)
    ),
    security(()),
    tag = "Auth"
)]
#[post("/auth/refresh")]
pub(crate) async fn refresh(
    db_pool: web::Data<sqlx::PgPool>,
    jwt_secret_logic: web::Data<Box<dyn JwtSecretLogic>>,
    auth: web::Data<AuthConfig>,
    payload: web::Json<RefreshRequest>,
) -> Result<impl Responder, RestError> {
    let signing_key = jwt_secret_logic
        .get_signing_key()
        .await
        .map_err(|e| RestError::internal(format!("Failed to get JWT secret: {e}")))?;

    let user_repo = user_repository_tx(db_pool.get_ref().clone());
    let role_repo = role_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let outcome = match refresh_session_in_tx(
        &mut tx,
        &user_repo,
        &role_repo,
        &signing_key,
        auth.get_ref(),
        &payload.refresh_token,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(e));
        }
    };

    // Commit rejections too: reuse detection revokes the token family.
    tx.commit()
        .await
        .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;

    match outcome {
        RefreshOutcome::Rotated(result) => Ok(HttpResponse::Ok().json(RefreshResponse {
            token: result.token,
            refresh_token: result.refresh_token,
            expires_in: result.expires_in,
            user: UserResponse::from(result.user),
        })),
        RefreshOutcome::Rejected(RefreshRejection::Invalid) => {
            Err(RestError::invalid_refresh_token())
        }
        RefreshOutcome::Rejected(RefreshRejection::Reused) => {
            Err(RestError::refresh_token_reused())
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    request_body(content = Option<LogoutRequest>, description = "Optional. When refreshToken is given, its token family is revoked as well."),
    responses(
        (status = 204, description = "Logged out"),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/logout")]
pub(crate) async fn logout(
    db_pool: web::Data<sqlx::PgPool>,
    logic: web::Data<Box<dyn JwtTokenLogic>>,
    req: HttpRequest,
    payload: Option<web::Json<LogoutRequest>>,
) -> Result<impl Responder, RestError> {
    let user = jwt_user(&req)?;
    let revoked_user_token = logic
        .revoke_token(&user.token_hash)
        .await
        .map_err(RestError::from)?;

    let refresh_token = payload
        .and_then(|body| body.into_inner().refresh_token)
        .filter(|token| !token.is_empty());
    if revoked_user_token && let Some(refresh_token) = refresh_token {
        logic
            .revoke_refresh_token_family(user.id, &refresh_token)
            .await
            .map_err(RestError::from)?;
    }

    if !revoked_user_token {
        let system_token_repo =
            crate::database::system_client_token::system_client_token_repository(
                db_pool.get_ref().clone(),
            );
        let _ = system_token_repo
            .revoke_token(&user.token_hash)
            .await
            .map_err(RestError::from)?;
    }

    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/reset-password",
    request_body = ResetPasswordRequest,
    responses(
        (status = 204, description = "Password reset successful"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/reset-password")]
pub(crate) async fn reset_password(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    payload: web::Json<ResetPasswordRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = jwt_user(&req)?;
    let actor = Some(ActorContext::new(jwt.id, jwt.username.clone()));

    let repo_tx = user_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let result = user_tx::reset_password_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(jwt.id),
        payload.current_password.clone(),
        payload.new_password.clone(),
        actor,
    )
    .await;

    match result {
        Ok(()) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(RestError::from(e))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/users/{id}/temporary-password",
    request_body = SetTemporaryPasswordRequest,
    params(
        ("id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 204, description = "Temporary password set"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/users/{id}/temporary-password")]
pub(crate) async fn set_temporary_password(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    user_id: web::Path<String>,
    payload: web::Json<SetTemporaryPasswordRequest>,
) -> Result<impl Responder, RestError> {
    let user_uuid = parse_uuid(&user_id, "user_id")?;
    let actor = actor_from_request(&req);

    let repo_tx = user_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let result = user_tx::set_temporary_password_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(user_uuid),
        payload.temporary_password.clone(),
        actor,
    )
    .await;

    match result {
        Ok(()) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(RestError::from(e))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/status",
    responses(
        (status = 200, description = "Application status", body = AuthStatusResponse)
    ),
    security(()),
    tag = "Auth"
)]
#[get("/auth/status")]
pub(crate) async fn auth_status(
    logic: web::Data<Box<dyn UserLogic>>,
) -> Result<impl Responder, RestError> {
    let admin_configured = logic.admin_exists().await.map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(AuthStatusResponse { admin_configured }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(login)
        .service(refresh)
        .service(logout)
        .service(reset_password)
        .service(set_temporary_password)
        .service(auth_status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::user::MockUserLogic;
    use actix_web::{App, http::StatusCode, test, web};
    use chrono::{DateTime, Utc};

    #[derive(Clone)]
    struct StubJwtTokenLogic {
        login_result: crate::logic::jwt_token::LoginResult,
        revoked_families: std::sync::Arc<std::sync::Mutex<Vec<(Uuid, String)>>>,
    }

    #[async_trait::async_trait]
    impl JwtTokenLogic for StubJwtTokenLogic {
        async fn login_user(
            &self,
            _username: String,
            _password: String,
        ) -> Result<crate::logic::jwt_token::LoginResult, crate::Error> {
            Ok(self.login_result.clone())
        }

        async fn logout_user(&self, _user_id: Uuid) -> Result<u64, crate::Error> {
            Ok(1)
        }

        async fn revoke_refresh_token_family(
            &self,
            user_id: Uuid,
            refresh_token: &str,
        ) -> Result<u64, crate::Error> {
            self.revoked_families
                .lock()
                .unwrap()
                .push((user_id, refresh_token.to_string()));
            Ok(1)
        }

        async fn store_token(
            &self,
            _user_id: Uuid,
            _token_hash: String,
            _expires_at: DateTime<Utc>,
        ) -> Result<crate::database::jwt_token::JwtToken, crate::Error> {
            Err(crate::Error::InvalidInput("not implemented".to_string()))
        }

        async fn is_token_valid(&self, _token_hash: &str) -> Result<bool, crate::Error> {
            Ok(true)
        }

        async fn revoke_token(&self, _token_hash: &str) -> Result<bool, crate::Error> {
            Ok(true)
        }

        async fn revoke_all_user_tokens(&self, _user_id: Uuid) -> Result<u64, crate::Error> {
            Ok(1)
        }

        async fn cleanup_expired_tokens(&self) -> Result<u64, crate::Error> {
            Ok(0)
        }

        async fn get_user_active_tokens(
            &self,
            _user_id: Uuid,
        ) -> Result<Vec<crate::database::jwt_token::JwtToken>, crate::Error> {
            Ok(vec![])
        }

        fn clone_box(&self) -> Box<dyn JwtTokenLogic> {
            Box::new(self.clone())
        }
    }

    fn sample_user() -> crate::logic::user::ApiUser {
        crate::logic::user::ApiUser {
            id: ID::from(Uuid::new_v4()),
            username: "admin".to_string(),
            first_name: "Admin".to_string(),
            last_name: "User".to_string(),
            email: "admin@example.com".to_string(),
            mobile_number: None,
            is_admin: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login: None,
            is_temporary_password: false,
        }
    }

    fn stub_logic() -> StubJwtTokenLogic {
        StubJwtTokenLogic {
            login_result: crate::logic::jwt_token::LoginResult {
                user: sample_user(),
                token: "token".to_string(),
                is_temporary: false,
                refresh_token: "refresh".to_string(),
                expires_in: 1800,
            },
            revoked_families: Default::default(),
        }
    }

    #[actix_web::test]
    async fn login_returns_token_and_user() {
        let stub_logic = stub_logic();

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(stub_logic) as Box<dyn JwtTokenLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/api/v1/auth/login")
            .set_json(LoginRequest {
                username: "admin".to_string(),
                password: "secret".to_string(),
            })
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["token"], "token");
        assert_eq!(body["isTemporary"], false);
        assert_eq!(body["refreshToken"], "refresh");
        assert_eq!(body["expiresIn"], 1800);
        assert_eq!(body["user"]["username"], "admin");
    }

    /// Calls POST /auth/logout as an authenticated user with the given request
    /// and returns the families the logic was asked to revoke.
    async fn call_logout(
        build: impl FnOnce(test::TestRequest) -> test::TestRequest,
    ) -> (StatusCode, Uuid, Vec<(Uuid, String)>) {
        use actix_web::dev::Service;

        let stub_logic = stub_logic();
        let revoked = stub_logic.revoked_families.clone();
        let user_id = Uuid::new_v4();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/unused")
            .unwrap();
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(stub_logic) as Box<dyn JwtTokenLogic>
                ))
                .wrap_fn(move |req, srv| {
                    req.extensions_mut().insert(JwtUser {
                        id: user_id,
                        username: "u".to_string(),
                        is_admin: false,
                        roles: vec![],
                        team_id: None,
                        token_hash: "hash".to_string(),
                    });
                    srv.call(req)
                })
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = build(test::TestRequest::post().uri("/api/v1/auth/logout")).to_request();
        let status = test::call_service(&app, req).await.status();
        let revoked = revoked.lock().unwrap().clone();
        (status, user_id, revoked)
    }

    #[actix_web::test]
    async fn logout_with_refresh_token_revokes_its_family() {
        let (status, user_id, revoked) =
            call_logout(|req| req.set_json(serde_json::json!({ "refreshToken": "rt-1" }))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(revoked, vec![(user_id, "rt-1".to_string())]);
    }

    #[actix_web::test]
    async fn logout_without_refresh_token_still_works() {
        // No body at all.
        let (status, _, revoked) = call_logout(|req| req).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(revoked.is_empty());

        // Empty JSON object, as sent by existing clients.
        let (status, _, revoked) = call_logout(|req| req.set_json(serde_json::json!({}))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(revoked.is_empty());

        // Empty refresh token string.
        let (status, _, revoked) =
            call_logout(|req| req.set_json(serde_json::json!({ "refreshToken": "" }))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(revoked.is_empty());
    }

    #[actix_web::test]
    async fn auth_status_returns_value() {
        let mut mock_logic = MockUserLogic::new();
        mock_logic
            .expect_admin_exists()
            .times(1)
            .returning(|| Ok(true));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Box::new(mock_logic) as Box<dyn UserLogic>))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/api/v1/auth/status")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }
}
