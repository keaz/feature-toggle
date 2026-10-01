use crate::grpc_client::{
    fetch_feature_via_grpc, get_or_fetch_client_info, try_get_or_fetch_client_info,
};
use crate::pb;
use crate::{AppState, EvaluationEvent};
use actix_web::http::StatusCode;
use actix_web::http::header::{self, HeaderValue};
use actix_web::{HttpResponse, Responder, web};
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

/// OFREP authentication or authorization failure (401 / 403).
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct OFREPAuthErrorResponse {
    /// `UNAUTHORIZED` (401) or `FORBIDDEN` (403).
    #[serde(rename = "errorCode")]
    pub error_code: String,
    /// Human-readable reason.
    #[serde(rename = "errorDetails")]
    pub error_details: String,
}

/// Error body of the edge's own (non-OFREP) endpoints.
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct EdgeErrorResponse {
    /// Error code, for example `FORBIDDEN`.
    pub error: String,
    /// Human-readable reason.
    pub message: String,
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

/// Origin rejected for a `Web` client (missing, unreadable or not listed in
/// its `web_origins`).
#[derive(Debug, PartialEq, Eq)]
struct OriginNotAllowed;

/// CORS decision for an actual (non-preflight) request from an authenticated
/// client.
///
/// - `Web` client, `Origin` listed in `web_origins`: `Ok(Some(origin))`, so
///   the response carries CORS headers for that origin.
/// - `Web` client, `Origin` missing or not listed: `Err(OriginNotAllowed)`.
/// - `Backend` client: `Ok(None)`. The request is served without CORS headers,
///   whatever the `Origin`.
fn cors_origin_for_client(
    http_req: &actix_web::HttpRequest,
    client_info: &pb::GetClientInfoResponse,
) -> Result<Option<HeaderValue>, OriginNotAllowed> {
    if !client_info.client_type.eq_ignore_ascii_case("web") {
        return Ok(None);
    }

    let origin = http_req.headers().get(header::ORIGIN);
    let allowed = origin
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| client_info.web_origins.iter().any(|o| o == origin));
    if allowed {
        return Ok(origin.cloned());
    }

    match origin.map(|value| value.to_str()) {
        Some(Ok(origin)) => error!(
            "Origin '{}' not allowed for web client '{}'",
            origin, client_info.name
        ),
        Some(Err(_)) => error!(
            "Unreadable Origin header for web client '{}'",
            client_info.name
        ),
        None => error!(
            "Missing Origin header for web client '{}'",
            client_info.name
        ),
    }
    Err(OriginNotAllowed)
}

/// Add the CORS headers of an allowed cross-origin request to `response`.
fn with_cors_headers(mut response: HttpResponse, origin: Option<&HeaderValue>) -> HttpResponse {
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("ETag"),
        );
        headers.append(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}

/// CORS preflight (`OPTIONS`) for the evaluation endpoints. A preflight
/// carries no credentials, so the client is not checked here; the actual
/// request is checked against the client's `web_origins`.
pub async fn cors_preflight(http_req: actix_web::HttpRequest) -> HttpResponse {
    let mut response = HttpResponse::NoContent();
    if let Some(origin) = http_req.headers().get(header::ORIGIN) {
        response.insert_header((header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone()));
    }
    response
        .insert_header((header::ACCESS_CONTROL_ALLOW_METHODS, "POST, OPTIONS"))
        .insert_header((
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            "Authorization, X-API-Key, Content-Type, If-None-Match",
        ))
        .insert_header((header::ACCESS_CONTROL_MAX_AGE, "600"))
        .insert_header((header::VARY, "Origin"))
        .finish()
}

/// Register the edge's HTTP routes, including CORS preflights.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health_handler))
        .route("/evaluate", web::post().to(evaluate_handler))
        .route(
            "/evaluate",
            web::method(actix_web::http::Method::OPTIONS).to(cors_preflight),
        )
        // OFREP (OpenFeature Remote Evaluation Protocol) endpoints
        .route(
            "/ofrep/v1/evaluate/flags",
            web::post().to(ofrep_evaluate_flags_bulk),
        )
        .route(
            "/ofrep/v1/evaluate/flags",
            web::method(actix_web::http::Method::OPTIONS).to(cors_preflight),
        )
        .route(
            "/ofrep/v1/evaluate/flags/{key}",
            web::post().to(ofrep_evaluate_flag),
        )
        .route(
            "/ofrep/v1/evaluate/flags/{key}",
            web::method(actix_web::http::Method::OPTIONS).to(cors_preflight),
        );
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
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Origin not allowed for the edge's web client", body = EdgeErrorResponse),
        (status = 502, description = "Backend unavailable")
    ),
    tag = "edge"
)]
pub async fn evaluate_handler(
    http_req: actix_web::HttpRequest,
    app: web::Data<AppState>,
    req: web::Json<EvaluateHttpRequest>,
) -> HttpResponse {
    // `/evaluate` always acts as the edge-configured client.
    let client_id = app.client_id.clone();
    let client_secret = app.client_secret.clone();

    // Fetch client information for origin validation (uses cache with 5min TTL)
    let Some(client_info) = get_or_fetch_client_info(&app, &client_id, &client_secret).await else {
        return actix_web::error::ErrorBadGateway("Failed to fetch client info").error_response();
    };
    note_configured_client_team(&app, &client_id, &client_info);

    let Ok(cors_origin) = cors_origin_for_client(&http_req, &client_info) else {
        return HttpResponse::Forbidden().json(EdgeErrorResponse {
            error: "FORBIDDEN".to_string(),
            message: "Origin is not allowed for this client".to_string(),
        });
    };

    let response = match evaluate_for_client(
        &app,
        req.into_inner(),
        &client_id,
        &client_secret,
        &client_info,
    )
    .await
    {
        Ok(body) => HttpResponse::Ok().json(body),
        Err(err) => err.error_response(),
    };
    with_cors_headers(response, cors_origin.as_ref())
}

/// Evaluate one flag for an authenticated client whose origin was accepted.
async fn evaluate_for_client(
    app: &AppState,
    req: EvaluateHttpRequest,
    client_id: &str,
    client_secret: &str,
    client_info: &pb::GetClientInfoResponse,
) -> actix_web::Result<EvaluateHttpResponse> {
    let feature_key = req.flag_key.clone();

    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(
        app,
        &feature_key,
        client_id,
        client_secret,
        &client_info.team_id,
    )
    .await
    {
        Ok(Some(f)) => f,
        Ok(None) => {
            // Feature doesn't exist, return default
            return Ok(EvaluateHttpResponse {
                flag_key: feature_key.clone(),
                value: serde_json::json!(false),
                variant: None,
                reason: "DEFAULT".to_string(),
                error_code: Some("FLAG_NOT_FOUND".to_string()),
                metadata: None,
            });
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
    let feature = std::sync::Arc::new(hydrate_feature_with_dependencies(app, &feature).await);

    // This is kill switch enabled we should disable the feature.
    if !feature.enabled {
        app.purge_assignments_for_feature(&feature.id).await;
        return Ok(EvaluateHttpResponse {
            flag_key: feature_key.clone(),
            value: serde_json::json!(false),
            variant: None,
            reason: "STATIC".to_string(),
            error_code: None,
            metadata: None,
        });
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
        return Ok(EvaluateHttpResponse {
            flag_key: feature_key.clone(),
            value: serde_json::json!(false),
            variant: None,
            reason: "DEFAULT".to_string(),
            error_code: Some("ENVIRONMENT_NOT_FOUND".to_string()),
            metadata: None,
        });
    }

    // Use targeting_key from request context (OpenFeature standard)
    let user_id_opt = Some(eval_context.bucketing_key.clone());

    // Perform evaluation (check cache first if we have a user_id)
    let (result, prior_assignment) = if let Some(user_id) = &user_id_opt {
        let cached = app
            .assigned_cache
            .get(user_id, &feature.id, &eval_context.environment_id)
            .map(|assignment| assignment.into_result(&feature_key));

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
        app.assigned_cache.insert(
            &user_id,
            &feature.id,
            &eval_context.environment_id,
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

    Ok(EvaluateHttpResponse {
        flag_key: result.flag_key,
        value: result.value,
        variant: result.variant,
        reason,
        error_code,
        metadata,
    })
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

/// Why an OFREP request carries no usable SDK key.
#[derive(Debug, PartialEq, Eq)]
enum SdkKeyError {
    /// Neither `Authorization: Bearer` nor `X-API-Key` is present.
    Missing,
    /// The key is not `<clientId>.<secret>` with a UUID client ID and a
    /// non-empty secret.
    Malformed,
}

/// Credentials from an SDK key.
#[derive(PartialEq, Eq)]
struct SdkCredentials {
    client_id: String,
    client_secret: String,
}

// Hand-written so the secret never reaches logs or panic messages.
impl std::fmt::Debug for SdkCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkCredentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

/// Parse an SDK key `<clientId>.<secret>`. The key is split on the first
/// `.`, so the secret may itself contain dots.
fn parse_sdk_key(sdk_key: &str) -> Result<SdkCredentials, SdkKeyError> {
    let (client_id, client_secret) = sdk_key
        .trim()
        .split_once('.')
        .ok_or(SdkKeyError::Malformed)?;
    if client_id.is_empty() || client_secret.is_empty() {
        return Err(SdkKeyError::Malformed);
    }
    uuid::Uuid::parse_str(client_id).map_err(|_| SdkKeyError::Malformed)?;
    Ok(SdkCredentials {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
    })
}

/// Read the SDK key from `Authorization: Bearer <sdkKey>` or, failing that,
/// `X-API-Key: <sdkKey>`.
fn extract_auth_from_headers(
    http_req: &actix_web::HttpRequest,
) -> Result<SdkCredentials, SdkKeyError> {
    let headers = http_req.headers();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.trim().split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then_some(token)
        });
    let sdk_key = match bearer {
        Some(token) => token,
        None => match headers.get("x-api-key") {
            Some(value) => value.to_str().map_err(|_| SdkKeyError::Malformed)?,
            None => return Err(SdkKeyError::Missing),
        },
    };
    parse_sdk_key(sdk_key)
}

/// OFREP 401 / 403 response with `errorCode` `UNAUTHORIZED` / `FORBIDDEN`.
fn ofrep_auth_error(status: StatusCode, details: &str) -> HttpResponse {
    let error_code = if status == StatusCode::FORBIDDEN {
        "FORBIDDEN"
    } else {
        "UNAUTHORIZED"
    };
    HttpResponse::build(status).json(OFREPAuthErrorResponse {
        error_code: error_code.to_string(),
        error_details: details.to_string(),
    })
}

/// OFREP response for a backend call rejected because of the caller's
/// credentials. Bad credentials and unknown clients get 401, disabled clients
/// 403. Returns `None` for other failures (backend unavailable or broken),
/// which are reported as 502.
///
/// None of these codes is retried (see `grpc_client::is_transient`).
fn ofrep_backend_auth_failure(status: &tonic::Status) -> Option<HttpResponse> {
    use tonic::Code;
    match status.code() {
        Code::Unauthenticated | Code::InvalidArgument | Code::NotFound => Some(ofrep_auth_error(
            StatusCode::UNAUTHORIZED,
            "SDK key is invalid",
        )),
        Code::PermissionDenied => Some(ofrep_auth_error(
            StatusCode::FORBIDDEN,
            "Client is disabled or not permitted to evaluate flags",
        )),
        _ => None,
    }
}

/// An OFREP caller whose SDK key and origin were accepted.
struct OfrepCaller {
    credentials: SdkCredentials,
    client_info: pb::GetClientInfoResponse,
    /// `Origin` to echo in CORS headers (allowed `Web` clients only).
    cors_origin: Option<HeaderValue>,
}

/// Authenticate an OFREP request with its SDK key and apply the client's
/// origin rules. On failure returns the response to send (401, 403 or 502).
async fn authenticate_ofrep(
    app: &AppState,
    http_req: &actix_web::HttpRequest,
) -> Result<OfrepCaller, HttpResponse> {
    let credentials = extract_auth_from_headers(http_req).map_err(|err| match err {
        SdkKeyError::Missing => ofrep_auth_error(
            StatusCode::UNAUTHORIZED,
            "Missing SDK key: send Authorization: Bearer <clientId>.<apiKey> or X-API-Key",
        ),
        SdkKeyError::Malformed => ofrep_auth_error(
            StatusCode::UNAUTHORIZED,
            "Malformed SDK key: expected <clientId>.<apiKey>",
        ),
    })?;

    let client_info =
        try_get_or_fetch_client_info(app, &credentials.client_id, &credentials.client_secret)
            .await
            .map_err(|status| {
                ofrep_backend_auth_failure(&status).unwrap_or_else(|| {
                    actix_web::error::ErrorBadGateway("Failed to fetch client info")
                        .error_response()
                })
            })?;
    if !client_info.enabled {
        return Err(ofrep_auth_error(
            StatusCode::FORBIDDEN,
            "Client is disabled or not permitted to evaluate flags",
        ));
    }
    note_configured_client_team(app, &credentials.client_id, &client_info);

    let cors_origin = cors_origin_for_client(http_req, &client_info).map_err(|_| {
        ofrep_auth_error(
            StatusCode::FORBIDDEN,
            "Origin is not allowed for this client",
        )
    })?;

    Ok(OfrepCaller {
        credentials,
        client_info,
        cors_origin,
    })
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
        let cached = app
            .assigned_cache
            .get(&user_id, &feature.id, &environment_id)
            .map(|assignment| assignment.into_result(&feature_key));

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
        app.assigned_cache.insert(
            &user_id,
            &feature.id,
            &environment_id,
            crate::CachedAssignment {
                value: result.value.clone(),
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

/// ETag of a bulk OFREP response. The response depends on the flag configs,
/// the caller's environment and the evaluation context, so all of them are
/// hashed. Each part is length-prefixed so field boundaries cannot shift.
fn ofrep_bulk_etag(
    features: &[std::sync::Arc<engine::Feature>],
    environment_id: &str,
    context: &OFREPContext,
) -> String {
    // Top-level attributes come from a HashMap; sort them. Nested objects are
    // `serde_json::Map`, which is already sorted (no `preserve_order`).
    let attributes = context
        .attributes
        .iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let attributes = serde_json::to_string(&attributes).unwrap_or_default();
    let config_hash = ofrep_flag_config_hash(features);

    let mut hasher = Sha256::new();
    for part in [
        config_hash.as_bytes(),
        environment_id.as_bytes(),
        context.targeting_key.as_bytes(),
        attributes.as_bytes(),
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    bytes_to_lower_hex(&hasher.finalize())
}

fn ofrep_flag_config_hash(features: &[std::sync::Arc<engine::Feature>]) -> String {
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

/// Whether `If-None-Match` lists `etag`. The `*` wildcard is ignored: a bulk
/// evaluation is a POST, so it must not short-circuit to 304.
fn if_none_match_contains(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(|part| part.trim().trim_matches('"'))
        .any(|candidate| candidate == etag)
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
        (status = 401, description = "Missing, malformed or invalid SDK key", body = OFREPAuthErrorResponse),
        (status = 403, description = "Client disabled or origin not allowed", body = OFREPAuthErrorResponse),
        (status = 500, description = "Server error", body = OFREPErrorResponse),
        (status = 502, description = "Backend unavailable")
    ),
    tag = "ofrep"
)]
pub async fn ofrep_evaluate_flag(
    http_req: actix_web::HttpRequest,
    app: web::Data<AppState>,
    path: web::Path<String>,
    req: web::Json<OFREPEvaluationRequest>,
) -> HttpResponse {
    let caller = match authenticate_ofrep(&app, &http_req).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let response =
        ofrep_evaluate_flag_for(&app, &caller, path.into_inner(), req.into_inner()).await;
    with_cors_headers(response, caller.cors_origin.as_ref())
}

async fn ofrep_evaluate_flag_for(
    app: &AppState,
    caller: &OfrepCaller,
    feature_key: String,
    req: OFREPEvaluationRequest,
) -> HttpResponse {
    // Validate targetingKey is not empty
    if req.context.targeting_key.is_empty() {
        return HttpResponse::BadRequest().json(ofrep_error(
            feature_key,
            "TARGETING_KEY_MISSING",
            Some("targetingKey is required and cannot be empty".to_string()),
        ));
    }

    // Get feature from cache or backend
    let feature = match get_or_fetch_feature(
        app,
        &feature_key,
        &caller.credentials.client_id,
        &caller.credentials.client_secret,
        &caller.client_info.team_id,
    )
    .await
    {
        Ok(Some(f)) => f,
        Ok(None) => {
            // OFREP: Return 404 for missing flags
            return HttpResponse::NotFound().json(ofrep_error(
                feature_key,
                "FLAG_NOT_FOUND",
                Some("The requested feature flag does not exist".to_string()),
            ));
        }
        Err(status) => {
            error!(
                "OFREP fetch failed for '{}': code={:?} msg={}",
                feature_key,
                status.code(),
                status.message()
            );
            if let Some(response) = ofrep_backend_auth_failure(&status) {
                return response;
            }
            return actix_web::error::ErrorBadGateway("Failed to fetch feature from backend")
                .error_response();
        }
    };
    let feature = std::sync::Arc::new(hydrate_feature_with_dependencies(app, &feature).await);

    let environment_id = caller.client_info.environment_id.clone();
    let Ok(context) = normalize_ofrep_context_environment(req.context, &environment_id) else {
        return actix_web::error::ErrorUnauthorized("Environment mismatch for client")
            .error_response();
    };

    let response = evaluate_ofrep_feature(app, feature_key, feature, environment_id, context).await;
    HttpResponse::Ok().json(response)
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
        (status = 401, description = "Missing, malformed or invalid SDK key", body = OFREPAuthErrorResponse),
        (status = 403, description = "Client disabled or origin not allowed", body = OFREPAuthErrorResponse),
        (status = 500, description = "Server error", body = OFREPBulkEvaluationFailure),
        (status = 502, description = "Backend unavailable")
    ),
    tag = "ofrep"
)]
pub async fn ofrep_evaluate_flags_bulk(
    http_req: actix_web::HttpRequest,
    app: web::Data<AppState>,
    query: web::Query<OFREPBulkEvaluationQuery>,
    req: web::Json<OFREPBulkEvaluationRequest>,
) -> HttpResponse {
    let _change_event_refetch =
        query.flag_config_etag.is_some() || query.flag_config_last_modified.is_some();

    let caller = match authenticate_ofrep(&app, &http_req).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let if_none_match = http_req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok());
    let response =
        ofrep_evaluate_flags_bulk_for(&app, &caller, req.into_inner(), if_none_match).await;
    with_cors_headers(response, caller.cors_origin.as_ref())
}

async fn ofrep_evaluate_flags_bulk_for(
    app: &AppState,
    caller: &OfrepCaller,
    req: OFREPBulkEvaluationRequest,
    if_none_match: Option<&str>,
) -> HttpResponse {
    if req.context.targeting_key.is_empty() {
        return HttpResponse::BadRequest().json(OFREPBulkEvaluationFailure {
            error_code: "TARGETING_KEY_MISSING".to_string(),
            error_details: Some("targetingKey is required and cannot be empty".to_string()),
        });
    }

    let environment_id = caller.client_info.environment_id.clone();
    let Ok(context) = normalize_ofrep_context_environment(req.context, &environment_id) else {
        return actix_web::error::ErrorUnauthorized("Environment mismatch for client")
            .error_response();
    };

    let mut features = Vec::new();
    for feature in app
        .mapped_cache
        .features_for_team(&caller.client_info.team_id)
    {
        features.push(std::sync::Arc::new(
            hydrate_feature_with_dependencies(app, &feature).await,
        ));
    }
    features.sort_by(|left, right| left.key.cmp(&right.key));

    let etag = ofrep_bulk_etag(&features, &environment_id, &context);
    if if_none_match.is_some_and(|if_none_match| if_none_match_contains(if_none_match, &etag)) {
        return HttpResponse::NotModified()
            .insert_header((header::ETAG, etag))
            .finish();
    }

    let mut flags = Vec::with_capacity(features.len());
    for feature in features {
        let feature_key = feature.key.clone();
        let response = evaluate_ofrep_feature(
            app,
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

    HttpResponse::Ok()
        .insert_header((header::ETAG, etag))
        .json(OFREPBulkEvaluationSuccess {
            flags,
            metadata: Some(metadata),
            event_streams: None,
        })
}

#[cfg(test)]
mod tests {
    use super::{
        EvaluateContext, OFREPContext, SdkCredentials, SdkKeyError, cache_fetched_feature,
        configure_routes, evaluate_handler, evaluate_http_feature_locally,
        extract_auth_from_headers, hydrate_feature_with_dependencies, if_none_match_contains,
        map_proto_to_engine, ofrep_bulk_etag, ofrep_evaluate_flag, ofrep_evaluate_flags_bulk,
        parse_sdk_key,
    };
    use crate::pb;
    use actix_web::test::TestRequest;
    use actix_web::{App, test as actix_test, web};
    use feature_toggle_backend::grpc::pb as backend_pb;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
            client_id: CONFIGURED_CLIENT_ID.into(),
            client_secret: CONFIGURED_SECRET.into(),
            edge_team_id: Arc::new(std::sync::OnceLock::new()),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            assigned_cache: Arc::new(crate::AssignmentCache::default()),
            pending_assignments: Arc::new(crate::PendingAssignments::default()),
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

    /// Client ID and secret the test `AppState` is configured with.
    const CONFIGURED_CLIENT_ID: &str = "11111111-1111-4111-8111-111111111111";
    const CONFIGURED_SECRET: &str = "secret";
    /// A second client of the same team, used for web / disabled scenarios.
    const OTHER_CLIENT_ID: &str = "22222222-2222-4222-8222-222222222222";
    const OTHER_SECRET: &str = "other.secret";
    const ALLOWED_ORIGIN: &str = "https://app.example.com";

    fn sdk_key(client_id: &str, secret: &str) -> String {
        format!("{client_id}.{secret}")
    }

    fn configured_sdk_key() -> String {
        sdk_key(CONFIGURED_CLIENT_ID, CONFIGURED_SECRET)
    }

    fn credentials(client_id: &str, secret: &str) -> SdkCredentials {
        SdkCredentials {
            client_id: client_id.to_string(),
            client_secret: secret.to_string(),
        }
    }

    #[test]
    fn parse_sdk_key_splits_on_first_dot() {
        assert_eq!(
            parse_sdk_key(&sdk_key(OTHER_CLIENT_ID, OTHER_SECRET)),
            Ok(credentials(OTHER_CLIENT_ID, "other.secret"))
        );
        assert_eq!(
            parse_sdk_key(&format!(" {} ", configured_sdk_key())),
            Ok(credentials(CONFIGURED_CLIENT_ID, CONFIGURED_SECRET))
        );
    }

    #[test]
    fn parse_sdk_key_rejects_malformed_keys() {
        for key in [
            "",
            CONFIGURED_CLIENT_ID,
            &format!("{CONFIGURED_CLIENT_ID}."),
            ".secret",
            "client.secret",
            "not-a-uuid.secret",
        ] {
            assert_eq!(parse_sdk_key(key), Err(SdkKeyError::Malformed), "{key:?}");
        }
    }

    #[test]
    fn sdk_credentials_debug_redacts_secret() {
        let debug = format!("{:?}", credentials(CONFIGURED_CLIENT_ID, "top-secret"));
        assert!(debug.contains(CONFIGURED_CLIENT_ID));
        assert!(!debug.contains("top-secret"), "{debug}");
    }

    #[test]
    fn extract_auth_requires_explicit_headers() {
        let req = TestRequest::default().to_http_request();
        assert_eq!(extract_auth_from_headers(&req), Err(SdkKeyError::Missing));

        // A non-Bearer Authorization header is not an SDK key.
        let req = TestRequest::default()
            .insert_header(("authorization", "Basic dXNlcjpwYXNz"))
            .to_http_request();
        assert_eq!(extract_auth_from_headers(&req), Err(SdkKeyError::Missing));
    }

    #[test]
    fn extract_auth_reads_bearer_sdk_key() {
        for scheme in ["Bearer", "bearer"] {
            let req = TestRequest::default()
                .insert_header((
                    "authorization",
                    format!("{scheme} {}", configured_sdk_key()),
                ))
                .to_http_request();
            assert_eq!(
                extract_auth_from_headers(&req),
                Ok(credentials(CONFIGURED_CLIENT_ID, CONFIGURED_SECRET))
            );
        }
    }

    #[test]
    fn extract_auth_reads_api_key_header() {
        let req = TestRequest::default()
            .insert_header(("x-api-key", sdk_key(OTHER_CLIENT_ID, OTHER_SECRET)))
            .to_http_request();
        assert_eq!(
            extract_auth_from_headers(&req),
            Ok(credentials(OTHER_CLIENT_ID, OTHER_SECRET))
        );
    }

    #[test]
    fn extract_auth_never_falls_back_to_bare_client_id() {
        for (name, value) in [
            ("authorization", format!("Bearer {CONFIGURED_CLIENT_ID}")),
            ("x-api-key", CONFIGURED_CLIENT_ID.to_string()),
        ] {
            let req = TestRequest::default()
                .insert_header((name, value))
                .to_http_request();
            assert_eq!(
                extract_auth_from_headers(&req),
                Err(SdkKeyError::Malformed),
                "{name}"
            );
        }
    }

    /// A client known to the mock backend.
    #[derive(Clone)]
    struct MockClient {
        secret: String,
        enabled: bool,
        client_type: &'static str,
        web_origins: Vec<String>,
    }

    impl MockClient {
        fn backend(secret: &str) -> Self {
            Self {
                secret: secret.to_string(),
                enabled: true,
                client_type: "Backend",
                web_origins: vec![],
            }
        }

        fn web(secret: &str, origins: &[&str]) -> Self {
            Self {
                client_type: "Web",
                web_origins: origins.iter().map(|o| o.to_string()).collect(),
                ..Self::backend(secret)
            }
        }
    }

    /// Minimal backend that authenticates clients the way the real one does
    /// and records every secret it receives. Knows the configured client
    /// (`CONFIGURED_CLIENT_ID` / `CONFIGURED_SECRET`, Backend type) by default.
    #[derive(Clone)]
    struct OfrepMockBackend {
        seen_secrets: Arc<std::sync::Mutex<Vec<String>>>,
        clients: Arc<std::sync::Mutex<HashMap<String, MockClient>>>,
        client_info_calls: Arc<AtomicUsize>,
        /// Keys for which `GetFeatureByKey` answers `NotFound`.
        missing_keys: Arc<std::sync::Mutex<Vec<String>>>,
        /// When set, `GetClientInfo` always fails with this code.
        client_info_error: Arc<std::sync::Mutex<Option<tonic::Code>>>,
        /// When set, `GetFeatureByKey` always fails with this code.
        feature_error: Arc<std::sync::Mutex<Option<tonic::Code>>>,
    }

    impl Default for OfrepMockBackend {
        fn default() -> Self {
            let backend = Self {
                seen_secrets: Arc::default(),
                clients: Arc::default(),
                client_info_calls: Arc::default(),
                missing_keys: Arc::default(),
                client_info_error: Arc::default(),
                feature_error: Arc::default(),
            };
            backend.set_client(CONFIGURED_CLIENT_ID, MockClient::backend(CONFIGURED_SECRET));
            backend
        }
    }

    impl OfrepMockBackend {
        fn set_client(&self, client_id: &str, client: MockClient) {
            self.clients
                .lock()
                .unwrap()
                .insert(client_id.to_string(), client);
        }

        /// Records the secret and authenticates like the real backend:
        /// empty secret `InvalidArgument`, unknown client `NotFound`, disabled
        /// client `PermissionDenied`, wrong secret `Unauthenticated`.
        #[allow(clippy::result_large_err)]
        fn authenticate(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<MockClient, tonic::Status> {
            self.seen_secrets
                .lock()
                .unwrap()
                .push(client_secret.to_string());
            if client_secret.is_empty() {
                return Err(tonic::Status::invalid_argument("client_secret is required"));
            }
            let Some(client) = self.clients.lock().unwrap().get(client_id).cloned() else {
                return Err(tonic::Status::not_found("client not found"));
            };
            if !client.enabled {
                return Err(tonic::Status::permission_denied("client is disabled"));
            }
            if client.secret != client_secret {
                return Err(tonic::Status::unauthenticated("invalid client_secret"));
            }
            Ok(client)
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
            self.authenticate(&req.client_id, &req.client_secret)?;
            if let Some(code) = *self.feature_error.lock().unwrap() {
                return Err(tonic::Status::new(code, "forced feature failure"));
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
            self.client_info_calls.fetch_add(1, Ordering::SeqCst);
            let req = request.into_inner();
            if let Some(code) = *self.client_info_error.lock().unwrap() {
                return Err(tonic::Status::new(code, "forced client info failure"));
            }
            let client = self.authenticate(&req.client_id, &req.client_secret)?;
            Ok(tonic::Response::new(backend_pb::GetClientInfoResponse {
                id: req.client_id,
                team_id: "team-1".to_string(),
                name: "test client".to_string(),
                description: String::new(),
                enabled: client.enabled,
                client_type: client.client_type.to_string(),
                web_origins: client.web_origins,
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

    /// Response of one test request, read into owned parts.
    struct TestResponse {
        status: actix_web::http::StatusCode,
        headers: actix_web::http::header::HeaderMap,
        body: serde_json::Value,
    }

    impl TestResponse {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).and_then(|value| value.to_str().ok())
        }

        fn has_no_cors_headers(&self) -> bool {
            self.header("access-control-allow-origin").is_none()
                && self.header("access-control-expose-headers").is_none()
        }
    }

    /// Send one request through the production route table.
    async fn call_routes(app_state: crate::AppState, req: actix_test::TestRequest) -> TestResponse {
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(configure_routes),
        )
        .await;
        let resp = actix_test::call_service(&service, req.to_request()).await;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = actix_test::read_body(resp).await;
        let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        TestResponse {
            status,
            headers,
            body,
        }
    }

    fn ofrep_uri(bulk: bool) -> &'static str {
        if bulk {
            "/ofrep/v1/evaluate/flags"
        } else {
            "/ofrep/v1/evaluate/flags/my-flag"
        }
    }

    /// An OFREP request (single flag or bulk) with the given headers.
    fn ofrep_request(bulk: bool, headers: &[(&str, String)]) -> actix_test::TestRequest {
        let mut req = actix_test::TestRequest::post()
            .uri(ofrep_uri(bulk))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }));
        for (name, value) in headers {
            req = req.insert_header((*name, value.clone()));
        }
        req
    }

    fn bearer(key: &str) -> (&'static str, String) {
        ("authorization", format!("Bearer {key}"))
    }

    fn assert_ofrep_auth_error(resp: &TestResponse, expected_code: &str, context: &str) {
        let expected_status = if expected_code == "FORBIDDEN" {
            actix_web::http::StatusCode::FORBIDDEN
        } else {
            actix_web::http::StatusCode::UNAUTHORIZED
        };
        assert_eq!(resp.status, expected_status, "{context}");
        assert_eq!(
            resp.body["errorCode"],
            serde_json::json!(expected_code),
            "{context}"
        );
        assert!(
            resp.body["errorDetails"]
                .as_str()
                .is_some_and(|details| !details.is_empty()),
            "{context}: body {}",
            resp.body
        );
    }

    #[actix_web::test]
    async fn ofrep_valid_sdk_key_authenticates_with_its_secret() {
        for bulk in [false, true] {
            for header in [
                bearer(&configured_sdk_key()),
                ("x-api-key", configured_sdk_key()),
            ] {
                let context = format!("bulk={bulk} header={}", header.0);
                let (app_state, backend) = ofrep_app_with_mock_backend().await;

                let resp = call_routes(app_state, ofrep_request(bulk, &[header])).await;

                assert_eq!(resp.status, actix_web::http::StatusCode::OK, "{context}");
                if !bulk {
                    assert_eq!(resp.body["key"], serde_json::json!("my-flag"));
                    assert_eq!(resp.body["value"], serde_json::json!(true));
                }
                let seen = backend.seen_secrets.lock().unwrap().clone();
                assert!(!seen.is_empty(), "{context}: expected backend calls");
                assert!(
                    seen.iter().all(|secret| secret == CONFIGURED_SECRET),
                    "{context}: backend saw secrets {seen:?}"
                );
            }
        }
    }

    #[actix_web::test]
    async fn ofrep_sdk_key_of_another_client_uses_that_clients_secret() {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        backend.set_client(OTHER_CLIENT_ID, MockClient::backend(OTHER_SECRET));

        let key = sdk_key(OTHER_CLIENT_ID, OTHER_SECRET);
        let resp = call_routes(app_state, ofrep_request(false, &[bearer(&key)])).await;

        assert_eq!(resp.status, actix_web::http::StatusCode::OK);
        let seen = backend.seen_secrets.lock().unwrap().clone();
        assert!(
            seen.iter().all(|secret| secret == OTHER_SECRET),
            "backend saw secrets {seen:?}"
        );
    }

    #[actix_web::test]
    async fn ofrep_missing_or_malformed_sdk_key_returns_401_without_backend_call() {
        let malformed = [
            vec![],
            vec![("authorization", "Basic dXNlcjpwYXNz".to_string())],
            vec![bearer(CONFIGURED_CLIENT_ID)],
            vec![("x-api-key", CONFIGURED_CLIENT_ID.to_string())],
            vec![bearer(&format!("{CONFIGURED_CLIENT_ID}."))],
            vec![bearer(".secret")],
            vec![bearer("client.secret")],
            vec![("x-api-key", "not-a-uuid.secret".to_string())],
        ];
        for bulk in [false, true] {
            for headers in &malformed {
                let context = format!("bulk={bulk} headers={headers:?}");
                let (app_state, backend) = ofrep_app_with_mock_backend().await;

                let resp = call_routes(app_state, ofrep_request(bulk, headers)).await;

                assert_ofrep_auth_error(&resp, "UNAUTHORIZED", &context);
                assert_eq!(
                    backend.client_info_calls.load(Ordering::SeqCst),
                    0,
                    "{context}"
                );
                assert!(backend.seen_secrets.lock().unwrap().is_empty(), "{context}");
            }
        }
    }

    #[actix_web::test]
    async fn ofrep_wrong_secret_returns_401_without_retry() {
        for bulk in [false, true] {
            let (mut app_state, backend) = ofrep_app_with_mock_backend().await;
            // Retries enabled: an auth error must still be tried only once.
            app_state.retry_config.max_attempts = 3;

            let key = sdk_key(CONFIGURED_CLIENT_ID, "wrong-secret");
            let resp = call_routes(app_state, ofrep_request(bulk, &[bearer(&key)])).await;

            assert_ofrep_auth_error(&resp, "UNAUTHORIZED", &format!("bulk={bulk}"));
            assert_eq!(backend.client_info_calls.load(Ordering::SeqCst), 1);
        }
    }

    #[actix_web::test]
    async fn ofrep_unknown_client_returns_401() {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        let key = sdk_key(OTHER_CLIENT_ID, OTHER_SECRET);
        let resp = call_routes(app_state, ofrep_request(false, &[bearer(&key)])).await;
        assert_ofrep_auth_error(&resp, "UNAUTHORIZED", "unknown client");
    }

    #[actix_web::test]
    async fn ofrep_wrong_secret_for_cached_client_still_fails() {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(configure_routes),
        )
        .await;

        let ok = ofrep_request(false, &[bearer(&configured_sdk_key())]).to_request();
        let resp = actix_test::call_service(&service, ok).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let wrong = sdk_key(CONFIGURED_CLIENT_ID, "wrong-secret");
        let req = ofrep_request(false, &[bearer(&wrong)]).to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::UNAUTHORIZED);

        // The good key is served from the cache; the wrong one is not.
        let ok = ofrep_request(false, &[bearer(&configured_sdk_key())]).to_request();
        let resp = actix_test::call_service(&service, ok).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        assert_eq!(backend.client_info_calls.load(Ordering::SeqCst), 2);
    }

    #[actix_web::test]
    async fn ofrep_auth_failures_are_not_cached() {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        let mut disabled = MockClient::backend(OTHER_SECRET);
        disabled.enabled = false;
        backend.set_client(OTHER_CLIENT_ID, disabled);
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(configure_routes),
        )
        .await;
        let key = sdk_key(OTHER_CLIENT_ID, OTHER_SECRET);

        let req = ofrep_request(false, &[bearer(&key)]).to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::FORBIDDEN);

        // Re-enabled client works at once: the rejection was not cached.
        backend.set_client(OTHER_CLIENT_ID, MockClient::backend(OTHER_SECRET));
        let req = ofrep_request(false, &[bearer(&key)]).to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
    }

    #[actix_web::test]
    async fn ofrep_disabled_client_returns_403() {
        for bulk in [false, true] {
            let (app_state, backend) = ofrep_app_with_mock_backend().await;
            let mut disabled = MockClient::backend(OTHER_SECRET);
            disabled.enabled = false;
            backend.set_client(OTHER_CLIENT_ID, disabled);

            let key = sdk_key(OTHER_CLIENT_ID, OTHER_SECRET);
            let resp = call_routes(app_state, ofrep_request(bulk, &[bearer(&key)])).await;

            assert_ofrep_auth_error(&resp, "FORBIDDEN", &format!("bulk={bulk}"));
        }
    }

    #[actix_web::test]
    async fn ofrep_maps_client_info_failures_to_auth_statuses() {
        use actix_web::http::StatusCode;
        let cases = [
            (tonic::Code::Unauthenticated, StatusCode::UNAUTHORIZED, 1),
            (tonic::Code::InvalidArgument, StatusCode::UNAUTHORIZED, 1),
            (tonic::Code::NotFound, StatusCode::UNAUTHORIZED, 1),
            (tonic::Code::PermissionDenied, StatusCode::FORBIDDEN, 1),
            // Only transient failures are retried, then reported as 502.
            (tonic::Code::Unavailable, StatusCode::BAD_GATEWAY, 3),
            (tonic::Code::Internal, StatusCode::BAD_GATEWAY, 3),
        ];
        for bulk in [false, true] {
            for (code, expected, attempts) in cases {
                let context = format!("bulk={bulk} code={code:?}");
                let (mut app_state, backend) = ofrep_app_with_mock_backend().await;
                app_state.retry_config.max_attempts = 2;
                *backend.client_info_error.lock().unwrap() = Some(code);

                let resp = call_routes(
                    app_state,
                    ofrep_request(bulk, &[bearer(&configured_sdk_key())]),
                )
                .await;

                assert_eq!(resp.status, expected, "{context}");
                match expected {
                    StatusCode::UNAUTHORIZED => {
                        assert_ofrep_auth_error(&resp, "UNAUTHORIZED", &context)
                    }
                    StatusCode::FORBIDDEN => assert_ofrep_auth_error(&resp, "FORBIDDEN", &context),
                    _ => {}
                }
                assert_eq!(
                    backend.client_info_calls.load(Ordering::SeqCst),
                    attempts,
                    "{context}"
                );
            }
        }
    }

    #[actix_web::test]
    async fn ofrep_feature_fetch_auth_failures_map_to_401_and_403() {
        use actix_web::http::StatusCode;
        for (code, expected) in [
            (tonic::Code::Unauthenticated, Some("UNAUTHORIZED")),
            (tonic::Code::PermissionDenied, Some("FORBIDDEN")),
            (tonic::Code::Unavailable, None),
        ] {
            let (app_state, backend) = ofrep_app_with_mock_backend().await;
            *backend.feature_error.lock().unwrap() = Some(code);

            let resp = call_routes(
                app_state,
                ofrep_request(false, &[bearer(&configured_sdk_key())]),
            )
            .await;

            match expected {
                Some(error_code) => {
                    assert_ofrep_auth_error(&resp, error_code, &format!("code={code:?}"))
                }
                None => assert_eq!(resp.status, StatusCode::BAD_GATEWAY, "code={code:?}"),
            }
        }
    }

    #[actix_web::test]
    async fn cors_preflight_allows_request_origin_without_checking_client() {
        for uri in ["/evaluate", ofrep_uri(true), ofrep_uri(false)] {
            let (app_state, backend) = ofrep_app_with_mock_backend().await;
            let req = actix_test::TestRequest::default()
                .method(actix_web::http::Method::OPTIONS)
                .uri(uri)
                .insert_header(("origin", "https://any.example.org"))
                .insert_header(("access-control-request-method", "POST"))
                .insert_header(("access-control-request-headers", "authorization"));

            let resp = call_routes(app_state, req).await;

            assert_eq!(
                resp.status,
                actix_web::http::StatusCode::NO_CONTENT,
                "{uri}"
            );
            assert_eq!(
                resp.header("access-control-allow-origin"),
                Some("https://any.example.org"),
                "{uri}"
            );
            assert_eq!(
                resp.header("access-control-allow-methods"),
                Some("POST, OPTIONS"),
                "{uri}"
            );
            assert_eq!(
                resp.header("access-control-allow-headers"),
                Some("Authorization, X-API-Key, Content-Type, If-None-Match"),
                "{uri}"
            );
            assert_eq!(resp.header("access-control-max-age"), Some("600"), "{uri}");
            assert_eq!(resp.header("vary"), Some("Origin"), "{uri}");
            assert_eq!(backend.client_info_calls.load(Ordering::SeqCst), 0, "{uri}");
        }
    }

    /// App state whose backend knows `OTHER_CLIENT_ID` as a Web client
    /// allowed for `ALLOWED_ORIGIN`, with one cached team flag.
    async fn web_client_app() -> crate::AppState {
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        backend.set_client(
            OTHER_CLIENT_ID,
            MockClient::web(OTHER_SECRET, &[ALLOWED_ORIGIN]),
        );
        cache_fetched_feature(&app_state, &team_feature("f-1", "my-flag", "team-1", true)).await;
        app_state.mapped_cache.run_pending_tasks().await;
        app_state
    }

    fn web_client_headers(origin: Option<&str>) -> Vec<(&'static str, String)> {
        let mut headers = vec![bearer(&sdk_key(OTHER_CLIENT_ID, OTHER_SECRET))];
        if let Some(origin) = origin {
            headers.push(("origin", origin.to_string()));
        }
        headers
    }

    #[actix_web::test]
    async fn ofrep_web_client_allowed_origin_gets_cors_headers() {
        for bulk in [false, true] {
            let context = format!("bulk={bulk}");
            let req = ofrep_request(bulk, &web_client_headers(Some(ALLOWED_ORIGIN)));

            let resp = call_routes(web_client_app().await, req).await;

            assert_eq!(resp.status, actix_web::http::StatusCode::OK, "{context}");
            assert_eq!(
                resp.header("access-control-allow-origin"),
                Some(ALLOWED_ORIGIN),
                "{context}"
            );
            assert_eq!(
                resp.header("access-control-expose-headers"),
                Some("ETag"),
                "{context}"
            );
            assert_eq!(resp.header("vary"), Some("Origin"), "{context}");
            if bulk {
                assert!(resp.header("etag").is_some(), "bulk response has an ETag");
            }
        }
    }

    #[actix_web::test]
    async fn ofrep_web_client_not_modified_keeps_cors_headers() {
        let app_state = web_client_app().await;
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .configure(configure_routes),
        )
        .await;
        let headers = web_client_headers(Some(ALLOWED_ORIGIN));
        let resp =
            actix_test::call_service(&service, ofrep_request(true, &headers).to_request()).await;
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .expect("ETag")
            .to_string();

        let req = ofrep_request(true, &headers).insert_header(("if-none-match", etag));
        let resp = actix_test::call_service(&service, req.to_request()).await;

        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_MODIFIED);
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some(ALLOWED_ORIGIN)
        );
    }

    #[actix_web::test]
    async fn ofrep_web_client_disallowed_or_missing_origin_returns_403() {
        for bulk in [false, true] {
            for origin in [Some("https://evil.example.org"), None] {
                let context = format!("bulk={bulk} origin={origin:?}");
                let req = ofrep_request(bulk, &web_client_headers(origin));

                let resp = call_routes(web_client_app().await, req).await;

                assert_ofrep_auth_error(&resp, "FORBIDDEN", &context);
                assert!(resp.has_no_cors_headers(), "{context}");
            }
        }
    }

    #[actix_web::test]
    async fn ofrep_backend_client_gets_no_cors_headers() {
        for bulk in [false, true] {
            let (app_state, _backend) = ofrep_app_with_mock_backend().await;
            let req = ofrep_request(
                bulk,
                &[
                    bearer(&configured_sdk_key()),
                    ("origin", ALLOWED_ORIGIN.to_string()),
                ],
            );

            let resp = call_routes(app_state, req).await;

            assert_eq!(resp.status, actix_web::http::StatusCode::OK, "bulk={bulk}");
            assert!(resp.has_no_cors_headers(), "bulk={bulk}");
        }
    }

    fn evaluate_request(origin: Option<&str>) -> actix_test::TestRequest {
        let req = actix_test::TestRequest::post()
            .uri("/evaluate")
            .set_json(serde_json::json!({
                "flagKey": "my-flag",
                "context": { "bucketingKey": "u1" }
            }));
        match origin {
            Some(origin) => req.insert_header(("origin", origin)),
            None => req,
        }
    }

    #[actix_web::test]
    async fn evaluate_applies_cors_rules_of_configured_client() {
        use actix_web::http::StatusCode;

        // Configured client is a Web client.
        let (app_state, backend) = ofrep_app_with_mock_backend().await;
        backend.set_client(
            CONFIGURED_CLIENT_ID,
            MockClient::web(CONFIGURED_SECRET, &[ALLOWED_ORIGIN]),
        );
        let resp = call_routes(app_state.clone(), evaluate_request(Some(ALLOWED_ORIGIN))).await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(resp.body["value"], serde_json::json!(true));
        assert_eq!(
            resp.header("access-control-allow-origin"),
            Some(ALLOWED_ORIGIN)
        );
        assert_eq!(resp.header("access-control-expose-headers"), Some("ETag"));
        assert_eq!(resp.header("vary"), Some("Origin"));

        for origin in [Some("https://evil.example.org"), None] {
            let resp = call_routes(app_state.clone(), evaluate_request(origin)).await;
            assert_eq!(resp.status, StatusCode::FORBIDDEN, "origin={origin:?}");
            assert_eq!(resp.body["error"], serde_json::json!("FORBIDDEN"));
            assert!(resp.has_no_cors_headers(), "origin={origin:?}");
        }

        // Configured client is a Backend client: served, no CORS headers.
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        let resp = call_routes(app_state, evaluate_request(Some(ALLOWED_ORIGIN))).await;
        assert_eq!(resp.status, StatusCode::OK);
        assert!(resp.has_no_cors_headers());
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
            .insert_header(("x-api-key", configured_sdk_key()))
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
            .insert_header(("authorization", format!("Bearer {}", configured_sdk_key())))
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
            .insert_header(("authorization", format!("Bearer {}", configured_sdk_key())))
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
            .insert_header(("authorization", format!("Bearer {}", configured_sdk_key())))
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
            .insert_header(("authorization", format!("Bearer {}", configured_sdk_key())))
            .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
            .to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags/missing-flag")
            .insert_header(("authorization", format!("Bearer {}", configured_sdk_key())))
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

    /// A bulk OFREP request for `context`, optionally with `If-None-Match`.
    fn bulk_request(
        context: &serde_json::Value,
        if_none_match: Option<&str>,
    ) -> actix_test::TestRequest {
        let req = actix_test::TestRequest::post()
            .uri("/ofrep/v1/evaluate/flags")
            .insert_header(("x-api-key", configured_sdk_key()))
            .set_json(serde_json::json!({ "context": context }));
        match if_none_match {
            Some(value) => req.insert_header(("if-none-match", value)),
            None => req,
        }
    }

    fn status_and_etag(
        resp: &actix_web::dev::ServiceResponse,
    ) -> (actix_web::http::StatusCode, Option<String>) {
        let etag = resp
            .headers()
            .get(actix_web::http::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        (resp.status(), etag)
    }

    #[actix_web::test]
    async fn ofrep_bulk_returns_304_only_for_the_same_context() {
        use actix_web::http::StatusCode;

        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        cache_fetched_feature(&app_state, &team_feature("f-1", "alpha", "team-1", true)).await;
        app_state.mapped_cache.run_pending_tasks().await;
        let service =
            actix_test::init_service(App::new().app_data(web::Data::new(app_state)).route(
                "/ofrep/v1/evaluate/flags",
                web::post().to(ofrep_evaluate_flags_bulk),
            ))
            .await;

        let anon = serde_json::json!({ "targetingKey": "anon-1" });
        let resp = actix_test::call_service(&service, bulk_request(&anon, None).to_request()).await;
        let (status, etag) = status_and_etag(&resp);
        assert_eq!(status, StatusCode::OK);
        let etag = etag.expect("bulk response carries an ETag");

        // Same config, other user: must re-evaluate.
        let user = serde_json::json!({ "targetingKey": "u-42" });
        let req = bulk_request(&user, Some(&etag)).to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "new targetingKey must not get 304"
        );

        // Same config and user, other attributes: must re-evaluate.
        let with_plan = serde_json::json!({ "targetingKey": "anon-1", "plan": "premium" });
        let req = bulk_request(&with_plan, Some(&etag)).to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "new attributes must not get 304"
        );

        // Identical repeat request: 304, with the ETag header.
        let req = bulk_request(&anon, Some(&etag)).to_request();
        let resp = actix_test::call_service(&service, req).await;
        let (status, not_modified_etag) = status_and_etag(&resp);
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert_eq!(not_modified_etag.as_deref(), Some(etag.as_str()));

        // `*` must not short-circuit a POST evaluation.
        let req = bulk_request(&anon, Some("*")).to_request();
        let resp = actix_test::call_service(&service, req).await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "If-None-Match: * must not get 304"
        );
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

        let features = [feature_a.clone(), feature_b.clone()];
        let context = ofrep_context("anon-1", &[("plan", "free"), ("country", "SE")]);

        let etag = ofrep_bulk_etag(&features, "env-1", &context);
        let reversed_etag = ofrep_bulk_etag(&[feature_b, feature_a], "env-1", &context);

        assert_eq!(etag, reversed_etag);
        assert!(if_none_match_contains(&etag, &etag));
        assert!(if_none_match_contains(&format!("\"{etag}\""), &etag));
        assert!(if_none_match_contains(&format!("stale, \"{etag}\""), &etag));

        // Context and environment are part of the ETag.
        let other_user = ofrep_context("u-42", &[("plan", "free"), ("country", "SE")]);
        assert_ne!(etag, ofrep_bulk_etag(&features, "env-1", &other_user));
        let other_attributes = ofrep_context("anon-1", &[("plan", "premium"), ("country", "SE")]);
        assert_ne!(etag, ofrep_bulk_etag(&features, "env-1", &other_attributes));
        let fewer_attributes = ofrep_context("anon-1", &[("plan", "free")]);
        assert_ne!(etag, ofrep_bulk_etag(&features, "env-1", &fewer_attributes));
        assert_ne!(etag, ofrep_bulk_etag(&features, "env-2", &context));

        // Attribute insertion order does not matter.
        for _ in 0..16 {
            let reordered = ofrep_context("anon-1", &[("country", "SE"), ("plan", "free")]);
            assert_eq!(etag, ofrep_bulk_etag(&features, "env-1", &reordered));
        }

        // Field boundaries are unambiguous.
        assert_ne!(
            ofrep_bulk_etag(&features, "env-", &ofrep_context("1", &[])),
            ofrep_bulk_etag(&features, "env", &ofrep_context("-1", &[]))
        );
    }

    fn ofrep_context(targeting_key: &str, attributes: &[(&str, &str)]) -> OFREPContext {
        OFREPContext {
            targeting_key: targeting_key.to_string(),
            attributes: attributes
                .iter()
                .map(|(key, value)| (key.to_string(), serde_json::json!(value)))
                .collect(),
        }
    }

    #[test]
    fn if_none_match_wildcard_does_not_match() {
        assert!(!if_none_match_contains("*", "abc"));
        assert!(!if_none_match_contains("\"*\"", "abc"));
        assert!(if_none_match_contains("*, \"abc\"", "abc"));
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

    /// Sticky results: a truthy result is served from the assignment cache
    /// until the feature's assignments are purged.
    #[actix_web::test]
    async fn sticky_results_are_served_until_the_feature_is_purged() {
        let (app_state, _backend) = ofrep_app_with_mock_backend().await;
        cache_fetched_feature(&app_state, &team_feature("f-1", "sticky", "team-1", true)).await;
        app_state.mapped_cache.run_pending_tasks().await;
        let service = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state.clone()))
                .route("/evaluate", web::post().to(evaluate_handler))
                .route(
                    "/ofrep/v1/evaluate/flags/{key}",
                    web::post().to(ofrep_evaluate_flag),
                ),
        )
        .await;
        let values = || async {
            let req = actix_test::TestRequest::post()
                .uri("/evaluate")
                .set_json(serde_json::json!({
                    "flagKey": "sticky",
                    "context": { "bucketingKey": "u1" }
                }))
                .to_request();
            let evaluate: serde_json::Value =
                actix_test::call_and_read_body_json(&service, req).await;
            let req = actix_test::TestRequest::post()
                .uri("/ofrep/v1/evaluate/flags/sticky")
                .insert_header(("x-api-key", configured_sdk_key()))
                .set_json(serde_json::json!({ "context": { "targetingKey": "u1" } }))
                .to_request();
            let ofrep: serde_json::Value = actix_test::call_and_read_body_json(&service, req).await;
            (evaluate["value"].clone(), ofrep["value"].clone())
        };

        let fresh = values().await;
        assert_eq!(fresh, (serde_json::json!(true), serde_json::json!(true)));

        // Target only premium users without purging: the sticky result is
        // still served to u1, who has no `plan` attribute.
        let mut premium_only = team_feature("f-1", "sticky", "team-1", true);
        premium_only.stages[0].criterias = vec![pb::StageCriterionFull {
            id: "criterion-1".to_string(),
            stage_id: premium_only.stages[0].id.clone(),
            priority: 0,
            rule_groups: vec![pb::RuleGroup {
                id: "group-1".to_string(),
                logic_operator: "AND".to_string(),
                conditions: vec![pb::RuleCondition {
                    id: "condition-1".to_string(),
                    context_key: "plan".to_string(),
                    operator: "EQUALS".to_string(),
                    value: "\"premium\"".to_string(),
                    order_index: 0,
                }],
            }],
            variant_allocations: vec![],
            variant_selection_mode: String::new(),
            selected_variant_control: String::new(),
        }];
        cache_fetched_feature(&app_state, &premium_only).await;
        app_state.mapped_cache.run_pending_tasks().await;
        let cached = values().await;
        assert_eq!(cached, (serde_json::json!(true), serde_json::json!(true)));

        // After a purge, both endpoints evaluate the new config.
        app_state.purge_assignments_for_feature("f-1").await;
        let purged = values().await;
        assert_eq!(purged, (serde_json::json!(false), serde_json::json!(false)));
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
