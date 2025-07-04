use actix_web::{
    Error,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use futures_util::future::LocalBoxFuture;
use std::future::{Ready, ready};
use std::rc::Rc;
use std::time::Instant;

use log::info;

pub struct AccessLogger;

impl<S: 'static, B> Transform<S, ServiceRequest> for AccessLogger
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = AccessLoggerMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AccessLoggerMiddleware {
            service: Rc::new(service),
        }))
    }
}

pub struct AccessLoggerMiddleware<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for AccessLoggerMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let start_time = Instant::now();
        let method = req.method().clone();
        let path = req.path().to_string();
        let peer_addr = req
            .connection_info()
            .realip_remote_addr()
            .unwrap_or("-")
            .to_string();

        Box::pin(async move {
            let res = service.call(req).await?;

            let duration = start_time.elapsed().as_millis();
            let status = res.status().as_u16();

            info!(
                "{method} {path} -> {status} ({duration} ms) [{peer_addr}]"
            );

            Ok(res)
        })
    }
}
