use std::rc::Rc;

use actix_session::SessionExt;
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error, HttpMessage, HttpResponse};
use futures_util::future::{ready, LocalBoxFuture, Ready};
use futures_util::StreamExt;

pub struct SessionGuard {
    ui_origin: String,
}

impl SessionGuard {
    pub fn new(ui_origin: String) -> Self {
        Self { ui_origin }
    }
}

impl<S: 'static, B> Transform<S, ServiceRequest> for SessionGuard
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: actix_web::body::MessageBody + 'static,
{
    type Response = ServiceResponse<actix_web::body::EitherBody<B>>;
    type Error = Error;
    type Transform = SessionGuardMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(SessionGuardMiddleware {
            service: Rc::new(service),
            ui_origin: self.ui_origin.clone(),
        }))
    }
}

pub struct SessionGuardMiddleware<S> {
    service: Rc<S>,
    ui_origin: String,
}

impl<S, B> Service<ServiceRequest> for SessionGuardMiddleware<S>
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

        Box::pin(async move {
            // Allow preflight OPTIONS
            let method = req.method().clone();
            if method == actix_web::http::Method::OPTIONS {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Only guard GraphQL POST
            let is_graphql_post = method == actix_web::http::Method::POST && req.path() == "/graphql";
            if !is_graphql_post {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Read body to inspect the operation name (login/createAdmin)
            let mut body = Vec::new();
            while let Some(chunk) = req.take_payload().next().await {
                body.extend_from_slice(&chunk?);
            }

            let body_str = String::from_utf8_lossy(&body);
            let skip = body_str.contains("mutation") && (body_str.contains("login") || body_str.contains("createAdmin"));

            // Restore payload for downstream
            req.set_payload(actix_web::web::Bytes::from(body.clone()).into());

            if skip {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // Check session
            let session = req.get_session();
            let has_user = session.get::<String>("user_id").ok().flatten().is_some();

            if has_user {
                let res = service.call(req).await?;
                return Ok(res.map_into_left_body());
            }

            // No session -> Unauthorized with redirect to login page (same format as admin_guard)
            let target = format!(
                "{}/login",
                ui_origin.trim_end_matches('/')
            );

            let res = HttpResponse::Unauthorized().json(serde_json::json!({
                "error": "log_in_required",
                "redirect": target
            })).map_into_right_body();
            Ok(req.into_response(res))
        })
    }
}
