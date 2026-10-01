use crate::grpc_client::{
    assignment_key, fetch_feature_via_grpc, get_or_fetch_client_info, try_get_or_fetch_client_info,
};
use crate::pb;
use crate::{AppState, EvaluationEvent};
use actix_web::{HttpResponse, Responder, http::header, web};
use evaluation_engine as engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use tracing::error;
use tracing::info as info_log;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema, Clone)]
pub struct EvaluateHttpRequest {
    /// The feature key to evaluate
    #[serde(rename = "flagKey")]
    pub flag_key: String,
    /// Context object with bucketing_key and dynamic attributes
    pub context: EvaluateRequestContext,
}

#[derive(Deserialize, ToSchema, Clone, Debug, PartialEq)]
pub struct EvaluateRequestContext {
    /// Bucketing key for consistent user experience
    #[serde(rename = "bucketingKey")]
    pub bucketing_key: String,
    /// Dynamic attributes (flattened into the context object)
    #[serde(flatten)]
    pub attributes: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvaluateContext {
    pub bucketing_key: String,
    pub environment_id: String,
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
#[derive(Serialize, ToSchema, Clone)]
pub struct OFREPSuccessResponse {
    /// Flag key
    pub key: String,
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
#[derive(Serialize, ToSchema, Clone)]
pub struct OFREPErrorResponse {
    /// Flag key
    pub key: String,
    /// Error code
    #[serde(rename = "errorCode")]
    pub error_code: String,
    /// Optional error details
    #[serde(rename = "errorDetails", skip_serializing_if = "Option::is_none")]
    pub error_details: Option<String>,
    /// Optional metadata about the failure
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// OFREP flag evaluation item for bulk responses.
#[derive(Serialize, ToSchema, Clone)]
#[serde(untagged)]
pub enum OFREPFlagEvaluation {
    Success(OFREPSuccessResponse),
    #[allow(dead_code)]
    Failure(OFREPErrorResponse),
}

/// OFREP bulk flag evaluation request.
#[derive(Deserialize, ToSchema, Clone)]
pub struct OFREPBulkEvaluationRequest {
    /// Static evaluation context with targetingKey and custom attributes.
    pub context: OFREPContext,
}

/// Optional OFREP bulk re-fetch metadata query parameters.
#[derive(Deserialize, ToSchema, Clone)]
pub struct OFREPBulkEvaluationQuery {
    /// ETag metadata from a change event. This is not an HTTP conditional header.
    #[serde(rename = "flagConfigEtag")]
    pub flag_config_etag: Option<String>,
    /// Last-modified metadata from a change event, accepted as epoch seconds or ISO 8601 text.
    #[serde(rename = "flagConfigLastModified")]
    pub flag_config_last_modified: Option<String>,
}

/// OFREP event stream endpoint descriptor.
#[derive(Serialize, ToSchema, Clone)]
pub struct OFREPEventStreamEndpoint {
    /// Optional endpoint origin. If absent, providers use their configured OFREP base URL origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Path and query component for the event stream endpoint.
    #[serde(rename = "requestUri")]
    pub request_uri: String,
}

/// OFREP event stream descriptor.
#[derive(Serialize, ToSchema, Clone)]
pub struct OFREPEventStream {
    /// Push mechanism type. OFREP currently defines `sse`.
    #[serde(rename = "type")]
    pub stream_type: String,
    /// Opaque event stream URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Structured endpoint components.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<OFREPEventStreamEndpoint>,
    /// Client inactivity timeout in seconds.
    #[serde(rename = "inactivityDelaySec", skip_serializing_if = "Option::is_none")]
    pub inactivity_delay_sec: Option<u32>,
}

/// OFREP successful bulk evaluation response.
#[derive(Serialize, ToSchema, Clone)]
pub struct OFREPBulkEvaluationSuccess {
    /// Array of successful evaluations and per-flag failures.
    pub flags: Vec<OFREPFlagEvaluation>,
    /// Optional flag-set metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Optional real-time change notification streams.
    #[serde(rename = "eventStreams", skip_serializing_if = "Option::is_none")]
    pub event_streams: Option<Vec<OFREPEventStream>>,
}

/// OFREP failure response for request-level bulk failures.
#[derive(Serialize, ToSchema, Clone)]
pub struct OFREPBulkEvaluationFailure {
    /// OpenFeature-compatible error code.
    #[serde(rename = "errorCode")]
    pub error_code: String,
    /// Optional error details.
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
        enabled: f.active && f.kill_switch_enabled,
        // Dependencies are hydrated from cache at evaluation time using dependency IDs.
        dependencies: vec![],
        stages,
        variants,
    }
}

fn missing_dependency_placeholder(dependency_id: &str) -> engine::Feature {
    engine::Feature {
        id: dependency_id.to_string(),
        key: dependency_id.to_string(),
        feature_type: "Simple".to_string(),
        active: false,
        enabled: false,
        dependencies: vec![],
        stages: vec![],
        variants: vec![],
    }
}

fn build_hydrated_feature(
    feature_id: &str,
    feature_map: &std::collections::HashMap<String, std::sync::Arc<engine::Feature>>,
    dependency_edges: &std::collections::HashMap<String, Vec<String>>,
    memo: &mut std::collections::HashMap<String, engine::Feature>,
    visiting: &mut std::collections::HashSet<String>,
) -> engine::Feature {
    if let Some(cached) = memo.get(feature_id) {
        return cached.clone();
    }

    let Some(base_feature) = feature_map.get(feature_id) else {
        return missing_dependency_placeholder(feature_id);
    };

    if !visiting.insert(feature_id.to_string()) {
        let mut cycle_blocked = (**base_feature).clone();
        cycle_blocked.enabled = false;
        cycle_blocked.dependencies = vec![];
        return cycle_blocked;
    }

    let dependencies = dependency_edges
        .get(feature_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|dependency_id| {
            if feature_map.contains_key(&dependency_id) {
                build_hydrated_feature(
                    dependency_id.as_str(),
                    feature_map,
                    dependency_edges,
                    memo,
                    visiting,
                )
            } else {
                missing_dependency_placeholder(dependency_id.as_str())
            }
        })
        .collect::<Vec<_>>();

    visiting.remove(feature_id);

    let mut hydrated = (**base_feature).clone();
    hydrated.dependencies = dependencies;

    memo.insert(feature_id.to_string(), hydrated.clone());
    hydrated
}

async fn hydrate_feature_with_dependencies(
    app: &AppState,
    root_feature: &std::sync::Arc<engine::Feature>,
) -> engine::Feature {
    let mut feature_map: std::collections::HashMap<String, std::sync::Arc<engine::Feature>> =
        std::collections::HashMap::new();
    let mut dependency_edges: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut queue: std::collections::VecDeque<String> =
        std::collections::VecDeque::from([root_feature.id.clone()]);

    feature_map.insert(root_feature.id.clone(), root_feature.clone());

    while let Some(feature_id) = queue.pop_front() {
        let dependency_ids = app
            .mapped_cache
            .get_dependency_ids(feature_id.as_str())
            .await;
        dependency_edges.insert(feature_id.clone(), dependency_ids.clone());

        for dependency_id in dependency_ids {
            if feature_map.contains_key(&dependency_id) {
                continue;
            }

            if let Some(dependency_feature) = app.mapped_cache.get_by_id(&dependency_id).await {
                feature_map.insert(dependency_id.clone(), dependency_feature);
                queue.push_back(dependency_id);
            }
        }
    }

    let mut memo = std::collections::HashMap::new();
    let mut visiting = std::collections::HashSet::new();
    build_hydrated_feature(
        root_feature.id.as_str(),
        &feature_map,
        &dependency_edges,
        &mut memo,
        &mut visiting,
    )
}

async fn cache_fetched_feature(
    app: &AppState,
    pb_feature: &pb::FeatureFull,
) -> std::sync::Arc<engine::Feature> {
    let dependency_ids = pb_feature
        .dependencies
        .iter()
        .map(|dependency| dependency.depends_on_id.clone())
        .collect::<Vec<_>>();

    let engine_feature = std::sync::Arc::new(map_proto_to_engine(pb_feature));
    app.mapped_cache
        .insert_with_dependencies(&pb_feature.team_id, engine_feature.clone(), dependency_ids)
        .await;
    engine_feature
}

/// Remember the edge's team when the caller is the configured client.
fn note_configured_client_team(
    app: &AppState,
    client_id: &str,
    client_info: &pb::GetClientInfoResponse,
) {
    if client_id == app.client_id {
        app.record_edge_team_id(&client_info.team_id);
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

fn evaluate_http_feature_locally(
    feature_key: &str,
    feature: &engine::Feature,
    eval_context: &EvaluateContext,
) -> engine::EvaluationResult {
    if !feature.enabled {
        return engine::EvaluationResult {
            flag_key: feature_key.to_string(),
            value: serde_json::json!(false),
            variant: None,
            reason: engine::EvaluationReason::Static,
            error_code: None,
            metadata: None,
        };
    }

    let stage_exists = feature
        .stages
        .iter()
        .any(|stage| stage.environment_id == eval_context.environment_id);
    if !stage_exists {
        return engine::EvaluationResult {
            flag_key: feature_key.to_string(),
            value: serde_json::json!(false),
            variant: None,
            reason: engine::EvaluationReason::Unknown,
            error_code: Some(engine::ErrorCode::FlagNotFound),
            metadata: None,
        };
    }

    let mut result = engine::evaluate(
        &map_http_context_to_engine(feature_key.to_string(), eval_context.clone()),
        feature,
    );

    if feature.feature_type == "Simple" {
        let is_enabled = result.value.as_bool().unwrap_or(false);
        result.value = serde_json::json!(is_enabled);
        result.variant = None;
    }

    result
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

/// Get feature from cache or fetch from backend (returns mapped engine::Feature).
/// Only features owned by `team_id` (the caller's team) are returned. The
/// cache, including the negative cache, is shared only by the edge's own team.
async fn get_or_fetch_feature(
    app: &AppState,
    feature_key: &str,
    client_id: &str,
    client_secret: &str,
    team_id: &str,
) -> Result<Option<std::sync::Arc<engine::Feature>>, tonic::Status> {
    let shares_cache = app.edge_team_id() == Some(team_id);

    // Check negative cache first - avoid repeated gRPC calls for non-existent features
    if shares_cache && app.mapped_cache.is_negative_cached(feature_key).await {
        return Ok(None);
    }

    if let Some(cached) = app.mapped_cache.get_for_team(feature_key, team_id).await {
        return Ok(Some(cached));
    }

    info_log!(
        "Feature '{}' NOT in cache, fetching from backend via gRPC",
        feature_key
    );

    let pb_feature = fetch_feature_via_grpc(app, feature_key, client_id, client_secret).await?;

    match pb_feature {
        Some(pf) if pf.team_id != team_id => {
            error!(
                "Backend returned feature '{}' owned by another team; treating as not found",
                feature_key
            );
            Ok(None)
        }
        Some(pf) if shares_cache => {
            let engine_feature = cache_fetched_feature(app, &pf).await;

            info_log!("Feature '{}' successfully fetched and cached", feature_key);

            Ok(Some(engine_feature))
        }
        // Another team's caller: serve the result without caching it.
        Some(pf) => Ok(Some(std::sync::Arc::new(map_proto_to_engine(&pf)))),
        None => {
            // Only negative-cache definitive misses. Transport/auth failures are returned as Err.
            if shares_cache {
                info_log!(
                    "Feature '{}' not found in backend, adding to negative cache",
                    feature_key
                );
                app.mapped_cache.add_negative(feature_key).await;
            }
            Ok(None)
        }
    }
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

    let client_id = app.client_id.clone();
    let client_secret = app.client_secret.clone();

    // Fetch client information for origin validation (uses cache with 5min TTL)
    let client_info = match get_or_fetch_client_info(&app, &client_id, &client_secret).await {
        Some(info) => info,
        None => {
            return Err(actix_web::error::ErrorBadGateway(
                "Failed to fetch client info",
            ));
        }
    };
    note_configured_client_team(&app, &client_id, &client_info);

    // Validate web origin for web clients
    if !validate_web_origin(&http_req, &client_info) {
        error!("Origin validation failed for client: {}", client_info.name);
        return Err(actix_web::error::ErrorUnauthorized(
            "Invalid origin for web client",
        ));
    }

    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(
        &app,
        &feature_key,
        &client_id,
        &client_secret,
        &client_info.team_id,
    )
    .await
    {
        Ok(Some(f)) => f,
        Ok(None) => {
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
        Err(status) => {
            error!(
                "Failed to fetch feature '{}' from backend: code={:?} msg={}",
                feature_key,
                status.code(),
                status.message()
            );
            return Err(actix_web::error::ErrorBadGateway(
                "Failed to fetch feature from backend",
            ));
        }
    };
    let feature = std::sync::Arc::new(hydrate_feature_with_dependencies(&app, &feature).await);

    // This is kill switch enabled we should disable the feature.
    if !feature.enabled {
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

    let environment_id = client_info.environment_id.clone();
    let EvaluateRequestContext {
        bucketing_key,
        mut attributes,
    } = req.context;
    if let Some(req_env) = attributes.get("environment_id").and_then(|v| v.as_str())
        && req_env != environment_id
    {
        return Err(actix_web::error::ErrorUnauthorized(
            "Environment mismatch for client",
        ));
    }
    attributes.remove("environment_id");

    let eval_context = EvaluateContext {
        bucketing_key,
        environment_id: environment_id.clone(),
        attributes,
    };

    let stage = feature
        .stages
        .iter()
        .find(|s| s.environment_id == eval_context.environment_id);

    if stage.is_none() {
        return Ok(web::Json(EvaluateHttpResponse {
            flag_key: feature_key.clone(),
            value: serde_json::json!(false),
            variant: None,
            reason: "DEFAULT".to_string(),
            error_code: Some("ENVIRONMENT_NOT_FOUND".to_string()),
            metadata: None,
        }));
    }

    // Use targeting_key from request context (OpenFeature standard)
    let user_id_opt = Some(eval_context.bucketing_key.clone());

    // Perform evaluation (check cache first if we have a user_id)
    let (result, prior_assignment) = if let Some(user_id) = &user_id_opt {
        let key = assignment_key(user_id, &feature.id, &eval_context.environment_id);
        let cached = app
            .assigned_cache
            .get(&key)
            .and_then(|entry| entry.value().clone().into_result(&feature_key, &feature));

        if let Some(cached_result) = cached {
            // Cached assignment - return cached result with original reason (not "CACHED")
            (cached_result, true)
        } else {
            let result = evaluate_http_feature_locally(&feature_key, &feature, &eval_context);
            (result, false)
        }
    } else {
        let result = evaluate_http_feature_locally(&feature_key, &feature, &eval_context);
        (result, false)
    };

    // Record the evaluation event for analytics
    // For analytics, consider the feature "enabled" if:
    // - A variant was resolved (Contextual features), OR
    // - The value is boolean true (Simple features or Contextual without variants)
    let evaluation_result = result.variant.is_some() || result.value.as_bool().unwrap_or(false);

    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: eval_context.environment_id.clone(),
        evaluation_result,
        evaluation_context: eval_context.clone(),
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

    // Non-blocking send; drop if the queue is full
    if let Err(err) = app.evaluation_event_tx.try_send(evaluation_event) {
        match err {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                app.evaluation_event_dropped.fetch_add(1, Ordering::Relaxed);
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                tracing::warn!("Evaluation event channel closed; dropping event");
            }
        }
    }

    // Determine if we should cache this assignment:
    // - For features with variants: cache if a variant was resolved
    // - For simple features (no variant): cache if value is true
    let should_cache_assignment =
        result.variant.is_some() || result.value.as_bool().unwrap_or(false);
    if should_cache_assignment && let Some(user_id) = user_id_opt {
        let key = assignment_key(&user_id, &feature.id, &eval_context.environment_id);
        app.assigned_cache.insert(
            key,
            crate::CachedAssignment {
                value: Some(result.value.clone()),
                variant: result.variant.clone(),
                reason: result.reason.clone(),
            },
        );
        // Lock-free push - no await needed!
        app.pending_assignments
            .push(crate::grpc_client::UserAssignment {
                user_id,
                feature_id: feature.id.clone(),
                environment_id: eval_context.environment_id.clone(),
                assigned: true,
                variant: result.variant.clone(),
            });
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

/// Extract explicit credentials from Authorization header or X-API-Key header.
/// Returns `None` when no explicit credentials were provided.
fn extract_auth_from_headers(http_req: &actix_web::HttpRequest) -> Option<(String, String)> {
    // Try Bearer token first
    if let Some(auth_header) = http_req.headers().get("authorization")
        && let Ok(auth_str) = auth_header.to_str()
        && let Some(token) = auth_str.strip_prefix("Bearer ")
    {
        // For now, we don't parse JWT - just use the token as client_id
        // In production, you'd validate the JWT and extract client_id
        return Some((token.to_string(), String::new()));
    }

    // Try X-API-Key
    if let Some(api_key) = http_req.headers().get("x-api-key")
        && let Ok(key_str) = api_key.to_str()
    {
        return Some((key_str.to_string(), String::new()));
    }

    None
}

/// Resolve OFREP credentials. When the caller supplies only the configured
/// client ID, use the configured secret.
fn resolve_ofrep_credentials(
    app: &AppState,
    http_req: &actix_web::HttpRequest,
) -> Option<(String, String)> {
    let (client_id, client_secret) = extract_auth_from_headers(http_req)?;
    if client_secret.is_empty() && client_id == app.client_id {
        return Some((client_id, app.client_secret.clone()));
    }
    Some((client_id, client_secret))
}

/// Map OFREP context to engine context
fn map_ofrep_context_to_engine(
    flag_key: String,
    ofrep_ctx: OFREPContext,
    environment_id: String,
) -> engine::FeatureEvaluationContext {
    engine::FeatureEvaluationContext {
        flag_key,
        context: engine::ContextObject {
            targeting_key: ofrep_ctx.targeting_key,
            environment_id,
            attributes: ofrep_ctx.attributes,
        },
    }
}

fn ofrep_error(
    key: String,
    error_code: impl Into<String>,
    error_details: Option<String>,
) -> OFREPErrorResponse {
    OFREPErrorResponse {
        key,
        error_code: error_code.into(),
        error_details,
        metadata: None,
    }
}

/// OFREP response for a failed client-info lookup. Bad credentials and
/// unknown clients get 401, disabled clients 403. Returns `None` for other
/// failures, which the handlers keep reporting as 502.
fn ofrep_client_auth_failure(
    status: &tonic::Status,
) -> Option<(actix_web::http::StatusCode, &'static str)> {
    use actix_web::http::StatusCode;
    use tonic::Code;
    match status.code() {
        Code::Unauthenticated | Code::InvalidArgument | Code::NotFound => Some((
            StatusCode::UNAUTHORIZED,
            "Client credentials are missing or invalid",
        )),
        Code::PermissionDenied => Some((
            StatusCode::FORBIDDEN,
            "Client is not permitted to evaluate flags",
        )),
        _ => None,
    }
}

fn normalize_ofrep_context_environment(
    mut context: OFREPContext,
    environment_id: &str,
) -> Result<OFREPContext, ()> {
    if let Some(req_env) = context
        .attributes
        .get("environment_id")
        .and_then(|v| v.as_str())
        && req_env != environment_id
    {
        return Err(());
    }
    context.attributes.remove("environment_id");
    Ok(context)
}

fn ofrep_reason(reason: &engine::EvaluationReason) -> String {
    reason.as_str().to_string()
}

fn ofrep_success(
    key: String,
    value: serde_json::Value,
    reason: impl Into<String>,
    variant: Option<String>,
    metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
) -> OFREPSuccessResponse {
    OFREPSuccessResponse {
        key,
        value: Some(value),
        reason: reason.into(),
        variant,
        metadata,
    }
}

fn ofrep_disabled_success(key: String) -> OFREPSuccessResponse {
    ofrep_success(key, serde_json::json!(false), "DISABLED", None, None)
}

async fn evaluate_ofrep_feature(
    app: &AppState,
    feature_key: String,
    feature: std::sync::Arc<engine::Feature>,
    environment_id: String,
    context: OFREPContext,
) -> OFREPSuccessResponse {
    if !feature.enabled {
        app.purge_assignments_for_feature(&feature.id).await;
        return ofrep_disabled_success(feature_key);
    }

    let stage_enabled = feature
        .stages
        .iter()
        .find(|s| s.environment_id == environment_id)
        .map(|s| s.enabled)
        .unwrap_or(false);

    if !stage_enabled {
        return ofrep_disabled_success(feature_key);
    }

    let OFREPContext {
        targeting_key,
        attributes,
    } = context;
    let user_id = targeting_key.clone();

    let (mut result, prior_assignment) = {
        let cache_key = assignment_key(&user_id, &feature.id, &environment_id);
        let cached = app
            .assigned_cache
            .get(&cache_key)
            .and_then(|entry| entry.value().clone().into_result(&feature_key, &feature));

        if let Some(cached_result) = cached {
            (cached_result, true)
        } else {
            let ofrep_ctx = OFREPContext {
                targeting_key: targeting_key.clone(),
                attributes: attributes.clone(),
            };
            let ec =
                map_ofrep_context_to_engine(feature_key.clone(), ofrep_ctx, environment_id.clone());
            (engine::evaluate(&ec, &feature), false)
        }
    };

    if feature.feature_type == "Simple" {
        let is_enabled = result.value.as_bool().unwrap_or(false);
        result.value = serde_json::json!(is_enabled);
        result.variant = None;
    }

    let evaluation_result = result.variant.is_some() || result.value.as_bool().unwrap_or(false);
    let evaluation_event = EvaluationEvent {
        feature_key: feature.key.clone(),
        environment_id: environment_id.clone(),
        evaluation_result,
        evaluation_context: EvaluateContext {
            bucketing_key: user_id.clone(),
            environment_id: environment_id.clone(),
            attributes: attributes.clone(),
        },
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
    if let Err(err) = app.evaluation_event_tx.try_send(evaluation_event) {
        match err {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                app.evaluation_event_dropped.fetch_add(1, Ordering::Relaxed);
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                tracing::warn!("Evaluation event channel closed; dropping event");
            }
        }
    }

    let should_cache = result.variant.is_some() || result.value.as_bool().unwrap_or(false);
    if should_cache {
        let cache_key = assignment_key(&user_id, &feature.id, &environment_id);
        app.assigned_cache.insert(
            cache_key,
            crate::CachedAssignment {
                value: Some(result.value.clone()),
                variant: result.variant.clone(),
                reason: result.reason.clone(),
            },
        );

        app.pending_assignments
            .push(crate::grpc_client::UserAssignment {
                user_id,
                feature_id: feature.id.clone(),
                environment_id: environment_id.to_string(),
                assigned: true,
                variant: result.variant.clone(),
            });
    }

    ofrep_success(
        feature_key,
        result.value,
        ofrep_reason(&result.reason),
        result.variant,
        result.metadata,
    )
}

fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn ofrep_bulk_etag(features: &[std::sync::Arc<engine::Feature>]) -> String {
    let mut feature_payloads = features
        .iter()
        .map(|feature| {
            serde_json::to_string(feature.as_ref()).unwrap_or_else(|_| feature.key.clone())
        })
        .collect::<Vec<_>>();
    feature_payloads.sort_unstable();

    let mut hasher = Sha256::new();
    for payload in feature_payloads {
        hasher.update(payload.as_bytes());
        hasher.update(b"\n");
    }
    bytes_to_lower_hex(&hasher.finalize())
}

fn if_none_match_contains(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(|part| part.trim().trim_matches('"'))
        .any(|candidate| candidate == etag || candidate == "*")
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
        (status = 403, description = "Forbidden"),
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
    let Some((client_id, client_secret)) = resolve_ofrep_credentials(&app, &http_req) else {
        return Err(actix_web::error::ErrorUnauthorized(
            "Missing explicit client credentials",
        ));
    };

    // Validate targetingKey is not empty
    if req.context.targeting_key.is_empty() {
        return Ok(HttpResponse::BadRequest().json(ofrep_error(
            feature_key,
            "TARGETING_KEY_MISSING",
            Some("targetingKey is required and cannot be empty".to_string()),
        )));
    }

    // Fetch client information for origin validation
    let client_info = match try_get_or_fetch_client_info(&app, &client_id, &client_secret).await {
        Ok(info) => info,
        Err(status) => {
            if let Some((http_status, details)) = ofrep_client_auth_failure(&status) {
                return Ok(HttpResponse::build(http_status).json(ofrep_error(
                    feature_key,
                    "GENERAL",
                    Some(details.to_string()),
                )));
            }
            return Err(actix_web::error::ErrorBadGateway(
                "Failed to fetch client info",
            ));
        }
    };
    note_configured_client_team(&app, &client_id, &client_info);

    // Validate web origin for web clients
    if !validate_web_origin(&http_req, &client_info) {
        return Err(actix_web::error::ErrorUnauthorized(
            "Invalid origin for web client",
        ));
    }

    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(
        &app,
        &feature_key,
        &client_id,
        &client_secret,
        &client_info.team_id,
    )
    .await
    {
        Ok(Some(f)) => f,
        Ok(None) => {
            // OFREP: Return 404 for missing flags
            return Ok(HttpResponse::NotFound().json(ofrep_error(
                feature_key,
                "FLAG_NOT_FOUND",
                Some("The requested feature flag does not exist".to_string()),
            )));
        }
        Err(status) => {
            error!(
                "OFREP fetch failed for '{}': code={:?} msg={}",
                feature_key,
                status.code(),
                status.message()
            );
            return Err(actix_web::error::ErrorBadGateway(
                "Failed to fetch feature from backend",
            ));
        }
    };
    let feature = std::sync::Arc::new(hydrate_feature_with_dependencies(&app, &feature).await);

    let environment_id = client_info.environment_id.clone();
    let context = match normalize_ofrep_context_environment(req.context, &environment_id) {
        Ok(context) => context,
        Err(()) => {
            return Err(actix_web::error::ErrorUnauthorized(
                "Environment mismatch for client",
            ));
        }
    };

    let response =
        evaluate_ofrep_feature(&app, feature_key, feature, environment_id, context).await;
    Ok(HttpResponse::Ok().json(response))
}

/// OFREP handler for bulk flag evaluation
/// Spec: POST /ofrep/v1/evaluate/flags
#[utoipa::path(
    post,
    path = "/ofrep/v1/evaluate/flags",
    request_body = OFREPBulkEvaluationRequest,
    params(
        ("If-None-Match" = Option<String>, Header, description = "ETag from a previous bulk evaluation response"),
        ("flagConfigEtag" = Option<String>, Query, description = "ETag metadata from an OFREP change event"),
        ("flagConfigLastModified" = Option<String>, Query, description = "Last-modified metadata from an OFREP change event")
    ),
    responses(
        (status = 200, description = "Successful bulk evaluation", body = OFREPBulkEvaluationSuccess),
        (status = 304, description = "Not modified"),
        (status = 400, description = "Invalid request", body = OFREPBulkEvaluationFailure),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Server error", body = OFREPBulkEvaluationFailure)
    ),
    tag = "ofrep"
)]
pub async fn ofrep_evaluate_flags_bulk(
    http_req: actix_web::HttpRequest,
    app: web::Data<AppState>,
    query: web::Query<OFREPBulkEvaluationQuery>,
    req: web::Json<OFREPBulkEvaluationRequest>,
) -> actix_web::Result<HttpResponse> {
    let req = req.into_inner();
    let _change_event_refetch =
        query.flag_config_etag.is_some() || query.flag_config_last_modified.is_some();

    let Some((client_id, client_secret)) = resolve_ofrep_credentials(&app, &http_req) else {
        return Err(actix_web::error::ErrorUnauthorized(
            "Missing explicit client credentials",
        ));
    };

    if req.context.targeting_key.is_empty() {
        return Ok(HttpResponse::BadRequest().json(OFREPBulkEvaluationFailure {
            error_code: "TARGETING_KEY_MISSING".to_string(),
            error_details: Some("targetingKey is required and cannot be empty".to_string()),
        }));
    }

    let client_info = match try_get_or_fetch_client_info(&app, &client_id, &client_secret).await {
        Ok(info) => info,
        Err(status) => {
            if let Some((http_status, details)) = ofrep_client_auth_failure(&status) {
                return Ok(
                    HttpResponse::build(http_status).json(OFREPBulkEvaluationFailure {
                        error_code: "GENERAL".to_string(),
                        error_details: Some(details.to_string()),
                    }),
                );
            }
            return Err(actix_web::error::ErrorBadGateway(
                "Failed to fetch client info",
            ));
        }
    };
    note_configured_client_team(&app, &client_id, &client_info);

    if !validate_web_origin(&http_req, &client_info) {
        return Err(actix_web::error::ErrorUnauthorized(
            "Invalid origin for web client",
        ));
    }

    let environment_id = client_info.environment_id.clone();
    let context = match normalize_ofrep_context_environment(req.context, &environment_id) {
        Ok(context) => context,
        Err(()) => {
            return Err(actix_web::error::ErrorUnauthorized(
                "Environment mismatch for client",
            ));
        }
    };

    let mut features = Vec::new();
    for feature in app.mapped_cache.features_for_team(&client_info.team_id) {
        features.push(std::sync::Arc::new(
            hydrate_feature_with_dependencies(&app, &feature).await,
        ));
    }
    features.sort_by(|left, right| left.key.cmp(&right.key));

    let etag = ofrep_bulk_etag(&features);
    if let Some(if_none_match) = http_req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        && if_none_match_contains(if_none_match, &etag)
    {
        return Ok(HttpResponse::NotModified().finish());
    }

    let mut flags = Vec::with_capacity(features.len());
    for feature in features {
        let feature_key = feature.key.clone();
        let response = evaluate_ofrep_feature(
            &app,
            feature_key,
            feature,
            environment_id.clone(),
            context.clone(),
        )
        .await;
        flags.push(OFREPFlagEvaluation::Success(response));
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "version".to_string(),
        serde_json::Value::String(etag.clone()),
    );

    Ok(HttpResponse::Ok()
        .insert_header((header::ETAG, etag))
        .json(OFREPBulkEvaluationSuccess {
            flags,
            metadata: Some(metadata),
            event_streams: None,
        }))
}

#[cfg(test)]
mod tests {
    use super::{
        EvaluateContext, cache_fetched_feature, evaluate_handler, evaluate_http_feature_locally,
        extract_auth_from_headers, hydrate_feature_with_dependencies, if_none_match_contains,
        map_proto_to_engine, ofrep_bulk_etag, ofrep_evaluate_flag, ofrep_evaluate_flags_bulk,
        resolve_ofrep_credentials,
    };
    use crate::pb;
    use actix_web::test::TestRequest;
    use actix_web::{App, test as actix_test, web};
    use feature_toggle_backend::grpc::pb as backend_pb;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tonic::transport::Endpoint;

    fn simple_stage(environment_id: &str, enabled: bool) -> pb::FeatureStageFull {
        pb::FeatureStageFull {
            id: format!("stage-{environment_id}-{enabled}"),
            environment_id: environment_id.to_string(),
            order_index: 0,
            position: "Start".to_string(),
            enabled,
            criterias: vec![],
        }
    }

    fn simple_feature(
        id: &str,
        key: &str,
        active: bool,
        kill_switch_enabled: bool,
        stages: Vec<pb::FeatureStageFull>,
        dependencies: Vec<pb::FeatureDependencyFull>,
    ) -> pb::FeatureFull {
        pb::FeatureFull {
            id: id.to_string(),
            key: key.to_string(),
            description: String::new(),
            feature_type: "Simple".to_string(),
            team_id: "team-1".to_string(),
            created_at: "2026-03-26T00:00:00Z".to_string(),
            kill_switch_enabled,
            kill_switch_activated_at: String::new(),
            rollback_scheduled_at: String::new(),
            stages,
            dependencies,
            active,
            variants: vec![],
        }
    }

    fn eval_context(environment_id: &str) -> EvaluateContext {
        EvaluateContext {
            bucketing_key: "user-1".to_string(),
            environment_id: environment_id.to_string(),
            attributes: HashMap::new(),
        }
    }

    fn test_app_state(mapped_cache: Arc<crate::MappedFeatureCache>) -> crate::AppState {
        let client_info_cache = Arc::new(crate::ClientInfoCache::new(
            std::time::Duration::from_secs(300),
        ));
        let channel = Endpoint::from_static("http://127.0.0.1:50051").connect_lazy();
        let grpc_client = pb::feature_evaluation_client::FeatureEvaluationClient::new(channel);
        let (event_tx, _event_rx) = mpsc::channel(4);

        crate::AppState {
            mapped_cache,
            client_info_cache,
            grpc: Arc::new(tokio::sync::Mutex::new(grpc_client)),
            client_id: "client".into(),
            client_secret: "secret".into(),
            edge_team_id: Arc::new(std::sync::OnceLock::new()),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(dashmap::DashMap::new()),
            pending_assignments: Arc::new(crossbeam::queue::SegQueue::new()),
            flush_interval: std::time::Duration::from_secs(60),
            assignment_flush_batch_size: 10,
            evaluation_event_tx: event_tx,
            evaluation_flush_interval: std::time::Duration::from_secs(60),
            evaluation_flush_batch_size: 10,
            evaluation_event_queue_capacity: 4,
            evaluation_event_dropped: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            retry_config: crate::config::RetryConfig::default(),
        }
    }

    #[test]
    fn extract_auth_requires_explicit_headers() {
        let req = TestRequest::default().to_http_request();
        assert!(extract_auth_from_headers(&req).is_none());
    }

    #[test]
    fn extract_auth_uses_bearer_token_when_present() {
        let req = TestRequest::default()
            .insert_header(("authorization", "Bearer token-123"))
            .to_http_request();
        let auth = extract_auth_from_headers(&req);
        assert_eq!(auth, Some(("token-123".to_string(), String::new())));
    }

    #[test]
    fn extract_auth_uses_api_key_when_present() {
        let req = TestRequest::default()
            .insert_header(("x-api-key", "api-key-123"))
            .to_http_request();
        let auth = extract_auth_from_headers(&req);
        assert_eq!(auth, Some(("api-key-123".to_string(), String::new())));
    }

    #[tokio::test]
    async fn resolve_ofrep_credentials_uses_configured_secret_for_bearer_client_id() {
        let app = test_app_state(Arc::new(crate::MappedFeatureCache::new(10)));
        let req = TestRequest::default()
            .insert_header(("authorization", "Bearer client"))
            .to_http_request();

        let auth = resolve_ofrep_credentials(&app, &req);
        assert_eq!(auth, Some(("client".to_string(), "secret".to_string())));
    }

    #[tokio::test]
    async fn resolve_ofrep_credentials_uses_configured_secret_for_api_key_client_id() {
        let app = test_app_state(Arc::new(crate::MappedFeatureCache::new(10)));
        let req = TestRequest::default()
            .insert_header(("x-api-key", "client"))
            .to_http_request();

        let auth = resolve_ofrep_credentials(&app, &req);
        assert_eq!(auth, Some(("client".to_string(), "secret".to_string())));
    }

    #[tokio::test]
    async fn resolve_ofrep_credentials_keeps_empty_secret_for_other_client_id() {
        let app = test_app_state(Arc::new(crate::MappedFeatureCache::new(10)));
        let bearer = TestRequest::default()
            .insert_header(("authorization", "Bearer other-client"))
            .to_http_request();
        let api_key = TestRequest::default()
            .insert_header(("x-api-key", "other-client"))
            .to_http_request();

        assert_eq!(
            resolve_ofrep_credentials(&app, &bearer),
            Some(("other-client".to_string(), String::new()))
        );
        assert_eq!(
            resolve_ofrep_credentials(&app, &api_key),
            Some(("other-client".to_string(), String::new()))
        );
    }

    #[tokio::test]
    async fn resolve_ofrep_credentials_requires_explicit_headers() {
        let app = test_app_state(Arc::new(crate::MappedFeatureCache::new(10)));
        let req = TestRequest::default().to_http_request();

        assert!(resolve_ofrep_credentials(&app, &req).is_none());
    }

    /// Minimal backend that only accepts the configured test credentials
    /// (`client` / `secret`) and records every secret it receives.
    #[derive(Clone, Default)]
    struct OfrepMockBackend {
        seen_secrets: Arc<std::sync::Mutex<Vec<String>>>,
        /// Keys for which `GetFeatureByKey` answers `NotFound`.
        missing_keys: Arc<std::sync::Mutex<Vec<String>>>,
        /// When set, `GetClientInfo` always fails with this code.
        client_info_error: Arc<std::sync::Mutex<Option<tonic::Code>>>,
    }

    impl OfrepMockBackend {
        /// Records the secret and returns the rejection status, if any.
        fn reject_credentials(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Option<tonic::Status> {
            self.seen_secrets
                .lock()
                .unwrap()
                .push(client_secret.to_string());
            if client_secret.is_empty() {
                return Some(tonic::Status::invalid_argument("client_secret is required"));
            }
            if client_id != "client" || client_secret != "secret" {
                return Some(tonic::Status::unauthenticated("invalid client credentials"));
            }
            None
        }
    }

    #[tonic::async_trait]
    impl backend_pb::feature_evaluation_server::FeatureEvaluation for OfrepMockBackend {
        type StreamUpdatesStream = tokio_stream::wrappers::ReceiverStream<
            Result<backend_pb::FeatureUpdate, tonic::Status>,
        >;

        async fn evaluate(
            &self,
            _request: tonic::Request<backend_pb::EvaluateRequest>,
        ) -> Result<tonic::Response<backend_pb::EvaluateResponse>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in OFREP tests"))
        }

        async fn get_feature_by_key(
            &self,
            request: tonic::Request<backend_pb::GetFeatureByKeyRequest>,
        ) -> Result<tonic::Response<backend_pb::GetFeatureByKeyResponse>, tonic::Status> {
            let req = request.into_inner();
            if let Some(status) = self.reject_credentials(&req.client_id, &req.client_secret) {
                return Err(status);
            }
            if self.missing_keys.lock().unwrap().contains(&req.feature_key) {
                return Err(tonic::Status::not_found("feature not found"));
            }
            Ok(tonic::Response::new(backend_pb::GetFeatureByKeyResponse {
                feature: Some(backend_pb::FeatureFull {
                    id: "feature-1".to_string(),
                    key: req.feature_key,
                    description: String::new(),
                    feature_type: "Simple".to_string(),
                    team_id: "team-1".to_string(),
                    created_at: "2026-03-26T00:00:00Z".to_string(),
                    kill_switch_enabled: true,
                    kill_switch_activated_at: String::new(),
                    rollback_scheduled_at: String::new(),
                    stages: vec![backend_pb::FeatureStageFull {
                        id: "stage-1".to_string(),
                        environment_id: "env-1".to_string(),
                        order_index: 0,
                        position: "Start".to_string(),
                        enabled: true,
                        criterias: vec![],
                    }],
                    dependencies: vec![],
                    active: true,
                    variants: vec![],
                }),
            }))
        }

        async fn get_client_info(
            &self,
            request: tonic::Request<backend_pb::GetClientInfoRequest>,
        ) -> Result<tonic::Response<backend_pb::GetClientInfoResponse>, tonic::Status> {
            let req = request.into_inner();
            if let Some(code) = *self.client_info_error.lock().unwrap() {
                return Err(tonic::Status::new(code, "forced client info failure"));
            }
            if let Some(status) = self.reject_credentials(&req.client_id, &req.client_secret) {
                return Err(status);
            }
            Ok(tonic::Response::new(backend_pb::GetClientInfoResponse {
                id: req.client_id,
                team_id: "team-1".to_string(),
                name: "test client".to_string(),
                description: String::new(),
                enabled: true,
                client_type: "Backend".to_string(),
                web_origins: vec![],
                environment_id: "env-1".to_string(),
            }))
        }

        async fn push_user_assignments(
            &self,
            _request: tonic::Request<tonic::Streaming<backend_pb::UserFlagAssignment>>,
        ) -> Result<tonic::Response<backend_pb::Ack>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in OFREP tests"))
        }

        async fn list_user_assignments(
            &self,
            _request: tonic::Request<backend_pb::ListUserFlagAssignmentsRequest>,
        ) -> Result<tonic::Response<backend_pb::ListUserFlagAssignmentsResponse>, tonic::Status>
        {
            Err(tonic::Status::unimplemented("not used in OFREP tests"))
        }

        async fn stream_updates(
            &self,
            _request: tonic::Request<tonic::Streaming<backend_pb::StreamRequest>>,
        ) -> Result<tonic::Response<Self::StreamUpdatesStream>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in OFREP tests"))
        }

        async fn push_evaluation_events(
            &self,
            _request: tonic::Request<backend_pb::PushEvaluationEventsRequest>,
        ) -> Result<tonic::Response<backend_pb::PushEvaluationEventsResponse>, tonic::Status>
        {
            Err(tonic::Status::unimplemented("not used in OFREP tests"))
        }

        async fn track_metrics(
            &self,
            _request: tonic::Request<backend_pb::TrackMetricRequest>,
        ) -> Result<tonic::Response<backend_pb::TrackMetricResponse>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in OFREP tests"))
        }
    }

    /// Start the OFREP mock backend and return an `AppState` (configured
    /// credentials `client` / `secret`) wired to it, without retry delays.
    async fn ofrep_app_with_mock_backend() -> (crate::AppState, OfrepMockBackend) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind mock backend listener");
        let addr = listener.local_addr().expect("listener addr");
        let backend = OfrepMockBackend::default();
        let router = tonic::transport::Server::builder().add_service(
            backend_pb::feature_evaluation_server::FeatureEvaluationServer::new(backend.clone()),
        );
        tokio::spawn(async move {
            router
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .expect("mock backend should run");
        });

        let mut app = test_app_state(Arc::new(crate::MappedFeatureCache::new(10)));
        let channel = Endpoint::from_shared(format!("http://{addr}"))
            .expect("valid gRPC endpoint")
            .connect_lazy();
        app.grpc = Arc::new(tokio::sync::Mutex::new(
            pb::feature_evaluation_client::FeatureEvaluationClient::new(channel),
        ));
        app.retry_config = crate::config::RetryConfig {
            base_delay_ms: 1,
            max_attempts: 0,
            ..crate::config::RetryConfig::default()
        };
        (app, backend)
    }

    #[actix_web::test]
    async fn ofrep_single_flag_with_configured_client_id_uses_configured_secret() {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags/{key}",
                web::post().to(ofrep_evaluate_flag),
            ))
            .await;

        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/my-flag")
            .insert_header(("authorization", "Bearer client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["key"], serde_json::json!("my-flag"));
        assert_eq!(body["value"], serde_json::json!(true));

        let seen = backend.seen_secrets.lock().unwrap().clone();
        assert!(!seen.is_empty(), "expected backend calls");
        assert!(
            seen.iter().all(|secret| secret == "secret"),
            "backend saw secrets {seen:?}"
        );
    }

    #[actix_web::test]
    async fn ofrep_bulk_with_configured_client_id_uses_configured_secret() {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags",
                web::post().to(ofrep_evaluate_flags_bulk),
            ))
            .await;

        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags")
            .insert_header(("x-api-key", "client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let seen = backend.seen_secrets.lock().unwrap().clone();
        assert_eq!(seen, vec!["secret".to_string()]);
    }

    /// Send one OFREP request (single flag or bulk) as `client_id` and return
    /// the response status and body.
    async fn ofrep_call(
        app_state: crate::AppState,
        bulk: bool,
        client_id: &str,
    ) -> (actix_web::http::StatusCode, serde_json::Value) {
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .route(
                    "/ofrep/v1/evaluate/flags/{key}",
                    web::post().to(ofrep_evaluate_flag),
                )
                .route(
                    "/ofrep/v1/evaluate/flags",
                    web::post().to(ofrep_evaluate_flags_bulk),
                ),
        )
        .await;
        let uri = if bulk {
            "/ofrep/v1/evaluate/flags"
        } else {
            "/ofrep/v1/evaluate/flags/my-flag"
        };
        let req = actix_test::TestRequest::post()
            .uri(uri)
            .insert_header(("authorization", format!("Bearer {client_id}")))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;
        let status = resp.status();
        let body = actix_test::read_body(resp).await;
        let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    #[actix_web::test]
    async fn ofrep_maps_client_info_failures_to_auth_statuses() {
        use actix_web::http::StatusCode;
        let cases = [
            (tonic::Code::Unauthenticated, StatusCode::UNAUTHORIZED),
            (tonic::Code::InvalidArgument, StatusCode::UNAUTHORIZED),
            (tonic::Code::NotFound, StatusCode::UNAUTHORIZED),
            (tonic::Code::PermissionDenied, StatusCode::FORBIDDEN),
            (tonic::Code::Unavailable, StatusCode::BAD_GATEWAY),
            (tonic::Code::Internal, StatusCode::BAD_GATEWAY),
        ];
        for bulk in [false, true] {
            for (code, expected) in cases {
                let (app_state, backend) = ofrep_app_with_mock_backend().await;
                *backend.client_info_error.lock().unwrap() = Some(code);

                let (status, body) = ofrep_call(app_state, bulk, "client").await;

                assert_eq!(status, expected, "bulk={bulk} code={code:?}");
                if expected != StatusCode::BAD_GATEWAY {
                    assert_eq!(
                        body["errorCode"],
                        serde_json::json!("GENERAL"),
                        "bulk={bulk} code={code:?}"
                    );
                }
                if !bulk && expected != StatusCode::BAD_GATEWAY {
                    assert_eq!(body["key"], serde_json::json!("my-flag"));
                }
            }
        }
    }

    #[actix_web::test]
    async fn ofrep_unknown_client_without_secret_returns_unauthorized() {
        // Another client ID with no secret: the backend rejects it with
        // InvalidArgument("client_secret is required").
        for bulk in [false, true] {
            let (app_state, _backend) = ofrep_app_with_mock_backend().await;
            let (status, _) = ofrep_call(app_state, bulk, "other-client").await;
            assert_eq!(
                status,
                actix_web::http::StatusCode::UNAUTHORIZED,
                "bulk={bulk}"
            );
        }
    }

    #[actix_web::test]
    async fn evaluate_keeps_bad_gateway_for_client_info_auth_failures() {
        for code in [tonic::Code::Unauthenticated, tonic::Code::PermissionDenied] {
            let (app_state, backend) = ofrep_app_with_mock_backend().await;
            *backend.client_info_error.lock().unwrap() = Some(code);
            let service = actix_test::init_service(
                App::new()
                    .app_data(web::Data::new(app_state))
                    .route("/evaluate", web::post().to(evaluate_handler)),
            )
            .await;
            let req = actix_test::TestRequest::post()
                .uri("/evaluate")
                .set_json(serde_json::json!({
                    "flagKey": "my-flag",
                    "context": { "bucketingKey": "u1" }
                }))
                .to_request();
            let resp = actix_test::call_service(&service, req).await;

            assert_eq!(
                resp.status(),
                actix_web::http::StatusCode::BAD_GATEWAY,
                "code={code:?}"
            );
        }
    }

    /// A Simple feature owned by `team_id` with one stage in `env-1`.
    fn team_feature(id: &str, key: &str, team_id: &str, stage_enabled: bool) -> pb::FeatureFull {
        let mut feature = simple_feature(
            id,
            key,
            true,
            true,
            vec![simple_stage("env-1", stage_enabled)],
            vec![],
        );
        feature.team_id = team_id.to_string();
        feature
    }

    #[actix_web::test]
    async fn ofrep_bulk_returns_only_callers_team_flags() {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        cache_fetched_feature(
            &app_state,
            &team_feature("own-id", "own-flag", "team-1", true),
        )
        .await;
        cache_fetched_feature(
            &app_state,
            &team_feature("foreign-id", "foreign-flag", "team-2", true),
        )
        .await;
        app_state.mapped_cache.run_pending_tasks().await;

        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags",
                web::post().to(ofrep_evaluate_flags_bulk),
            ))
            .await;
        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags")
            .insert_header(("x-api-key", "client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        let keys = body["flags"]
            .as_array()
            .expect("flags array")
            .iter()
            .map(|flag| flag["key"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["own-flag".to_string()]);
    }

    #[actix_web::test]
    async fn ofrep_single_flag_for_foreign_team_key_returns_not_found() {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        backend
            .missing_keys
            .lock()
            .unwrap()
            .push("foreign-flag".to_string());
        cache_fetched_feature(
            &app_state,
            &team_feature("foreign-id", "foreign-flag", "team-2", true),
        )
        .await;
        app_state.mapped_cache.run_pending_tasks().await;

        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags/{key}",
                web::post().to(ofrep_evaluate_flag),
            ))
            .await;
        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/foreign-flag")
            .insert_header(("authorization", "Bearer client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["errorCode"], serde_json::json!("FLAG_NOT_FOUND"));
    }

    #[actix_web::test]
    async fn ofrep_single_flag_key_collision_serves_callers_team_flag() {
        // Team 2's "checkout" (stage disabled) sits in the cache; the caller
        // is in team 1, whose "checkout" (stage enabled) comes from the backend.
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        cache_fetched_feature(
            &app_state,
            &team_feature("foreign-id", "checkout", "team-2", false),
        )
        .await;
        app_state.mapped_cache.run_pending_tasks().await;

        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags/{key}",
                web::post().to(ofrep_evaluate_flag),
            ))
            .await;
        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/checkout")
            .insert_header(("authorization", "Bearer client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["value"], serde_json::json!(true));
    }

    #[actix_web::test]
    async fn ofrep_configured_client_records_edge_team_and_caches_fetch() {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        assert_eq!(app_state.edge_team_id(), None);
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state.clone()))
                .route(
                    "/ofrep/v1/evaluate/flags/{key}",
                    web::post().to(ofrep_evaluate_flag),
                ),
        )
        .await;
        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/my-flag")
            .insert_header(("authorization", "Bearer client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        assert_eq!(app_state.edge_team_id(), Some("team-1"));
        app_state.mapped_cache.run_pending_tasks().await;
        assert!(
            app_state
                .mapped_cache
                .get_for_team("my-flag", "team-1")
                .await
                .is_some()
        );
    }

    #[actix_web::test]
    async fn ofrep_fetch_for_another_teams_caller_is_not_cached() {
        // The edge serves "team-edge"; the caller's client belongs to "team-1".
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        app_state.record_edge_team_id("team-edge");
        backend
            .missing_keys
            .lock()
            .unwrap()
            .push("missing-flag".to_string());
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state.clone()))
                .route(
                    "/ofrep/v1/evaluate/flags/{key}",
                    web::post().to(ofrep_evaluate_flag),
                ),
        )
        .await;

        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/my-flag")
            .insert_header(("authorization", "Bearer client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/missing-flag")
            .insert_header(("authorization", "Bearer client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);

        app_state.mapped_cache.run_pending_tasks().await;
        assert_eq!(app_state.edge_team_id(), Some("team-edge"));
        assert_eq!(app_state.mapped_cache.entry_count(), 0);
        assert!(
            !app_state
                .mapped_cache
                .is_negative_cached("missing-flag")
                .await
        );
    }

    #[actix_web::test]
    async fn evaluate_does_not_serve_foreign_team_cached_feature() {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        cache_fetched_feature(
            &app_state,
            &team_feature("foreign-id", "checkout", "team-2", false),
        )
        .await;
        app_state.mapped_cache.run_pending_tasks().await;

        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .route("/evaluate", web::post().to(evaluate_handler)),
        )
        .await;
        let req = actix_test::TestRequest::post()
            .uri("/evaluate")
            .set_json(serde_json::json!({
                "flagKey": "checkout",
                "context": { "bucketingKey": "u1" }
            }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["value"], serde_json::json!(true));
    }

    #[test]
    fn ofrep_bulk_etag_is_stable_and_matchable() {
        let feature_a = Arc::new(map_proto_to_engine(&simple_feature(
            "feature-a",
            "alpha",
            true,
            false,
            vec![simple_stage("env-1", true)],
            vec![],
        )));
        let feature_b = Arc::new(map_proto_to_engine(&simple_feature(
            "feature-b",
            "beta",
            true,
            false,
            vec![simple_stage("env-1", true)],
            vec![],
        )));

        let etag = ofrep_bulk_etag(&[feature_a.clone(), feature_b.clone()]);
        let reversed_etag = ofrep_bulk_etag(&[feature_b, feature_a]);

        assert_eq!(etag, reversed_etag);
        assert!(if_none_match_contains(&etag, &etag));
        assert!(if_none_match_contains(&format!("\"{etag}\""), &etag));
        assert!(if_none_match_contains(&format!("stale, \"{etag}\""), &etag));
    }

    #[test]
    fn map_proto_to_engine_disables_kill_switched_features() {
        let feature = simple_feature(
            "feature-1",
            "feature-key",
            true,
            false,
            vec![simple_stage("env-1", true)],
            vec![],
        );

        let mapped = map_proto_to_engine(&feature);
        assert!(mapped.active);
        assert!(!mapped.enabled);
    }

    #[test]
    fn local_evaluation_returns_false_for_kill_switched_features() {
        let feature = map_proto_to_engine(&simple_feature(
            "feature-1",
            "feature-key",
            true,
            false,
            vec![simple_stage("env-1", true)],
            vec![],
        ));

        let result = evaluate_http_feature_locally("feature-key", &feature, &eval_context("env-1"));
        assert_eq!(result.value, serde_json::json!(false));
        assert_eq!(result.reason.as_str(), "STATIC");
    }

    #[test]
    fn local_evaluation_returns_false_for_disabled_stage() {
        let feature = map_proto_to_engine(&simple_feature(
            "feature-1",
            "feature-key",
            true,
            true,
            vec![simple_stage("env-1", false)],
            vec![],
        ));

        let result = evaluate_http_feature_locally("feature-key", &feature, &eval_context("env-1"));
        assert_eq!(result.value, serde_json::json!(false));
        assert_eq!(result.reason.as_str(), "DISABLED");
    }

    #[tokio::test]
    async fn local_evaluation_returns_false_for_disabled_dependency() {
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(10));
        let app = test_app_state(mapped_cache.clone());

        let dependency = simple_feature(
            "dep-1",
            "dependency-flag",
            true,
            false,
            vec![simple_stage("env-1", true)],
            vec![],
        );
        let root = simple_feature(
            "feature-1",
            "feature-key",
            true,
            true,
            vec![simple_stage("env-1", true)],
            vec![pb::FeatureDependencyFull {
                feature_id: "feature-1".to_string(),
                depends_on_id: "dep-1".to_string(),
            }],
        );

        cache_fetched_feature(&app, &dependency).await;
        let root_feature = cache_fetched_feature(&app, &root).await;
        mapped_cache.run_pending_tasks().await;

        let hydrated = hydrate_feature_with_dependencies(&app, &root_feature).await;
        let result =
            evaluate_http_feature_locally("feature-key", &hydrated, &eval_context("env-1"));

        assert_eq!(result.value, serde_json::json!(false));
        assert_eq!(result.reason.as_str(), "DISABLED");
        assert!(
            result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("dependencyBlock"))
                .is_some(),
            "expected dependency block metadata"
        );
    }

    /// A Contextual team-1 feature with variants `on` (true) and `off` (false)
    /// and no stage criteria, so a fresh evaluation serves `true` without a variant.
    fn contextual_feature_with_on_off(id: &str, key: &str) -> pb::FeatureFull {
        let mut feature = team_feature(id, key, "team-1", true);
        feature.feature_type = "Contextual".to_string();
        feature.variants = vec![
            pb::FeatureVariant {
                control: "on".to_string(),
                value: "true".to_string(),
            },
            pb::FeatureVariant {
                control: "off".to_string(),
                value: "false".to_string(),
            },
        ];
        feature
    }

    fn persisted_assignment(feature_id: &str, variant: &str) -> pb::UserFlagAssignment {
        pb::UserFlagAssignment {
            user_id: "u1".to_string(),
            feature_id: feature_id.to_string(),
            environment_id: "env-1".to_string(),
            assigned: true,
            client_id: String::new(),
            client_secret: String::new(),
            variant: variant.to_string(),
        }
    }

    /// Warm the assignment cache (without the snapshot purge that hides B18
    /// at startup) and call `/evaluate` for user `u1`.
    async fn evaluate_after_warmup(
        feature: pb::FeatureFull,
        variant: &str,
    ) -> (crate::AppState, serde_json::Value) {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        cache_fetched_feature(&app_state, &feature).await;
        app_state.mapped_cache.run_pending_tasks().await;
        crate::grpc_client::warm_assignment_cache(
            &app_state,
            vec![persisted_assignment(&feature.id, variant)],
        );

        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state.clone()))
                .route("/evaluate", web::post().to(evaluate_handler)),
        )
        .await;
        let req = actix_test::TestRequest::post()
            .uri("/evaluate")
            .set_json(serde_json::json!({
                "flagKey": feature.key,
                "context": { "bucketingKey": "u1" }
            }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        (app_state, actix_test::read_body_json(resp).await)
    }

    #[actix_web::test]
    async fn evaluate_serves_warmed_assignment_with_its_variant_value() {
        let (_app, body) =
            evaluate_after_warmup(contextual_feature_with_on_off("f-ctx", "ctx"), "off").await;

        assert_eq!(body["value"], serde_json::json!(false));
        assert_eq!(body["variant"], serde_json::json!("off"));
    }

    #[actix_web::test]
    async fn evaluate_serves_warmed_assignment_without_variant_as_true() {
        let (_app, body) =
            evaluate_after_warmup(team_feature("f-simple", "simple", "team-1", true), "").await;

        assert_eq!(body["value"], serde_json::json!(true));
        assert_eq!(body["variant"], serde_json::Value::Null);
    }

    #[actix_web::test]
    async fn evaluate_skips_warmed_assignment_for_removed_variant() {
        let (_app, body) =
            evaluate_after_warmup(contextual_feature_with_on_off("f-ctx", "ctx"), "gone").await;

        // Fresh evaluation: no criteria, so `true` without a variant.
        assert_eq!(body["value"], serde_json::json!(true));
        assert_eq!(body["variant"], serde_json::Value::Null);
        assert_eq!(body["reason"], serde_json::json!("STATIC"));
    }

    #[actix_web::test]
    async fn ofrep_single_flag_serves_warmed_assignment_with_its_variant_value() {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        let feature = contextual_feature_with_on_off("f-ctx", "ctx");
        cache_fetched_feature(&app_state, &feature).await;
        app_state.mapped_cache.run_pending_tasks().await;
        crate::grpc_client::warm_assignment_cache(
            &app_state,
            vec![persisted_assignment("f-ctx", "off")],
        );

        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags/{key}",
                web::post().to(ofrep_evaluate_flag),
            ))
            .await;
        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/ctx")
            .insert_header(("x-api-key", "client"))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["value"], serde_json::json!(false));
        assert_eq!(body["variant"], serde_json::json!("off"));
    }

    #[tokio::test]
    async fn cache_fetched_feature_clears_negative_cache_and_indexes_by_id() {
        let mapped_cache = Arc::new(crate::MappedFeatureCache::new(10));
        let app = test_app_state(mapped_cache.clone());

        let feature_key = "flag-new";
        mapped_cache.add_negative(feature_key).await;
        assert!(mapped_cache.is_negative_cached(feature_key).await);

        let pb_feature = simple_feature(
            "feature-id",
            feature_key,
            true,
            true,
            vec![],
            vec![pb::FeatureDependencyFull {
                feature_id: "feature-id".to_string(),
                depends_on_id: "dep-1".to_string(),
            }],
        );

        let cached = cache_fetched_feature(&app, &pb_feature).await;
        mapped_cache.run_pending_tasks().await;

        assert_eq!(cached.key, feature_key);
        assert!(mapped_cache.get(feature_key).await.is_some());
        assert!(mapped_cache.get_by_id("feature-id").await.is_some());
        assert!(!mapped_cache.is_negative_cached(feature_key).await);
        assert_eq!(
            mapped_cache.get_dependency_ids("feature-id").await,
            vec!["dep-1".to_string()]
        );
    }
}
