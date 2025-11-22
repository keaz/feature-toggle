use crate::grpc_client::{assignment_key, fetch_feature_via_grpc, get_or_fetch_client_info};
use crate::pb;
use crate::{AppState, EvaluationEvent};
use actix_web::{HttpResponse, Responder, web};
use evaluation_engine as engine;
use serde::{Deserialize, Serialize};
use tracing::error;
use tracing::info as info_log;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema, Clone)]
pub struct EvaluateHttpRequest {
    /// The feature key to evaluate
    #[serde(rename = "flagKey")]
    pub flag_key: String,
    /// Context object with bucketing_key, environment_id, and dynamic attributes
    pub context: EvaluateContext,
    /// Optional client credentials overriding server defaults
    pub client_id: Option<String>,
    /// Optional client credentials overriding server defaults
    pub client_secret: Option<String>,
}

#[derive(Deserialize, ToSchema, Clone, Debug, PartialEq)]
pub struct EvaluateContext {
    /// Bucketing key for consistent user experience
    #[serde(rename = "bucketingKey")]
    pub bucketing_key: String,
    /// Environment identifier (UUID as string)
    pub environment_id: String,
    /// Dynamic attributes (flattened into the context object)
    #[serde(flatten)]
    pub attributes: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
pub struct EvaluateHttpResponse {
    /// The feature key that was evaluated
    #[serde(rename = "flagKey")]
    pub flag_key: String,
    /// The resolved value (can be boolean, string, number, or JSON object)
    pub value: serde_json::Value,
    /// The variant name that was served (if any)
    pub variant: Option<String>,
    /// The reason for the evaluation result
    pub reason: String,
    /// Error code if evaluation failed
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Optional metadata about the evaluation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Map protobuf feature to evaluation engine format
pub fn map_proto_to_engine(f: &pb::FeatureFull) -> engine::Feature {
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
                    // Parse compound rules from protobuf
                    let rule_groups = c
                        .rule_groups
                        .iter()
                        .map(|group| {
                            let logic_operator = match group.logic_operator.to_uppercase().as_str()
                            {
                                "OR" => engine::LogicOperator::Or,
                                _ => engine::LogicOperator::And, // Default to AND
                            };

                            let conditions = group
                                .conditions
                                .iter()
                                .map(|cond| {
                                    let cond_operator = match cond.operator.to_uppercase().as_str()
                                    {
                                        "EQUALS" => engine::Operator::Equals,
                                        "NOTEQUALS" | "NOT_EQUALS" => engine::Operator::NotEquals,
                                        "GREATERTHAN" | "GREATER_THAN" => {
                                            engine::Operator::GreaterThan
                                        }
                                        "LESSTHAN" | "LESS_THAN" => engine::Operator::LessThan,
                                        "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => {
                                            engine::Operator::GreaterThanOrEqual
                                        }
                                        "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => {
                                            engine::Operator::LessThanOrEqual
                                        }
                                        "CONTAINS" => engine::Operator::Contains,
                                        "STARTSWITH" | "STARTS_WITH" => {
                                            engine::Operator::StartsWith
                                        }
                                        "ENDSWITH" | "ENDS_WITH" => engine::Operator::EndsWith,
                                        "REGEX" => engine::Operator::Regex,
                                        "IN" => engine::Operator::In,
                                        "NOTIN" | "NOT_IN" => engine::Operator::NotIn,
                                        "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => {
                                            engine::Operator::SemverGreaterThan
                                        }
                                        "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => {
                                            engine::Operator::SemverLessThan
                                        }
                                        _ => engine::Operator::In,
                                    };

                                    let value = serde_json::from_str(&cond.value)
                                        .unwrap_or_else(|_| serde_json::json!(cond.value.clone()));

                                    engine::RuleCondition {
                                        context_key: cond.context_key.clone(),
                                        operator: cond_operator,
                                        value,
                                    }
                                })
                                .collect();

                            engine::RuleGroup {
                                logic_operator,
                                conditions,
                            }
                        })
                        .collect();

                    // Map variant allocations from protobuf
                    let variant_allocations = c
                        .variant_allocations
                        .iter()
                        .map(|alloc| engine::VariantAllocation {
                            variant_control: alloc.variant_control.clone(),
                            weight: alloc.weight,
                        })
                        .collect();

                    engine::StageCriterion {
                        priority: c.priority,
                        rule_groups,
                        variant_allocations,
                    }
                })
                .collect(),
        })
        .collect();

    // Map proto variants to engine variants
    let variants = f
        .variants
        .iter()
        .map(|v| engine::FeatureVariant {
            control: v.control.clone(),
            value: serde_json::from_str(&v.value).unwrap_or(serde_json::json!(v.value.clone())),
        })
        .collect();

    engine::Feature {
        id: f.id.clone(),
        key: f.key.clone(),
        feature_type: f.feature_type.clone(),
        active: f.active,
        enabled: f.active,
        dependencies: vec![], // For minimal implementation, ignore dependency recursion
        stages,
        variants,
    }
}

/// Map HTTP context to evaluation engine format
pub fn map_http_context_to_engine(
    feature_key: String,
    ctx: EvaluateContext,
) -> engine::FeatureEvaluationContext {
    engine::FeatureEvaluationContext {
        flag_key: feature_key,
        context: engine::ContextObject {
            bucketing_key: ctx.bucketing_key,
            environment_id: ctx.environment_id,
            attributes: ctx.attributes,
        },
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

/// Get feature from cache or fetch from backend (returns mapped engine::Feature)
async fn get_or_fetch_feature(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<std::sync::Arc<engine::Feature>> {
    // Check mapped cache first
    if let Some(mapped) = app.mapped_cache.get(feature_key).await {
        info_log!("Feature '{}' found in mapped cache", feature_key);
        return Some(mapped);
    }

    // Cache miss - fetch protobuf from backend and map it
    info_log!(
        "Feature '{}' NOT in cache, fetching from backend via gRPC",
        feature_key
    );
    let pb_feature = fetch_feature_via_grpc(app, feature_key, client_id, client_secret).await?;

    // Map to engine format
    let engine_feature = std::sync::Arc::new(map_proto_to_engine(&pb_feature));

    // Cache the mapped version
    app.mapped_cache.insert(engine_feature.clone()).await;
    info_log!(
        "Feature '{}' successfully cached in mapped cache",
        feature_key
    );

    Some(engine_feature)
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
    let feature_key = req.flag_key.clone();

    let (client_id, client_secret) = resolve_credentials(&app, &req);

    // Fetch client information for origin validation (uses cache with 5min TTL)
    let client_info = get_or_fetch_client_info(&app, &client_id, &client_secret).await;

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
            // Feature doesn't exist, return default
            return Ok(web::Json(EvaluateHttpResponse {
                flag_key: feature_key.clone(),
                value: serde_json::json!(false),
                variant: None,
                reason: "DEFAULT".to_string(),
                error_code: Some("FLAG_NOT_FOUND".to_string()),
                metadata: None,
            }));
        }
    };

    // This is kill switch enabled we should disable the feature.
    if !feature.active {
        app.purge_assignments_for_feature(&feature.id).await;
        return Ok(web::Json(EvaluateHttpResponse {
            flag_key: feature_key.clone(),
            value: serde_json::json!(false),
            variant: None,
            reason: "STATIC".to_string(),
            error_code: None,
            metadata: None,
        }));
    }

    let stage = feature
        .stages
        .iter()
        .find(|s| s.environment_id == req.context.environment_id);

    if stage.is_none() || !stage.unwrap().enabled {
        return Ok(web::Json(EvaluateHttpResponse {
            flag_key: feature_key.clone(),
            value: serde_json::json!(false),
            variant: None,
            reason: if stage.is_none() {
                "DEFAULT"
            } else {
                "DISABLED"
            }
            .to_string(),
            error_code: if stage.is_none() {
                Some("ENVIRONMENT_NOT_FOUND".to_string())
            } else {
                None
            },
            metadata: None,
        }));
    }

    let stage = stage.unwrap();
    let bucketing_key = if stage.bucketing_key.is_some() {
        stage.bucketing_key.as_ref().unwrap().as_str()
    } else {
        "bucketingKey"
    };

    // Extract user_id from bucketing_key attribute or use default bucketing_key
    let user_id_opt = if bucketing_key == "bucketingKey" {
        Some(req.context.bucketing_key.clone())
    } else {
        req.context
            .attributes
            .get(bucketing_key)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    };

    // Perform evaluation (check cache first if we have a user_id)
    let (mut result, prior_assignment) = if let Some(user_id) = &user_id_opt {
        let key = assignment_key(user_id, &feature.id, &req.context.environment_id);
        let cached = app
            .assigned_cache
            .get(&key)
            .map(|entry| entry.value().clone());

        if let Some(cached_assignment) = cached {
            // Cached assignment - return cached result with variant
            (
                engine::EvaluationResult {
                    flag_key: feature_key.clone(),
                    value: cached_assignment.value,
                    variant: cached_assignment.variant,
                    reason: engine::EvaluationReason::Cached,
                    error_code: None,
                    metadata: None,
                },
                true,
            )
        } else {
            let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
            let result = engine::evaluate(ec, (*feature).clone());
            (result, false)
        }
    } else {
        let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
        let result = engine::evaluate(ec, (*feature).clone());
        (result, false)
    };

    // For Simple features, ensure value is always boolean and variant is None
    if feature.feature_type == "Simple" {
        let is_enabled = result.value.as_bool().unwrap_or(false);
        result.value = serde_json::json!(is_enabled);
        result.variant = None;
    }

    // Record the evaluation event for analytics
    // For analytics, consider the feature "enabled" if:
    // - A variant was resolved (Contextual features), OR
    // - The value is boolean true (Simple features or Contextual without variants)
    let evaluation_result = result.variant.is_some() || result.value.as_bool().unwrap_or(false);

    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: req.context.environment_id.clone(),
        evaluation_result,
        evaluation_context: req.context.clone(),
        user_context: user_id_opt.clone(),
        evaluated_at: std::time::SystemTime::now(),
        prior_assignment,
        variant: result.variant.clone(),
    };

    // Non-blocking send - if channel is full, this will fail silently (unbounded so shouldn't happen)
    let _ = app.evaluation_event_tx.send(evaluation_event);

    // Determine if we should cache this assignment:
    // - For features with variants: cache if a variant was resolved
    // - For simple features (no variant): cache if value is true
    let should_cache_assignment =
        result.variant.is_some() || result.value.as_bool().unwrap_or(false);
    if should_cache_assignment {
        if let Some(user_id) = user_id_opt {
            let key = assignment_key(&user_id, &feature.id, &req.context.environment_id);
            app.assigned_cache.insert(
                key,
                crate::CachedAssignment {
                    value: result.value.clone(),
                    variant: result.variant.clone(),
                },
            );
            // Lock-free push - no await needed!
            app.pending_assignments
                .push(crate::grpc_client::UserAssignment {
                    user_id,
                    feature_id: feature.id.clone(),
                    environment_id: req.context.environment_id.clone(),
                    assigned: true,
                    variant: result.variant.clone(),
                });
        }
    }

    // Convert evaluation reason to string
    let reason = format!("{:?}", result.reason).to_uppercase();
    let error_code = result
        .error_code
        .map(|ec| format!("{:?}", ec).to_uppercase());

    // Convert metadata HashMap to JSON Value
    let metadata = result
        .metadata
        .map(|m| serde_json::to_value(m).unwrap_or(serde_json::json!({})));

    Ok(web::Json(EvaluateHttpResponse {
        flag_key: result.flag_key,
        value: result.value,
        variant: result.variant,
        reason,
        error_code,
        metadata,
    }))
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
