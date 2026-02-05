use actix_web::{get, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::model::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::activity_log::ActivityLogRepository;
use crate::database::user::user_repository_tx;
use crate::logic::jwt_token::JwtTokenLogic;
use crate::logic::user::UserLogic;
use crate::logic::user_tx;
use crate::logic::ActorContext;
use crate::rest::error::RestError;
use crate::rest::user::UserResponse;
use crate::JwtUser;

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
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    responses(
        (status = 204, description = "Logged out"),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Auth"
)]
#[post("/auth/logout")]
pub(crate) async fn logout(
    logic: web::Data<Box<dyn JwtTokenLogic>>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    let user = jwt_user(&req)?;
    let _ = logic
        .revoke_token(&user.token_hash)
        .await
        .map_err(RestError::from)?;
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
        .service(logout)
        .service(reset_password)
        .service(set_temporary_password)
        .service(auth_status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test, web};
    use crate::logic::user::MockUserLogic;
    use chrono::{DateTime, Utc};

    #[derive(Clone)]
    struct StubJwtTokenLogic {
        login_result: crate::logic::jwt_token::LoginResult,
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

    fn sample_user() -> crate::logic::user::GqlUser {
        crate::logic::user::GqlUser {
            id: ID::from(Uuid::new_v4()),
            username: "admin".to_string(),
            first_name: "Admin".to_string(),
            last_name: "User".to_string(),
            email: "admin@example.com".to_string(),
            is_admin: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login: None,
            is_temporary_password: false,
        }
    }

    #[actix_web::test]
    async fn login_returns_token_and_user() {
        let stub_logic = StubJwtTokenLogic {
            login_result: crate::logic::jwt_token::LoginResult {
                user: sample_user(),
                token: "token".to_string(),
                is_temporary: false,
            },
        };

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
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn UserLogic>
                ))
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
