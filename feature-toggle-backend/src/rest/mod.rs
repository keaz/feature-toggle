pub mod approval;
pub mod auth;
pub mod client;
pub mod context;
pub mod criteria;
pub mod environment;
pub mod error;
pub mod feature;
pub mod jwt_secret;
pub mod metrics;
pub mod notification;
pub mod pagination;
pub mod pipeline;
pub mod role;
pub mod serde;
pub mod stream;
pub mod system_client;
pub mod team;
pub mod types;
pub mod user;

use actix_web::{HttpResponse, Responder, get, web};
use utoipa::openapi::{
    ObjectBuilder, Required, Schema,
    path::{Operation, ParameterBuilder, ParameterIn},
    schema::Type,
    security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme},
};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

use crate::rest::approval::{
    AppliesTo, ApprovalActionRequest, ApprovalPolicyResponse, ApprovalRequestListQuery,
    ApprovalRequestResponse, ApprovalRequestStatus, ApprovalRequestsResponse, ApprovalVoteResponse,
    CreateApprovalPolicyRequest, UpdateApprovalPolicyRequest,
};
use crate::rest::auth::{
    AuthStatusResponse, LoginRequest, LoginResponse, ResetPasswordRequest,
    SetTemporaryPasswordRequest,
};
use crate::rest::client::{
    ClientListQuery, ClientResponse, ClientType, ClientsResponse, CreateClientRequest,
    UpdateClientRequest,
};
use crate::rest::context::{
    ContextEntryResponse, ContextListQuery, ContextResponse, ContextsResponse,
    CreateContextRequest, UpdateContextRequest,
};
use crate::rest::criteria::{
    CompoundRuleConditionResponse, CompoundRuleGroupResponse, CreateRuleConditionRequest,
    CreateRuleGroupRequest, CreateStageCriterionRequest, CreateVariantAllocationRequest,
    InlineRuleGroupRequest, LogicOperator, RuleOperator, SetVariantAllocationsRequest,
    StageCriterionResponse, UpdateRuleGroupRequest, VariantAllocationResponse,
    VariantSelectionMode,
};
use crate::rest::environment::{
    CreateEnvironmentRequest, EnvironmentListQuery, EnvironmentResponse, EnvironmentsResponse,
    UpdateEnvironmentRequest,
};
use crate::rest::error::ErrorResponse;
use crate::rest::feature::{
    CreateFeatureRequest, CreateFeatureStageRequest, CreateFeatureVariantRequest,
    EmergencyDisableRequest, FeatureListQuery, FeatureRelationshipResponse, FeatureResponse,
    FeatureRolloutQuery, FeatureStageResponse, FeatureType, FeatureVariantResponse,
    FeatureVersionDiffEntryResponse, FeatureVersionDiffResponse, FeatureVersionResponse,
    FeatureVersionsResponse, FeaturesResponse, LifecycleStage, RollbackFeatureVersionRequest,
    RolloutMetricsQuery, RolloutMetricsResponse, StageChangeRequest, StageChangeRequestBody,
    UpdateFeatureRequest, VariantValueType,
};
use crate::rest::jwt_secret::JwtSecretResponse;
use crate::rest::metrics::{
    ActivityEntityDetailsResponse, ActivityLogPageResponse, ActivityLogResponse,
    ActivityRecentQuery, AnalyzeCanaryGateRequest, CanaryAnalysisResponse, CanaryDirection,
    CanaryGateConfigRequest, CanaryGateResponse, CanaryVariantSnapshotResponse,
    CreateMetricRequest, EvaluationByFeatureResponse, EvaluationCountQuery, EvaluationRateResponse,
    EvaluationRatesQuery, EvaluationSummaryQuery, EvaluationSummaryResponse,
    EvaluationsByFeatureQuery, EvaluationsByFeatureResponse, ExperimentAnalysisResponse,
    ExperimentResultsQuery, FeatureGrowthQuery, FeatureGrowthResponse, MetricAnalysisResponse,
    MetricResponse, MetricResultResponse, MetricsByFeatureQuery, MetricsResponse,
    SetCanaryGatesRequest, SystemMetricsResponse, TrackMetricEventRequest, TrackMetricsRequest,
    TrackMetricsResponse,
};
use crate::rest::notification::{
    NotificationChannelConfigResponse, NotificationPreferenceResponse,
    NotificationSettingsResponse, UpdateNotificationChannelConfigRequest,
    UpdateNotificationPreferenceRequest,
};
use crate::rest::pagination::{PageMeta, PaginationQuery};
use crate::rest::pipeline::{
    CreatePipelineRequest, CreateRelationshipRequest, CreateStageRequest, PipelineListQuery,
    PipelineRelationshipResponse, PipelineResponse, PipelineStageResponse, PipelinesResponse,
    UpdatePipelineRequest,
};
use crate::rest::role::{CreateRoleRequest, RoleResponse};
use crate::rest::system_client::{
    CreateSystemClientRequest, SystemClientListQuery, SystemClientResponse,
    SystemClientWithTokenResponse, SystemClientsResponse, UpdateSystemClientRequest,
};
use crate::rest::team::{CreateTeamRequest, TeamResponse, UpdateTeamRequest};
use crate::rest::types::HealthResponse;
use crate::rest::user::{
    AssignUserRolesRequest, AssignUserTeamsRequest, CreateUserRequest, UpdateUserRequest,
    UserListQuery, UserResponse, UsersResponse,
};

#[utoipa::path(
    get,
    path = "/api/v1/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 500, description = "Server error", body = ErrorResponse)
    ),
    security(()),
    tag = "System"
)]
#[get("/health")]
async fn health() -> impl Responder {
    HttpResponse::Ok().json(HealthResponse::ok())
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        environment::list_environments,
        environment::get_environment,
        environment::create_environment,
        environment::update_environment,
        environment::delete_environment,
        context::list_contexts,
        context::get_context,
        context::create_context,
        context::update_context,
        context::delete_context,
        client::list_clients,
        client::get_client,
        client::create_client,
        client::update_client,
        system_client::list_system_clients,
        system_client::get_system_client,
        system_client::create_system_client,
        system_client::update_system_client,
        system_client::regenerate_system_client_token,
        pipeline::list_pipelines,
        pipeline::get_pipeline,
        pipeline::create_pipeline,
        pipeline::update_pipeline,
        feature::list_features,
        feature::get_feature,
        feature::list_feature_versions,
        feature::get_feature_version_diff,
        feature::rollback_feature_version,
        feature::create_feature,
        feature::update_feature,
        feature::emergency_disable_feature,
        feature::emergency_enable_feature,
        feature::request_stage_change,
        feature::pending_approvals,
        feature::active_kill_switches,
        feature::rollout_metrics,
        approval::list_approval_requests,
        approval::approve_request,
        approval::reject_request,
        approval::cancel_request,
        approval::list_approval_policies,
        approval::get_approval_policy,
        approval::create_approval_policy,
        approval::update_approval_policy,
        approval::delete_approval_policy,
        role::list_roles,
        role::create_role,
        role::delete_role,
        team::list_teams,
        team::create_team,
        team::update_team,
        user::list_users,
        user::get_user,
        user::create_user,
        user::create_admin,
        user::update_user,
        user::assign_user_teams,
        user::get_user_roles,
        user::assign_user_roles,
        auth::login,
        auth::logout,
        auth::reset_password,
        auth::set_temporary_password,
        auth::auth_status,
        jwt_secret::list_jwt_secrets,
        jwt_secret::generate_jwt_secret,
        jwt_secret::deactivate_all_jwt_secrets,
        criteria::get_stage_criteria,
        criteria::set_stage_criteria,
        criteria::set_variant_allocations,
        criteria::create_rule_group,
        criteria::update_rule_group,
        criteria::delete_rule_group,
        metrics::list_metrics,
        metrics::create_metric,
        metrics::metrics_by_feature,
        metrics::experiment_results,
        metrics::evaluation_summary,
        metrics::evaluation_rates,
        metrics::evaluations_by_feature,
        metrics::evaluation_count,
        metrics::feature_growth,
        metrics::recent_activity,
        metrics::system_metrics,
        metrics::track_metrics,
        metrics::list_canary_gates,
        metrics::replace_canary_gates,
        metrics::analyze_canary_gate,
        notification::get_notification_settings,
        notification::update_notification_channel,
        notification::update_notification_preference
    ),
    components(schemas(
        HealthResponse,
        ErrorResponse,
        PaginationQuery,
        PageMeta,
        EnvironmentListQuery,
        EnvironmentResponse,
        EnvironmentsResponse,
        CreateEnvironmentRequest,
        UpdateEnvironmentRequest,
        ContextListQuery,
        ContextResponse,
        ContextsResponse,
        ContextEntryResponse,
        CreateContextRequest,
        UpdateContextRequest,
        ClientListQuery,
        ClientResponse,
        ClientsResponse,
        CreateClientRequest,
        UpdateClientRequest,
        ClientType,
        SystemClientListQuery,
        SystemClientResponse,
        SystemClientsResponse,
        CreateSystemClientRequest,
        UpdateSystemClientRequest,
        SystemClientWithTokenResponse,
        PipelineListQuery,
        PipelineResponse,
        PipelinesResponse,
        PipelineStageResponse,
        PipelineRelationshipResponse,
        CreatePipelineRequest,
        UpdatePipelineRequest,
        CreateStageRequest,
        CreateRelationshipRequest,
        FeatureListQuery,
        FeatureRolloutQuery,
        RolloutMetricsQuery,
        FeatureResponse,
        FeaturesResponse,
        FeatureRelationshipResponse,
        FeatureStageResponse,
        FeatureVariantResponse,
        FeatureVersionDiffEntryResponse,
        FeatureVersionDiffResponse,
        FeatureVersionResponse,
        FeatureVersionsResponse,
        CreateFeatureRequest,
        UpdateFeatureRequest,
        RollbackFeatureVersionRequest,
        CreateFeatureStageRequest,
        CreateFeatureVariantRequest,
        EmergencyDisableRequest,
        StageChangeRequestBody,
        StageChangeRequest,
        FeatureType,
        LifecycleStage,
        VariantValueType,
        RolloutMetricsResponse,
        ApprovalRequestListQuery,
        ApprovalRequestStatus,
        ApprovalVoteResponse,
        ApprovalRequestResponse,
        ApprovalRequestsResponse,
        ApprovalActionRequest,
        ApprovalPolicyResponse,
        CreateApprovalPolicyRequest,
        UpdateApprovalPolicyRequest,
        AppliesTo,
        RoleResponse,
        CreateRoleRequest,
        TeamResponse,
        CreateTeamRequest,
        UpdateTeamRequest,
        UserListQuery,
        UserResponse,
        UsersResponse,
        CreateUserRequest,
        UpdateUserRequest,
        AssignUserTeamsRequest,
        AssignUserRolesRequest,
        LoginRequest,
        LoginResponse,
        ResetPasswordRequest,
        SetTemporaryPasswordRequest,
        AuthStatusResponse,
        JwtSecretResponse,
        StageCriterionResponse,
        VariantAllocationResponse,
        CompoundRuleConditionResponse,
        CompoundRuleGroupResponse,
        CreateStageCriterionRequest,
        CreateVariantAllocationRequest,
        InlineRuleGroupRequest,
        CreateRuleConditionRequest,
        SetVariantAllocationsRequest,
        CreateRuleGroupRequest,
        UpdateRuleGroupRequest,
        RuleOperator,
        LogicOperator,
        VariantSelectionMode,
        MetricsByFeatureQuery,
        ExperimentResultsQuery,
        EvaluationSummaryQuery,
        EvaluationRatesQuery,
        EvaluationsByFeatureQuery,
        EvaluationCountQuery,
        FeatureGrowthQuery,
        ActivityRecentQuery,
        MetricResponse,
        MetricsResponse,
        CreateMetricRequest,
        metrics::MetricType,
        MetricResultResponse,
        MetricAnalysisResponse,
        ExperimentAnalysisResponse,
        EvaluationRateResponse,
        EvaluationSummaryResponse,
        EvaluationByFeatureResponse,
        EvaluationsByFeatureResponse,
        FeatureGrowthResponse,
        ActivityEntityDetailsResponse,
        ActivityLogResponse,
        ActivityLogPageResponse,
        SystemMetricsResponse,
        TrackMetricsResponse,
        TrackMetricEventRequest,
        TrackMetricsRequest,
        CanaryDirection,
        CanaryGateConfigRequest,
        SetCanaryGatesRequest,
        AnalyzeCanaryGateRequest,
        CanaryGateResponse,
        CanaryVariantSnapshotResponse,
        CanaryAnalysisResponse,
        NotificationChannelConfigResponse,
        NotificationPreferenceResponse,
        NotificationSettingsResponse,
        UpdateNotificationChannelConfigRequest,
        UpdateNotificationPreferenceRequest
    )),
    modifiers(&SecurityAddon),
    security(
        ("bearer_auth" = [])
    ),
    tags(
        (name = "System", description = "System health and metadata"),
        (name = "Environments", description = "Environment management"),
        (name = "Contexts", description = "Context management"),
        (name = "Clients", description = "Client management"),
        (name = "System Clients", description = "System automation clients and JWT token management"),
        (name = "Pipelines", description = "Pipeline management"),
        (name = "Features", description = "Feature management and rollout"),
        (name = "Criteria", description = "Stage criteria and rule groups"),
        (name = "Approvals", description = "Approval requests and policies"),
        (name = "Roles", description = "Role management"),
        (name = "Teams", description = "Team management"),
        (name = "Users", description = "User management"),
        (name = "Auth", description = "Authentication"),
        (name = "Metrics", description = "Metrics and analytics"),
        (name = "Activity", description = "Activity logs"),
        (name = "Notifications", description = "Notification settings and delivery preferences")
    )
)]
pub struct ApiDoc;

struct SecurityAddon;

fn is_public_operation(path: &str, method: &str) -> bool {
    matches!(
        (path, method),
        ("/api/v1/health", "GET")
            | ("/api/v1/metrics/track", "POST")
            | ("/api/v1/auth/login", "POST")
            | ("/api/v1/auth/status", "GET")
            | ("/api/v1/admins", "POST")
    )
}

fn set_operation_security(path: &str, method: &str, operation: &mut Option<Operation>) {
    let Some(operation) = operation.as_mut() else {
        return;
    };

    if is_public_operation(path, method) {
        operation.security = Some(Vec::new());
        remove_bearer_header_parameter(operation);
    } else {
        operation.security = Some(vec![SecurityRequirement::new(
            "bearer_auth",
            Vec::<String>::new(),
        )]);
        ensure_bearer_header_parameter(operation);
    }
}

fn is_authorization_header(parameter: &utoipa::openapi::path::Parameter) -> bool {
    matches!(parameter.parameter_in, ParameterIn::Header)
        && parameter.name.eq_ignore_ascii_case("Authorization")
}

fn remove_bearer_header_parameter(operation: &mut Operation) {
    if let Some(parameters) = operation.parameters.as_mut() {
        parameters.retain(|parameter| !is_authorization_header(parameter));
        if parameters.is_empty() {
            operation.parameters = None;
        }
    }
}

fn ensure_bearer_header_parameter(operation: &mut Operation) {
    if operation
        .parameters
        .as_ref()
        .is_some_and(|parameters| parameters.iter().any(is_authorization_header))
    {
        return;
    }

    let authorization_header = ParameterBuilder::new()
        .name("Authorization")
        .parameter_in(ParameterIn::Header)
        .required(Required::True)
        .description(Some("Bearer access token. Format: Bearer <token>"))
        .schema(Some(Schema::Object(
            ObjectBuilder::new().schema_type(Type::String).build(),
        )))
        .build();

    if let Some(parameters) = operation.parameters.as_mut() {
        parameters.push(authorization_header);
    } else {
        operation.parameters = Some(vec![authorization_header]);
    }
}

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }

        for (path, path_item) in openapi.paths.paths.iter_mut() {
            set_operation_security(path, "GET", &mut path_item.get);
            set_operation_security(path, "POST", &mut path_item.post);
            set_operation_security(path, "PUT", &mut path_item.put);
            set_operation_security(path, "PATCH", &mut path_item.patch);
            set_operation_security(path, "DELETE", &mut path_item.delete);
            set_operation_security(path, "OPTIONS", &mut path_item.options);
            set_operation_security(path, "HEAD", &mut path_item.head);
            set_operation_security(path, "TRACE", &mut path_item.trace);
        }
    }
}

async fn get_openapi() -> impl Responder {
    HttpResponse::Ok().json(ApiDoc::openapi())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            .service(health)
            .route("/openapi.json", web::get().to(get_openapi))
            .configure(environment::configure)
            .configure(context::configure)
            .configure(client::configure)
            .configure(system_client::configure)
            .configure(pipeline::configure)
            .configure(feature::configure)
            .configure(criteria::configure)
            .configure(metrics::configure)
            .configure(approval::configure)
            .configure(role::configure)
            .configure(team::configure)
            .configure(user::configure)
            .configure(auth::configure)
            .configure(jwt_secret::configure)
            .configure(notification::configure)
            .configure(stream::configure),
    );
}

pub fn swagger_ui() -> SwaggerUi {
    SwaggerUi::new("/docs/{_:.*}").url("/api/v1/openapi.json", ApiDoc::openapi())
}
