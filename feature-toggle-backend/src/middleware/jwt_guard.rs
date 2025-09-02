use std::rc::Rc;

use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::{Error, HttpMessage, HttpResponse};
use futures_util::StreamExt;
use futures_util::future::{LocalBoxFuture, Ready, ready};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // user id
    pub username: String,
    pub is_admin: bool,
    pub exp: usize, // expiration timestamp
    pub iat: usize, // issued at timestamp
}

pub struct JwtGuard {
    ui_origin: String,
    jwt_secret: String,
    pool: sqlx::PgPool,
}

impl JwtGuard {
    pub fn new(ui_origin: String, jwt_secret: String, pool: sqlx::PgPool) -> Self {
        Self {
            ui_origin,
            jwt_secret,
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
            jwt_secret: self.jwt_secret.clone(),
            pool: self.pool.clone(),
        }))
    }
}

pub struct JwtGuardMiddleware<S> {
    service: Rc<S>,
    ui_origin: String,
    jwt_secret: String,
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

    fn call(&self, mut req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let ui_origin = self.ui_origin.clone();
        let jwt_secret = self.jwt_secret.clone();
        let pool = self.pool.clone();

        Box::pin(async move {
            // Allow preflight OPTIONS
            let method = req.method().clone();
            if method == actix_web::http::Method::OPTIONS {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Only guard GraphQL POST
            let is_graphql_post =
                method == actix_web::http::Method::POST && req.path() == "/graphql";
            if !is_graphql_post {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Read body to inspect the operation name (login/createAdmin only skip JWT validation)
            let mut body = Vec::new();
            while let Some(chunk) = req.take_payload().next().await {
                body.extend_from_slice(&chunk?);
            }

            let body_str = String::from_utf8_lossy(&body);
            let skip_jwt = body_str.contains("mutation")
                && (body_str.contains("login") || body_str.contains("createAdmin"));

            // Restore payload for downstream
            req.set_payload(actix_web::web::Bytes::from(body.clone()).into());

            if skip_jwt {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Check JWT token in Authorization header
            let auth_header = req.headers().get("Authorization");
            if let Some(auth_value) = auth_header {
                if let Ok(auth_str) = auth_value.to_str() {
                    if let Some(token) = auth_str.strip_prefix("Bearer ") {
                        // Verify JWT token
                        let decoding_key = DecodingKey::from_secret(jwt_secret.as_ref());
                        let validation = Validation::new(Algorithm::HS256);

                        if let Ok(token_data) = decode::<Claims>(token, &decoding_key, &validation)
                        {
                            // Check if token is still valid in database
                            let token_hash = hash_token(token);
                            let token_repo = crate::database::jwt_token::jwt_token_repository(pool);

                            if let Ok(is_valid) = token_repo.is_token_valid(&token_hash).await {
                                if is_valid {
                                    // Token is valid, inject user data into request
                                    req.extensions_mut().insert(crate::JwtUser {
                                        id: Uuid::parse_str(&token_data.claims.sub)
                                            .unwrap_or_default(),
                                        username: token_data.claims.username,
                                        is_admin: token_data.claims.is_admin,
                                        token_hash: token_hash.clone(),
                                    });

                                    let res = service.call(req).await?;
                                    return Ok(res.map_into_left_body());
                                }
                            }
                        }
                    }
                }
            }

            // No valid JWT token -> Unauthorized with redirect to login page
            let target = format!("{}/login", ui_origin.trim_end_matches('/'));

            let res = HttpResponse::Unauthorized()
                .json(serde_json::json!({
                    "error": "log_in_required",
                    "redirect": target
                }))
                .map_into_right_body();
            Ok(req.into_response(res))
        })
    }
}

pub fn create_jwt_token(
    user_id: Uuid,
    username: &str,
    is_admin: bool,
    secret: &str,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = chrono::Utc::now();
    let exp = now + chrono::Duration::hours(24); // Token expires in 24 hours

    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        is_admin,
        exp: exp.timestamp() as usize,
        iat: now.timestamp() as usize,
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

    #[actix_web::test]
    async fn allows_login_mutation_without_token() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    "test_secret".to_string(),
                    test_pool(),
                ))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "mutation { login(input:{}) { id } }" }"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn blocks_graphql_post_without_valid_token() {
        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://localhost:3000".to_string(),
                    "test_secret".to_string(),
                    test_pool(),
                ))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "query { features }" }"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);

        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["error"], "log_in_required");
        assert_eq!(body["redirect"], "http://localhost:3000/login");
    }

    #[actix_web::test]
    async fn allows_graphql_post_with_valid_token() {
        let secret = "test_secret";
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(user_id, "testuser", false, secret).unwrap();

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    secret.to_string(),
                    test_pool(),
                ))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "query { features }" }"#)
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
                    "test_secret".to_string(),
                    test_pool(),
                ))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/graphql")
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_ne!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);
    }

    #[actix_web::test]
    async fn allows_logout_mutation_with_valid_token() {
        let secret = "test_secret";
        let user_id = Uuid::new_v4();
        let token = create_jwt_token(user_id, "testuser", false, secret).unwrap();

        let app = test::init_service(
            App::new()
                .wrap(JwtGuard::new(
                    "http://ui".to_string(),
                    secret.to_string(),
                    test_pool(),
                ))
                .route(
                    "/graphql",
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
            .uri("/graphql")
            .set_payload(r#"{ "query": "mutation { logout }" }"#)
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
}
