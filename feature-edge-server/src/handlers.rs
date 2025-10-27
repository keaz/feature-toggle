use crate::grpc_client::{assignment_key, fetch_client_info_via_grpc, fetch_feature_via_grpc};
use crate::kill_switch::parse_rollback_timestamp;
use crate::pb;
use crate::{AppState, EvaluationEvent};
use actix_web::{HttpResponse, Responder, web};
use chrono::{DateTime, Utc};
use evaluation_engine as engine;
use serde::{Deserialize, Serialize};
use tracing::error;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema, Clone)]
pub struct EvaluateHttpRequest {
    /// The feature key to evaluate
    pub feature_key: String,
    /// Environment identifier (e.g., "prod", "staging")
    pub environment_id: String,
    /// Context entries used for evaluation (key/value)
    pub context: Vec<HttpContext>,
    /// Optional client credentials overriding server defaults
    pub client_id: Option<String>,
    /// Optional client credentials overriding server defaults
    pub client_secret: Option<String>,
}

#[derive(Deserialize, ToSchema, Clone, Debug, PartialEq)]
pub struct HttpContext {
    /// Context key, e.g., "user.id" or a bucketing key
    pub key: String,
    /// Context value as string
    pub value: String,
}

#[derive(Serialize, ToSchema)]
pub struct EvaluateHttpResponse {
    /// Whether the feature is enabled under provided context
    pub enabled: bool,
}

/// Map protobuf feature to evaluation engine format
fn map_proto_to_engine(f: &pb::FeatureFull, rollback_at: Option<DateTime<Utc>>) -> engine::Feature {
    let stages = f
        .stages
        .iter()
        .map(|s| engine::FeatureStage {
            environment_id: s.environment_id.clone(),
            enabled: s.enabled,
            bucketing_key: if s.bucketing_key.is_empty() {
                None
            } else {
                Some(s.bucketing_key.clone())
            },
            criterias: s
                .criterias
                .iter()
                .map(|c| {
                    // Map CriterionContext to StageContext
                    let context = if let Some(ref ctx) = c.context {
                        engine::StageContext {
                            key: ctx.key.clone(),
                            entries: ctx.entries.clone(),
                        }
                    } else {
                        engine::StageContext {
                            key: String::new(),
                            entries: vec![],
                        }
                    };

                    engine::StageCriterion {
                        context_key: c.context_key.clone(),
                        context,
                        rollout_percentage: c.rollout_percentage,
                    }
                })
                .collect(),
        })
        .collect();

    engine::Feature {
        enabled: true, // Top-level flag not present in proto; default to true
        kill_switch_enabled: f.kill_switch_enabled,
        rollback_scheduled_at: rollback_at,
        dependencies: vec![], // For minimal implementation, ignore dependency recursion
        stages,
    }
}

/// Map HTTP context to evaluation engine format
pub fn map_http_context_to_engine(
    feature_key: String,
    environment_id: String,
    ctx: Vec<HttpContext>,
) -> engine::FeatureEvaluationContext {
    engine::FeatureEvaluationContext {
        feature: feature_key,
        environment_id,
        context: ctx
            .into_iter()
            .map(|c| engine::Context {
                key: c.key,
                value: c.value,
            })
            .collect(),
    }
}

/// Resolve client credentials from request or app defaults
fn resolve_credentials(app: &AppState, req: &EvaluateHttpRequest) -> (String, String) {
    let client_id = req
        .client_id
        .clone()
        .unwrap_or_else(|| app.client_id.clone());
    let client_secret = req
        .client_secret
        .clone()
        .unwrap_or_else(|| app.client_secret.clone());
    (client_id, client_secret)
}

/// Validate web origin for Web client types
fn validate_web_origin(
    http_req: &actix_web::HttpRequest,
    client_info: &pb::GetClientInfoResponse,
) -> bool {
    // Only validate origins for Web clients
    if client_info.client_type != "Web" {
        return true; // Backend clients don't need origin validation
    }

    // Get the Origin header from the request
    let origin = match http_req.headers().get("origin") {
        Some(origin_header) => match origin_header.to_str() {
            Ok(origin_str) => origin_str,
            Err(_) => {
                error!("Invalid Origin header format");
                return false;
            }
        },
        None => {
            // For web clients, Origin header is required
            error!("Missing Origin header for web client request");
            return false;
        }
    };

    // Check if the origin is in the allowed list
    let allowed = client_info.web_origins.contains(&origin.to_string());
    if !allowed {
        error!(
            "Origin '{}' not allowed for client '{}'. Allowed origins: {:?}",
            origin, client_info.name, client_info.web_origins
        );
    }
    allowed
}

/// Get feature from cache or fetch from backend
async fn get_or_fetch_feature(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<pb::FeatureFull> {
    if let Some(f) = app.cache.get_by_key(feature_key).await {
        return Some(f);
    }
    let feature = fetch_feature_via_grpc(app, feature_key, client_id, client_secret).await?;
    app.cache.upsert(feature.clone()).await;
    Some(feature)
}

/// HTTP handler for feature evaluation
#[utoipa::path(
    post,
    path = "/evaluate",
    request_body = EvaluateHttpRequest,
    responses(
        (status = 200, description = "Evaluation result", body = EvaluateHttpResponse),
        (status = 502, description = "Backend unavailable"),
        (status = 400, description = "Invalid request")
    ),
    tag = "edge"
)]
pub async fn evaluate_handler(
    http_req: actix_web::HttpRequest,
    app: web::Data<AppState>,
    req: web::Json<EvaluateHttpRequest>,
) -> actix_web::Result<web::Json<EvaluateHttpResponse>> {
    let req = req.into_inner();
    let feature_key = req.feature_key.clone();

    let (client_id, client_secret) = resolve_credentials(&app, &req);

    // Fetch client information for origin validation
    let client_info = fetch_client_info_via_grpc(&app, &client_id, &client_secret).await;

    // Validate web origin for web clients (if client info is available)
    if let Some(ref client_info) = client_info
        && !validate_web_origin(&http_req, client_info)
    {
        error!("Origin validation failed for client: {}", client_info.name);
        return Err(actix_web::error::ErrorUnauthorized(
            "Invalid origin for web client",
        ));
    }

    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(&app, &feature_key, &client_id, &client_secret).await {
        Some(f) => f,
        None => {
            // Feature doesn't exist, return false
            return Ok(web::Json(EvaluateHttpResponse { enabled: false }));
        }
    };

    let stage = feature
        .stages
        .iter()
        .find(|s| s.environment_id == req.environment_id);

    if stage.is_none() || !stage.unwrap().enabled {
        return Ok(web::Json(EvaluateHttpResponse { enabled: false }));
    }

    // Check kill switch - if kill_switch_enabled is false, feature is disabled
    if !feature.kill_switch_enabled {
        app.purge_assignments_for_feature(&feature.id).await;
        return Ok(web::Json(EvaluateHttpResponse { enabled: false }));
    }

    let rollback_at = parse_rollback_timestamp(&feature.rollback_scheduled_at);
    if let Some(ts) = rollback_at {
        if ts <= Utc::now() {
            app.purge_assignments_for_feature(&feature.id).await;
            return Ok(web::Json(EvaluateHttpResponse { enabled: false }));
        }
    }

    let stage = stage.unwrap();
    let bucketing_key = stage.bucketing_key.clone();

    // Extract user.id if present
    let user_id_opt = req
        .context
        .iter()
        .find(|c| c.key == bucketing_key)
        .map(|c| c.value.clone());

    // If we have a prior assignment for this user+feature+env, short-circuit to true
    let (enabled, prior_assignment) = if let Some(user_id) = &user_id_opt {
        let key = assignment_key(user_id, &feature.id, &req.environment_id);
        if app.assigned_true.read().await.contains(&key) {
            (true, true) // cached assignment
        } else {
            let engine_feature = map_proto_to_engine(&feature, rollback_at.clone());
            let ec = map_http_context_to_engine(
                feature_key,
                req.environment_id.clone(),
                req.context.clone(),
            );
            let enabled = engine::evaluate(ec, engine_feature);
            (enabled, false) // fresh evaluation
        }
    } else {
        let engine_feature = map_proto_to_engine(&feature, rollback_at.clone());
        let ec = map_http_context_to_engine(
            feature_key,
            req.environment_id.clone(),
            req.context.clone(),
        );
        let enabled = engine::evaluate(ec, engine_feature);
        (enabled, false) // fresh evaluation
    };

    // Record the evaluation event for analytics
    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: req.environment_id.clone(),
        evaluation_result: enabled,
        evaluation_context: req.context.clone(),
        user_context: user_id_opt.clone(),
        evaluated_at: std::time::SystemTime::now(),
        prior_assignment,
    };

    {
        let mut pending_events = app.pending_evaluation_events.write().await;
        pending_events.push(evaluation_event);
    }

    // If evaluated true, remember and enqueue for flush
    if enabled && let Some(user_id) = user_id_opt {
        let key = assignment_key(&user_id, &feature.id, &req.environment_id);
        {
            let mut set = app.assigned_true.write().await;
            set.insert(key);
        }
        let mut pending = app.pending_assignments.write().await;
        pending.push(crate::grpc_client::UserAssignment {
            user_id,
            feature_id: feature.id.clone(),
            environment_id: req.environment_id,
            assigned: true,
        });
    }

    Ok(web::Json(EvaluateHttpResponse { enabled }))
}

/// HTTP handler for health check
#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Service is healthy"), (status = 503, description = "Service is not connected to backend")),
    tag = "edge"
)]
pub async fn health_handler(app: web::Data<AppState>) -> impl Responder {
    use std::sync::atomic::Ordering;
    if app.connected.load(Ordering::Relaxed) {
        HttpResponse::Ok().body("OK")
    } else {
        HttpResponse::ServiceUnavailable().body("UNAVAILABLE")
    }
}
