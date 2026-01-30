use crate::grpc_client::{assignment_key, fetch_feature_via_grpc, get_or_fetch_client_info};
use crate::pb;
use crate::{AppState, EvaluationEvent};
use actix_web::{web, HttpResponse, Responder};
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

// ===== OFREP (OpenFeature Remote Evaluation Protocol) Models =====

/// OFREP-compliant evaluation context
#[derive(Deserialize, ToSchema, Clone, Debug)]
pub struct OFREPContext {
    /// Targeting key for user identification (OFREP standard field)
    #[serde(rename = "targetingKey")]
    pub targeting_key: String,
    /// Dynamic attributes (flattened into the context object)
    #[serde(flatten)]
    pub attributes: std::collections::HashMap<String, serde_json::Value>,
}

/// OFREP single flag evaluation request
#[derive(Deserialize, ToSchema, Clone)]
pub struct OFREPEvaluationRequest {
    /// Evaluation context with targetingKey and custom attributes
    pub context: OFREPContext,
}

/// OFREP successful evaluation response
#[derive(Serialize, ToSchema)]
pub struct OFREPSuccessResponse {
    /// Flag key (only included in error responses for single eval, always in bulk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The resolved value (omitted for code defaults)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// The reason for the evaluation result
    pub reason: String,
    /// The variant name that was served (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Optional metadata about the evaluation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// OFREP error response
#[derive(Serialize, ToSchema)]
pub struct OFREPErrorResponse {
    /// Flag key
    pub key: String,
    /// Error code
    #[serde(rename = "errorCode")]
    pub error_code: String,
    /// Optional error details
    #[serde(rename = "errorDetails", skip_serializing_if = "Option::is_none")]
    pub error_details: Option<String>,
}

/// Map protobuf feature to evaluation engine format
pub fn map_proto_to_engine(f: &pb::FeatureFull) -> engine::Feature {
    let stages = f
        .stages
        .iter()
        .map(|s| engine::FeatureStage {
            environment_id: s.environment_id.clone(),
            enabled: s.enabled,
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

                    // Parse variant selection mode
                    let variant_selection_mode =
                        match c.variant_selection_mode.to_uppercase().as_str() {
                            "SPECIFIC_VARIANT" => engine::VariantSelectionMode::SpecificVariant,
                            _ => engine::VariantSelectionMode::WeightedSplit,
                        };

                    engine::StageCriterion {
                        priority: c.priority,
                        rule_groups,
                        variant_allocations,
                        variant_selection_mode,
                        selected_variant_control: if c.selected_variant_control.is_empty() {
                            None
                        } else {
                            Some(c.selected_variant_control.clone())
                        },
                    }
                })
                .collect(),
        })
        .collect();

    // Map proto variants to engine variants
    let variants = f
        .variants
        .iter()
        .map(|v| {
            let value = serde_json::from_str(&v.value).unwrap_or_else(|e| {
                error!(
                    "Failed to parse variant value for control '{}' in feature '{}': {}. Raw value: '{}'",
                    v.control,
                    f.key,
                    e,
                    v.value
                );
                // If parsing fails, treat the raw string as a JSON string value
                serde_json::json!(v.value.clone())
            });
            engine::FeatureVariant {
                control: v.control.clone(),
                value,
            }
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
            targeting_key: ctx.bucketing_key,
            environment_id: ctx.environment_id,
            attributes: ctx.attributes,
        },
    }
}

/// Resolve client credentials from request or app defaults
fn resolve_credentials<'a>(app: &'a AppState, req: &'a EvaluateHttpRequest) -> (&'a str, &'a str) {
    let client_id = req.client_id.as_deref().unwrap_or(&app.client_id);
    let client_secret = req.client_secret.as_deref().unwrap_or(&app.client_secret);
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
/// Uses request coalescing to prevent concurrent fetches for the same key
async fn get_or_fetch_feature(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
) -> Option<std::sync::Arc<engine::Feature>> {
    // Check negative cache first - avoid repeated gRPC calls for non-existent features
    if app.mapped_cache.is_negative_cached(feature_key).await {
        return None;
    }

    // Use optionally_get_with for automatic request coalescing
    // If multiple concurrent requests ask for the same uncached key,
    // only one will execute the fetch function while others wait
    let client_id_owned = client_id.to_string();
    let client_secret_owned = client_secret.to_string();
    let app_clone = app.clone();
    let feature_key_owned = feature_key.to_string();

    let result = app
        .mapped_cache
        .optionally_get_with(feature_key.to_string(), || async move {
            info_log!(
                "Feature '{}' NOT in cache, fetching from backend via gRPC",
                feature_key_owned
            );

            // Fetch protobuf from backend
            let pb_feature = fetch_feature_via_grpc(
                &app_clone,
                &feature_key_owned,
                &client_id_owned,
                &client_secret_owned,
            )
                .await;

            match pb_feature {
                Some(pf) => {
                    // Map to engine format
                    let engine_feature = std::sync::Arc::new(map_proto_to_engine(&pf));

                    // Also update the by_id index
                    app_clone
                        .mapped_cache
                        .update_id_index(&engine_feature.id, &engine_feature.key)
                        .await;

                    info_log!(
                        "Feature '{}' successfully fetched and cached",
                        feature_key_owned
                    );

                    Some(engine_feature)
                }
                None => {
                    // Feature not found - add to negative cache
                    info_log!(
                        "Feature '{}' not found in backend, adding to negative cache",
                        feature_key_owned
                    );
                    app_clone
                        .mapped_cache
                        .add_negative(&feature_key_owned)
                        .await;
                    None
                }
            }
        })
        .await;

    result
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

    let _stage = stage.unwrap();

    // Use targeting_key from request context (OpenFeature standard)
    let user_id_opt = Some(req.context.bucketing_key.clone());

    // Perform evaluation (check cache first if we have a user_id)
    let (mut result, prior_assignment) = if let Some(user_id) = &user_id_opt {
        let key = assignment_key(user_id, &feature.id, &req.context.environment_id);
        let cached = app
            .assigned_cache
            .get(&key)
            .map(|entry| entry.value().clone());

        if let Some(cached_assignment) = cached {
            // Cached assignment - return cached result with original reason (not "CACHED")
            (
                engine::EvaluationResult {
                    flag_key: feature_key.clone(),
                    value: cached_assignment.value,
                    variant: cached_assignment.variant,
                    reason: cached_assignment.reason,
                    error_code: None,
                    metadata: None,
                },
                true,
            )
        } else {
            let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
            let result = engine::evaluate(&ec, &feature);
            (result, false)
        }
    } else {
        let ec = map_http_context_to_engine(feature_key.clone(), req.context.clone());
        let result = engine::evaluate(&ec, &feature);
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
        variant_value: if result.variant.is_some() {
            Some(result.value.clone())
        } else {
            None
        },
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
                    reason: result.reason.clone(),
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

    // Convert evaluation reason to string using zero-allocation as_str()
    let reason = result.reason.as_str().to_string();
    let error_code = result.error_code.map(|ec| ec.as_str().to_string());

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

// ===== OFREP (OpenFeature Remote Evaluation Protocol) Handlers =====

/// Extract credentials from Authorization header or X-API-Key header
fn extract_auth_from_headers(
    http_req: &actix_web::HttpRequest,
    app: &AppState,
) -> (String, String) {
    // Try Bearer token first
    if let Some(auth_header) = http_req.headers().get("authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str.starts_with("Bearer ") {
                let token = &auth_str[7..];
                // For now, we don't parse JWT - just use the token as client_id
                // In production, you'd validate the JWT and extract client_id
                return (token.to_string(), String::new());
            }
        }
    }

    // Try X-API-Key
    if let Some(api_key) = http_req.headers().get("x-api-key") {
        if let Ok(key_str) = api_key.to_str() {
            return (key_str.to_string(), String::new());
        }
    }

    // Fallback to app defaults
    (app.client_id.clone(), app.client_secret.clone())
}

/// Map OFREP context to engine context
fn map_ofrep_context_to_engine(
    flag_key: String,
    ofrep_ctx: OFREPContext,
) -> engine::FeatureEvaluationContext {
    // Extract environment_id from attributes or use a default
    let environment_id = ofrep_ctx
        .attributes
        .get("environment_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    engine::FeatureEvaluationContext {
        flag_key,
        context: engine::ContextObject {
            targeting_key: ofrep_ctx.targeting_key,
            environment_id,
            attributes: ofrep_ctx.attributes,
        },
    }
}

/// OFREP handler for single flag evaluation
/// Spec: POST /ofrep/v1/evaluate/flags/{key}
#[utoipa::path(
    post,
    path = "/ofrep/v1/evaluate/flags/{key}",
    request_body = OFREPEvaluationRequest,
    params(
        ("key" = String, Path, description = "Feature flag key")
    ),
    responses(
        (status = 200, description = "Successful evaluation", body = OFREPSuccessResponse),
        (status = 400, description = "Invalid request", body = OFREPErrorResponse),
        (status = 404, description = "Flag not found", body = OFREPErrorResponse),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Server error", body = OFREPErrorResponse)
    ),
    tag = "ofrep"
)]
pub async fn ofrep_evaluate_flag(
    http_req: actix_web::HttpRequest,
    app: web::Data<AppState>,
    path: web::Path<String>,
    req: web::Json<OFREPEvaluationRequest>,
) -> actix_web::Result<HttpResponse> {
    let feature_key = path.into_inner();
    let req = req.into_inner();

    // Extract credentials from headers (OFREP standard)
    let (client_id, client_secret) = extract_auth_from_headers(&http_req, &app);

    // Validate targetingKey is not empty
    if req.context.targeting_key.is_empty() {
        return Ok(HttpResponse::BadRequest().json(OFREPErrorResponse {
            key: feature_key,
            error_code: "TARGETING_KEY_MISSING".to_string(),
            error_details: Some("targetingKey is required and cannot be empty".to_string()),
        }));
    }

    // Fetch client information for origin validation
    let client_info = get_or_fetch_client_info(&app, &client_id, &client_secret).await;

    // Validate web origin for web clients
    if let Some(ref client_info) = client_info {
        if !validate_web_origin(&http_req, client_info) {
            return Err(actix_web::error::ErrorUnauthorized(
                "Invalid origin for web client",
            ));
        }
    }

    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(&app, &feature_key, &client_id, &client_secret).await {
        Some(f) => f,
        None => {
            // OFREP: Return 404 for missing flags
            return Ok(HttpResponse::NotFound().json(OFREPErrorResponse {
                key: feature_key,
                error_code: "FLAG_NOT_FOUND".to_string(),
                error_details: Some("The requested feature flag does not exist".to_string()),
            }));
        }
    };

    // Kill switch: disable feature if not active
    if !feature.active {
        app.purge_assignments_for_feature(&feature.id).await;
        return Ok(HttpResponse::Ok().json(OFREPSuccessResponse {
            key: None,
            value: Some(serde_json::json!(false)),
            reason: "DISABLED".to_string(),
            variant: None,
            metadata: None,
        }));
    }

    // Check environment stage enabled
    let environment_id = req
        .context
        .attributes
        .get("environment_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    let stage_enabled = feature
        .stages
        .iter()
        .find(|s| s.environment_id == environment_id)
        .map(|s| s.enabled)
        .unwrap_or(false);

    if !stage_enabled {
        return Ok(HttpResponse::Ok().json(OFREPSuccessResponse {
            key: None,
            value: Some(serde_json::json!(false)),
            reason: "DISABLED".to_string(),
            variant: None,
            metadata: None,
        }));
    }

    // Extract user_id from targeting_key
    let user_id = req.context.targeting_key.clone();

    // Check cache for sticky assignment
    let (mut result, prior_assignment) = {
        let cache_key = assignment_key(&user_id, &feature.id, &environment_id);
        let cached = app
            .assigned_cache
            .get(&cache_key)
            .map(|entry| entry.value().clone());

        if let Some(cached_assignment) = cached {
            // Return cached result with original reason
            (
                engine::EvaluationResult {
                    flag_key: feature_key.clone(),
                    value: cached_assignment.value,
                    variant: cached_assignment.variant,
                    reason: cached_assignment.reason,
                    error_code: None,
                    metadata: None,
                },
                true,
            )
        } else {
            // Perform fresh evaluation
            let ec = map_ofrep_context_to_engine(feature_key.clone(), req.context.clone());
            let result = engine::evaluate(&ec, &feature);
            (result, false)
        }
    };

    // For Simple features, ensure value is always boolean and variant is None
    if feature.feature_type == "Simple" {
        let is_enabled = result.value.as_bool().unwrap_or(false);
        result.value = serde_json::json!(is_enabled);
        result.variant = None;
    }

    // Record evaluation event for analytics/observability
    let evaluation_result = result.variant.is_some() || result.value.as_bool().unwrap_or(false);
    let evaluation_context = EvaluateContext {
        bucketing_key: user_id.clone(),
        environment_id: environment_id.clone(),
        attributes: req.context.attributes.clone(),
    };
    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: environment_id.clone(),
        evaluation_result,
        evaluation_context,
        user_context: Some(user_id.clone()),
        evaluated_at: std::time::SystemTime::now(),
        prior_assignment,
        variant: result.variant.clone(),
        variant_value: if result.variant.is_some() {
            Some(result.value.clone())
        } else {
            None
        },
    };
    let _ = app.evaluation_event_tx.send(evaluation_event);

    // Cache successful assignments
    let should_cache = result.variant.is_some() || result.value.as_bool().unwrap_or(false);
    if should_cache {
        let cache_key = assignment_key(&user_id, &feature.id, &environment_id);
        app.assigned_cache.insert(
            cache_key,
            crate::CachedAssignment {
                value: result.value.clone(),
                variant: result.variant.clone(),
                reason: result.reason.clone(),
            },
        );

        // Queue for backend persistence
        app.pending_assignments
            .push(crate::grpc_client::UserAssignment {
                user_id,
                feature_id: feature.id.clone(),
                environment_id: environment_id.to_string(),
                assigned: true,
                variant: result.variant.clone(),
            });
    }

    // Convert reason to SCREAMING_SNAKE_CASE string (using serde serialization)
    let reason = serde_json::to_string(&result.reason)
        .unwrap_or_else(|_| "UNKNOWN".to_string())
        .trim_matches('"')
        .to_string();

    // OFREP response: no "key" field for single evaluation success
    Ok(HttpResponse::Ok().json(OFREPSuccessResponse {
        key: None,
        value: Some(result.value),
        reason,
        variant: result.variant,
        metadata: None,
    }))
}
