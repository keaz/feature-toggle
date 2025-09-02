// DEPRECATED: This module is replaced by jwt_guard.rs
// Keeping this file for compatibility during transition
// TODO: Remove this file once all references are updated

use std::rc::Rc;
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error};
use futures_util::future::{ready, LocalBoxFuture, Ready};

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

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();

        Box::pin(async move {
            // Since we're transitioning to JWT, just pass through all requests
            let res = service.call(req).await?;
            Ok(res.map_into_left_body())
        })
    }
}
