pub mod error;
pub mod environment;
pub mod context;
pub mod pipeline;
pub mod client;
pub mod feature;
pub mod criteria;
pub mod approval;
pub mod role;
pub mod team;
pub mod user;
pub mod auth;
pub mod jwt_secret;
pub mod pagination;
pub mod types;
pub mod metrics;

use actix_web::{get, web, HttpResponse, Responder};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::rest::error::ErrorResponse;
use crate::rest::environment::{
    CreateEnvironmentRequest, EnvironmentListQuery, EnvironmentResponse, EnvironmentsResponse,
    UpdateEnvironmentRequest,
};
use crate::rest::context::{
    ContextListQuery, ContextResponse, ContextsResponse, ContextEntryResponse, CreateContextRequest,
    UpdateContextRequest,
};
use crate::rest::client::{
    ClientListQuery, ClientResponse, ClientsResponse, CreateClientRequest, UpdateClientRequest,
    ClientType,
};
use crate::rest::pipeline::{
    CreatePipelineRequest, CreateRelationshipRequest, CreateStageRequest, PipelineListQuery,
    PipelineRelationshipResponse, PipelineResponse, PipelineStageResponse, PipelinesResponse,
    UpdatePipelineRequest,
};
use crate::rest::feature::{
    FeatureListQuery, FeatureRolloutQuery, RolloutMetricsQuery, FeatureResponse, FeaturesResponse,
    FeatureRelationshipResponse, FeatureStageResponse, FeatureVariantResponse, CreateFeatureRequest,
    UpdateFeatureRequest, CreateFeatureStageRequest, CreateFeatureVariantRequest,
    EmergencyDisableRequest, StageChangeRequestBody, StageChangeRequest, FeatureType,
    LifecycleStage, VariantValueType, RolloutMetricsResponse,
};
use crate::rest::criteria::{
    CreateRuleConditionRequest, CreateRuleGroupRequest, CreateStageCriterionRequest,
    CreateVariantAllocationRequest, InlineRuleGroupRequest, LogicOperator, RuleOperator,
    SetVariantAllocationsRequest, StageCriterionResponse, VariantAllocationResponse,
    CompoundRuleConditionResponse, CompoundRuleGroupResponse, UpdateRuleGroupRequest,
    VariantSelectionMode,
};
use crate::rest::approval::{
    ApprovalActionRequest, ApprovalPolicyResponse, ApprovalRequestListQuery,
    ApprovalRequestResponse, ApprovalRequestStatus, ApprovalRequestsResponse, ApprovalVoteResponse,
    AppliesTo, CreateApprovalPolicyRequest, UpdateApprovalPolicyRequest,
};
use crate::rest::role::{CreateRoleRequest, RoleResponse};
use crate::rest::team::{TeamResponse, CreateTeamRequest, UpdateTeamRequest};
use crate::rest::user::{
    UserListQuery, UserResponse, UsersResponse, CreateUserRequest, UpdateUserRequest,
    AssignUserTeamsRequest, AssignUserRolesRequest,
};
use crate::rest::auth::{
    LoginRequest, LoginResponse, ResetPasswordRequest, SetTemporaryPasswordRequest,
    AuthStatusResponse,
};
use crate::rest::jwt_secret::JwtSecretResponse;
use crate::rest::pagination::{PageMeta, PaginationQuery};
use crate::rest::types::HealthResponse;
use crate::rest::metrics::{
    MetricsByFeatureQuery, ExperimentResultsQuery, EvaluationSummaryQuery, EvaluationRatesQuery,
    EvaluationsByFeatureQuery, EvaluationCountQuery, FeatureGrowthQuery, ActivityRecentQuery,
    MetricResponse, MetricsResponse, CreateMetricRequest, MetricResultResponse,
    MetricAnalysisResponse, ExperimentAnalysisResponse, EvaluationRateResponse,
    EvaluationSummaryResponse, EvaluationByFeatureResponse, EvaluationsByFeatureResponse,
    FeatureGrowthResponse, ActivityEntityDetailsResponse, ActivityLogResponse,
    ActivityLogPageResponse, SystemMetricsResponse, TrackMetricsResponse,
    TrackMetricEventRequest, TrackMetricsRequest,
};

#[utoipa::path(
    get,
    path = "/api/v1/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 500, description = "Server error", body = ErrorResponse)
    ),
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
        pipeline::list_pipelines,
        pipeline::get_pipeline,
        pipeline::create_pipeline,
        pipeline::update_pipeline,
        feature::list_features,
        feature::get_feature,
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
        metrics::track_metrics
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
        CreateFeatureRequest,
        UpdateFeatureRequest,
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
        TrackMetricsRequest
    )),
    tags(
        (name = "System", description = "System health and metadata"),
        (name = "Environments", description = "Environment management"),
        (name = "Contexts", description = "Context management"),
        (name = "Clients", description = "Client management"),
        (name = "Pipelines", description = "Pipeline management"),
        (name = "Features", description = "Feature management and rollout"),
        (name = "Criteria", description = "Stage criteria and rule groups"),
        (name = "Approvals", description = "Approval requests and policies"),
        (name = "Roles", description = "Role management"),
        (name = "Teams", description = "Team management"),
        (name = "Users", description = "User management"),
        (name = "Auth", description = "Authentication"),
        (name = "Metrics", description = "Metrics and analytics"),
        (name = "Activity", description = "Activity logs")
    )
)]
pub struct ApiDoc;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            .service(health)
            .configure(environment::configure)
            .configure(context::configure)
            .configure(client::configure)
            .configure(pipeline::configure)
            .configure(feature::configure)
            .configure(criteria::configure)
            .configure(metrics::configure)
            .configure(approval::configure)
            .configure(role::configure)
            .configure(team::configure)
            .configure(user::configure)
            .configure(auth::configure)
            .configure(jwt_secret::configure),
    );
}

pub fn swagger_ui() -> SwaggerUi {
    SwaggerUi::new("/docs/{_:.*}").url("/api/v1/openapi.json", ApiDoc::openapi())
}
