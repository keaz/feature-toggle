use std::rc::Rc;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error, HttpMessage, HttpResponse};
use futures_util::future::{ready, LocalBoxFuture, Ready};
use futures_util::StreamExt;
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
        Self { pool, ui_origin, state }
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
                if let Ok(exists) = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = TRUE)")
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

            // No admin exists -> allow only GraphQL POST for admin creation and preflight OPTIONS
            let path = req.path().to_string();
            let method = req.method().clone();

            let is_preflight = method == actix_web::http::Method::OPTIONS;
            let is_graphql_post = method == actix_web::http::Method::POST && path == "/graphql";

            if is_preflight || !is_graphql_post {
                // Let it pass. If this was an admin creation, mutation handler will flip the flag via AdminState.
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Clone and read the body (non-destructive using `take_payload`)
            let mut body = Vec::new();
            while let Some(chunk) = req.take_payload().next().await {
                body.extend_from_slice(&chunk?);
            }

            let body_str = String::from_utf8_lossy(&body);
            if body_str.contains("mutation") && body_str.contains("createAdmin") {
                // Replace the payload so Actix can read it again downstream
                req.set_payload(actix_web::web::Bytes::from(body.clone()).into());

                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Redirect to UI create-admin page
            let target = format!("{}/create-admin", ui_origin.trim_end_matches('/'));
            let res = HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "admin_account_missing",
                "redirect": target
            })).map_into_right_body();
            Ok(req.into_response(res))
        })
    }
}
