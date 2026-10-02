//! AI judgment endpoints: subsystem status (AI-00).

use actix_web::{HttpResponse, Responder, get, web};
use serde::Serialize;
use utoipa::ToSchema;

use crate::judgment::AiRuntime;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiStatusResponse {
    /// True when the server has a TypeSafe API key.
    pub available: bool,
    /// Pinned model id; null when unavailable.
    pub model: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ai/status",
    responses(
        (status = 200, description = "AI judgment subsystem status", body = AiStatusResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[get("/ai/status")]
pub(crate) async fn get_ai_status(runtime: web::Data<AiRuntime>) -> impl Responder {
    HttpResponse::Ok().json(AiStatusResponse {
        available: runtime.available(),
        model: runtime.status_model(),
    })
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_ai_status);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix_web::{App, test, web};
    use serde_json::{Value, json};

    use crate::judgment::client::MockJudgmentClient;
    use crate::judgment::{AiRuntime, JudgmentClient};

    #[actix_web::test]
    async fn status_without_key_is_unavailable() {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AiRuntime::new(None, "jev-1.13.0")))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/api/v1/ai/status")
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body, json!({ "available": false, "model": null }));
    }

    #[actix_web::test]
    async fn status_with_client_reports_model() {
        let client: Arc<dyn JudgmentClient> = Arc::new(MockJudgmentClient::new());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AiRuntime::new(Some(client), "jev-1.13.0")))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get()
            .uri("/api/v1/ai/status")
            .to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body, json!({ "available": true, "model": "jev-1.13.0" }));
    }
}
