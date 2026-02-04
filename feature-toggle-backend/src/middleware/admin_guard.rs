use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::{Error, HttpMessage, HttpResponse};
use futures_util::StreamExt;
use futures_util::future::{LocalBoxFuture, Ready, ready};
use sqlx::PgPool;

#[derive(Clone)]
pub struct AdminState {
    exists_cached: Arc<AtomicBool>,
    initialized: Arc<AtomicBool>,
}

impl AdminState {
    pub fn new() -> Self {
        Self {
            exists_cached: Arc::new(AtomicBool::new(false)),
            initialized: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_exists(&self, exists: bool) {
        self.exists_cached.store(exists, Ordering::Relaxed);
        self.initialized.store(true, Ordering::Relaxed);
    }

    pub fn exists(&self) -> bool {
        self.exists_cached.load(Ordering::Relaxed)
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }
}

pub struct AdminGuard {
    pool: PgPool,
    ui_origin: String,
    state: AdminState,
}

impl AdminGuard {
    pub fn new(pool: PgPool, ui_origin: String, state: AdminState) -> Self {
        Self {
            pool,
            ui_origin,
            state,
        }
    }
}

impl<S: 'static, B> Transform<S, ServiceRequest> for AdminGuard
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<actix_web::body::EitherBody<B>>;
    type Error = Error;
    type Transform = AdminGuardMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AdminGuardMiddleware {
            service: Rc::new(service),
            pool: self.pool.clone(),
            ui_origin: self.ui_origin.clone(),
            state: self.state.clone(),
        }))
    }
}

pub struct AdminGuardMiddleware<S> {
    service: Rc<S>,
    pool: PgPool,
    ui_origin: String,
    state: AdminState,
}

impl<S, B> Service<ServiceRequest> for AdminGuardMiddleware<S>
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
        let pool = self.pool.clone();
        let ui_origin = self.ui_origin.clone();
        let state = self.state.clone();

        Box::pin(async move {
            // Initialize cache once lazily
            if !state.is_initialized() {
                if let Ok(exists) = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = TRUE)",
                )
                .fetch_one(&pool)
                .await
                {
                    state.set_exists(exists);
                } else {
                    // On DB error, be conservative: allow the request to proceed
                    state.set_exists(true);
                }
            }

            // If admin exists -> proceed
            if state.exists() {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // No admin exists -> allow only admin creation/status checks and preflight OPTIONS
            let path = req.path().to_string();
            let method = req.method().clone();

            let is_preflight = method == actix_web::http::Method::OPTIONS;
            let is_graphql_post = method == actix_web::http::Method::POST && path == "/graphql";

            if is_preflight {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            if !is_graphql_post {
                let is_public_path = path == "/api/v1/health"
                    || path == "/api/v1/openapi.json"
                    || path.starts_with("/docs")
                    || (path == "/metrics/track" && method == actix_web::http::Method::POST)
                    || (path == "/api/v1/metrics/track" && method == actix_web::http::Method::POST);

                let is_allowed_rest = is_public_path
                    || (path == "/api/v1/admins" && method == actix_web::http::Method::POST)
                    || (path == "/api/v1/auth/status" && method == actix_web::http::Method::GET);

                if is_allowed_rest {
                    let res = service.call(req).await?;
                    return Ok(res.map_into_left_body());
                }

                let target = format!("{}/create-admin", ui_origin.trim_end_matches('/'));
                let res = HttpResponse::Unauthorized()
                    .json(serde_json::json!({
                        "error": "admin_account_missing",
                        "redirect": target
                    }))
                    .map_into_right_body();
                return Ok(req.into_response(res));
            }

            // Clone and read the body (non-destructive using `take_payload`)
            let mut body = Vec::new();
            while let Some(chunk) = req.take_payload().next().await {
                body.extend_from_slice(&chunk?);
            }

            let body_str = String::from_utf8_lossy(&body);
            if (body_str.contains("mutation") && body_str.contains("createAdmin"))
                || (body_str.contains("query") && body_str.contains("applicationStatus"))
            {
                // Replace the payload so Actix can read it again downstream
                req.set_payload(actix_web::web::Bytes::from(body.clone()).into());

                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Redirect to UI create-admin page
            let target = format!("{}/create-admin", ui_origin.trim_end_matches('/'));
            let res = HttpResponse::Unauthorized()
                .json(serde_json::json!({
                    "error": "admin_account_missing",
                    "redirect": target
                }))
                .map_into_right_body();
            Ok(req.into_response(res))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, HttpResponse, test, web};
    use sqlx::postgres::PgPoolOptions;

    fn test_pool() -> PgPool {
        // Lazy pool so no actual DB connection attempt until used.
        PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/feature_toggle_test")
            .expect("lazy pool")
    }

    #[actix_web::test]
    async fn allows_when_admin_exists_cached() {
        let pool = test_pool();
        let state = AdminState::new();
        state.set_exists(true);

        let app = test::init_service(
            App::new()
                .wrap(AdminGuard::new(pool, "http://ui".to_string(), state))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;
        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "query { ping }" }"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn blocks_graphql_post_without_create_admin_when_no_admin() {
        let pool = test_pool();
        let state = AdminState::new();
        state.set_exists(false);

        let app = test::init_service(
            App::new()
                .wrap(AdminGuard::new(
                    pool,
                    "http://localhost:3000".to_string(),
                    state,
                ))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;
        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "mutation { somethingElse }" }"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = test::read_body_json(resp).await;
        assert_eq!(body["error"], "admin_account_missing");
        assert_eq!(body["redirect"], "http://localhost:3000/create-admin");
    }

    #[actix_web::test]
    async fn allows_graphql_post_create_admin_when_no_admin() {
        let pool = test_pool();
        let state = AdminState::new();
        state.set_exists(false);

        let app = test::init_service(
            App::new()
                .wrap(AdminGuard::new(pool, "http://ui".to_string(), state))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;
        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "mutation { createAdmin(input:{}) { id } }" }"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn blocks_non_graphql_and_allows_options_when_no_admin() {
        let pool = test_pool();
        let state = AdminState::new();
        state.set_exists(false);

        let app = test::init_service(
            App::new()
                .wrap(AdminGuard::new(pool, "http://ui".to_string(), state))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::get().to(|| async { HttpResponse::Ok().finish() }),
                )
                .route(
                    "/other",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        // OPTIONS preflight
        let req_options = test::TestRequest::default()
            .method(actix_web::http::Method::OPTIONS)
            .uri("/graphql")
            .to_request();
        let resp_options = test::call_service(&app, req_options).await;
        assert_ne!(
            resp_options.status(),
            actix_web::http::StatusCode::UNAUTHORIZED
        );

        // GET non-graphql
        let req_get = test::TestRequest::get().uri("/other").to_request();
        let resp_get = test::call_service(&app, req_get).await;
        assert_eq!(
            resp_get.status(),
            actix_web::http::StatusCode::UNAUTHORIZED
        );

        // POST non-graphql
        let req_post = test::TestRequest::post()
            .uri("/other")
            .set_payload("{}")
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp_post = test::call_service(&app, req_post).await;
        assert_eq!(
            resp_post.status(),
            actix_web::http::StatusCode::UNAUTHORIZED
        );
    }

    #[actix_web::test]
    async fn allows_application_status_query_when_no_admin() {
        let pool = test_pool();
        let state = AdminState::new();
        state.set_exists(false);

        let app = test::init_service(
            App::new()
                .wrap(AdminGuard::new(
                    pool,
                    "http://localhost:3000".to_string(),
                    state,
                ))
                .route(
                    "/graphql",
                    web::post().to(|| async { HttpResponse::Ok().finish() }),
                ),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/graphql")
            .set_payload(r#"{ "query": "query { applicationStatus { adminConfigured } }" }"#)
            .insert_header(("content-type", "application/json"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }
}
