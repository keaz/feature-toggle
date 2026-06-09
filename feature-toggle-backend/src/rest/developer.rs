use actix_web::{HttpRequest, HttpResponse, Responder, get, web};
use chrono::Utc;
use serde::Serialize;
use utoipa::ToSchema;

use crate::rest::error::RestError;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityCapability {
    pub name: String,
    pub status: String,
    pub notes: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OfrepStatusResponse {
    pub status: String,
    pub generated_at: String,
    pub rest_evaluate_url: String,
    pub metrics_ingest_url: String,
    pub grpc_evaluation: String,
    pub authentication: Vec<String>,
    pub supported_evaluation_types: Vec<String>,
    pub capabilities: Vec<CompatibilityCapability>,
    pub examples: Vec<String>,
}

fn api_base_url(req: &HttpRequest) -> String {
    let info = req.connection_info();
    let scheme = info.scheme();
    let host = info.host();
    format!("{scheme}://{host}/api/v1")
}

#[utoipa::path(
    get,
    path = "/api/v1/developer/ofrep-status",
    responses(
        (status = 200, description = "OpenFeature and OFREP status", body = OfrepStatusResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Developer"
)]
#[get("/developer/ofrep-status")]
pub(crate) async fn ofrep_status(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
) -> Result<impl Responder, RestError> {
    sqlx::query("SELECT 1")
        .execute(pool.get_ref())
        .await
        .map_err(|err| RestError::internal(format!("database health check failed: {err}")))?;

    let base = api_base_url(&req);
    Ok(HttpResponse::Ok().json(OfrepStatusResponse {
        status: "healthy".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        rest_evaluate_url: format!("{base}/evaluate"),
        metrics_ingest_url: format!("{base}/metrics/track/system"),
        grpc_evaluation: "available via feature-toggle-backend gRPC evaluation service".to_string(),
        authentication: vec![
            "Bearer system-client token with evaluate scope for REST evaluation".to_string(),
            "Bearer system-client token with metrics:write scope for token metrics ingestion".to_string(),
            "Client id and client secret remain supported by SDK/gRPC ingestion paths".to_string(),
        ],
        supported_evaluation_types: vec![
            "boolean flags".to_string(),
            "string variants".to_string(),
            "number variants".to_string(),
            "JSON variants".to_string(),
            "OpenFeature targetingKey contexts".to_string(),
        ],
        capabilities: vec![
            CompatibilityCapability {
                name: "OpenFeature provider".to_string(),
                status: "supported".to_string(),
                notes: "REST evaluation accepts targetingKey, environmentId, featureKey, and context attributes.".to_string(),
            },
            CompatibilityCapability {
                name: "OFREP protocol".to_string(),
                status: "partial".to_string(),
                notes: "FluxGate exposes compatible evaluation semantics; full OFREP wire shape is documented as partial.".to_string(),
            },
            CompatibilityCapability {
                name: "Metrics ingestion".to_string(),
                status: "supported".to_string(),
                notes: "Client-secret ingestion and scoped system-token ingestion are available.".to_string(),
            },
            CompatibilityCapability {
                name: "Streaming updates".to_string(),
                status: "supported".to_string(),
                notes: "gRPC streams and REST WebSocket dashboard streams are available for live updates.".to_string(),
            },
        ],
        examples: vec![
            format!("curl -H 'Authorization: Bearer $FLUXGATE_TOKEN' {base}/developer/ofrep-status"),
            format!("curl -X POST -H 'Authorization: Bearer $FLUXGATE_TOKEN' -H 'Content-Type: application/json' {base}/evaluate -d '{{\"featureKey\":\"checkout\",\"environmentId\":\"$ENV_ID\",\"targetingKey\":\"user-1\",\"context\":{{}}}}'"),
        ],
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(ofrep_status);
}
