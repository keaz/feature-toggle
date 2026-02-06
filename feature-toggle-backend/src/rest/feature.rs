use actix_web::{get, patch, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::model::ID;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::approval::{
    approval_repository_tx, ApprovalRepository, ApprovalRepositoryTx, CreateApprovalRequestInput,
};
use crate::database::entity::{
    ApprovalPolicy, DBStage, FeaturePipelineStage, VariantValueType as DbVariantValueType,
};
use crate::database::activity_log::ActivityLogRepository;
use crate::database::feature::{feature_repository_tx, FeatureRepository};
use crate::database::feature::FeatureRepositoryTx;
use crate::broadcast::map_db_feature_to_full_for_broadcast;
use crate::model::{
    CreateFeatureInput, CreateFeatureStageInput, CreateFeatureVariantInput,
    CreateRelationshipInput, Feature as ModelFeature, FeatureType as ModelFeatureType,
    LifecycleStage as ModelLifecycleStage, UpdateFeatureInput,
    VariantValueType as ModelVariantValueType,
};
use crate::validation::{
    validate_duplicate_environment_and_index, validate_relationships_and_stages,
    validate_stage_transition,
};
use crate::logic::approval::{policy_applies, status_requires_interception};
use crate::logic::authorization::RoleAuthorizer;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::{FeatureLogic, StageChangeRequestType};
use crate::logic::feature_tx;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::{create_relationships, get_environment_map, ActorContext};
use crate::rest::environment::EnvironmentResponse;
use crate::rest::error::RestError;
use crate::rest::pagination::{normalize_pagination, PageMeta, PaginationQuery};
use crate::rest::pipeline::CreateRelationshipRequest;
use crate::JwtUser;
use feature_toggle_shared::constants::StageStatus;

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeatureType {
    Simple,
    Contextual,
}

impl From<ModelFeatureType> for FeatureType {
    fn from(value: ModelFeatureType) -> Self {
        match value {
            ModelFeatureType::Simple => FeatureType::Simple,
            ModelFeatureType::Contextual => FeatureType::Contextual,
        }
    }
}

impl From<FeatureType> for ModelFeatureType {
    fn from(value: FeatureType) -> Self {
        match value {
            FeatureType::Simple => ModelFeatureType::Simple,
            FeatureType::Contextual => ModelFeatureType::Contextual,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LifecycleStage {
    Active,
    Deprecated,
    Archived,
    Permanent,
}

impl From<ModelLifecycleStage> for LifecycleStage {
    fn from(value: ModelLifecycleStage) -> Self {
        match value {
            ModelLifecycleStage::Active => LifecycleStage::Active,
            ModelLifecycleStage::Deprecated => LifecycleStage::Deprecated,
            ModelLifecycleStage::Archived => LifecycleStage::Archived,
            ModelLifecycleStage::Permanent => LifecycleStage::Permanent,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VariantValueType {
    String,
    Number,
    Boolean,
    Json,
}

impl From<ModelVariantValueType> for VariantValueType {
    fn from(value: ModelVariantValueType) -> Self {
        match value {
            ModelVariantValueType::String => VariantValueType::String,
            ModelVariantValueType::Number => VariantValueType::Number,
            ModelVariantValueType::Boolean => VariantValueType::Boolean,
            ModelVariantValueType::Json => VariantValueType::Json,
        }
    }
}

impl From<VariantValueType> for ModelVariantValueType {
    fn from(value: VariantValueType) -> Self {
        match value {
            VariantValueType::String => ModelVariantValueType::String,
            VariantValueType::Number => ModelVariantValueType::Number,
            VariantValueType::Boolean => ModelVariantValueType::Boolean,
            VariantValueType::Json => ModelVariantValueType::Json,
        }
    }
}

impl From<DbVariantValueType> for VariantValueType {
    fn from(value: DbVariantValueType) -> Self {
        match value {
            DbVariantValueType::String => VariantValueType::String,
            DbVariantValueType::Number => VariantValueType::Number,
            DbVariantValueType::Boolean => VariantValueType::Boolean,
            DbVariantValueType::Json => VariantValueType::Json,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureListQuery {
    pub name: Option<String>,
    pub feature_type: Option<FeatureType>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureRolloutQuery {
    pub team_id: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutMetricsQuery {
    pub team_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureRelationshipResponse {
    pub source_id: i32,
    pub target_id: i32,
}

impl crate::model::Relationship for FeatureRelationshipResponse {}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureStageResponse {
    pub id: String,
    pub environment: EnvironmentResponse,
    pub order_index: i32,
    pub position: String,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureVariantResponse {
    pub id: String,
    pub feature_id: String,
    pub control: String,
    pub value: serde_json::Value,
    pub value_type: VariantValueType,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureResponse {
    pub id: String,
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: bool,
    pub kill_switch_enabled: bool,
    pub kill_switch_activated_at: Option<DateTime<Utc>>,
    pub rollback_scheduled_at: Option<DateTime<Utc>>,
    pub lifecycle_stage: LifecycleStage,
    pub deprecated_at: Option<DateTime<Utc>>,
    pub deprecation_notice: Option<String>,
    pub last_evaluated_at: Option<DateTime<Utc>>,
    pub evaluation_count_7d: i64,
    pub evaluation_count_30d: i64,
    pub evaluation_count_90d: i64,
    pub dependencies: Vec<String>,
    pub team_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_approval_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relationships: Option<Vec<FeatureRelationshipResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<FeatureStageResponse>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<FeatureVariantResponse>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeaturesResponse {
    pub items: Vec<FeatureResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateFeatureStageRequest {
    pub id: Option<String>,
    pub environment_id: String,
    pub order_index: i32,
    pub position: String,
    pub bucketing_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateFeatureVariantRequest {
    pub control: String,
    pub value: serde_json::Value,
    pub value_type: VariantValueType,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateFeatureRequest {
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub dependencies: Vec<String>,
    pub relationships: Vec<CreateRelationshipRequest>,
    pub stages: Vec<CreateFeatureStageRequest>,
    pub variants: Option<Vec<CreateFeatureVariantRequest>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFeatureRequest {
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub dependencies: Vec<String>,
    pub relationships: Vec<CreateRelationshipRequest>,
    pub stages: Vec<CreateFeatureStageRequest>,
    pub variants: Option<Vec<CreateFeatureVariantRequest>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyDisableRequest {
    pub rollback_in_minutes: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StageChangeRequest {
    DeploymentRequested,
    DeploymentRejected,
    Deployed,
    RollbackRequested,
    RollbackRejected,
    Rollbacked,
}

impl StageChangeRequest {
    fn as_str(&self) -> &'static str {
        match self {
            StageChangeRequest::DeploymentRequested => "DEPLOYMENT_REQUESTED",
            StageChangeRequest::DeploymentRejected => "DEPLOYMENT_REJECTED",
            StageChangeRequest::Deployed => "DEPLOYED",
            StageChangeRequest::RollbackRequested => "ROLLBACK_REQUESTED",
            StageChangeRequest::RollbackRejected => "ROLLBACK_REJECTED",
            StageChangeRequest::Rollbacked => "ROLLBACKED",
        }
    }
}

impl From<StageChangeRequest> for StageChangeRequestType {
    fn from(value: StageChangeRequest) -> Self {
        match value {
            StageChangeRequest::DeploymentRequested => StageChangeRequestType::DeploymentRequested,
            StageChangeRequest::DeploymentRejected => StageChangeRequestType::DeploymentRejected,
            StageChangeRequest::Deployed => StageChangeRequestType::Deployed,
            StageChangeRequest::RollbackRequested => StageChangeRequestType::RollbackRequested,
            StageChangeRequest::RollbackRejected => StageChangeRequestType::RollbackRejected,
            StageChangeRequest::Rollbacked => StageChangeRequestType::Rollbacked,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StageChangeRequestBody {
    pub request: StageChangeRequest,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutMetricsResponse {
    pub average_time_in_pipeline: f64,
    pub approval_rate: f64,
    pub features_deployed_this_week: i32,
    pub features_deployed_last_week: i32,
    pub deployment_change: f64,
    pub bottleneck_stage: String,
    pub bottleneck_duration: f64,
    pub total_pending_approvals: i32,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

async fn find_applicable_policy(
    approval_repo: &dyn ApprovalRepository,
    env_logic: &dyn EnvironmentLogic,
    team_id: Uuid,
    environment_id: Uuid,
) -> Result<Option<ApprovalPolicy>, RestError> {
    let env = env_logic
        .get_environment_by_id(ID::from(environment_id))
        .await
        .map_err(RestError::from)?;

    let policies = approval_repo
        .list_policies_for_team(team_id)
        .await
        .map_err(RestError::from)?;

    let mut applicable: Vec<ApprovalPolicy> = policies
        .into_iter()
        .filter(|policy| {
            policy_applies(policy, environment_id, env.environment_type.as_str())
        })
        .collect();

    if applicable.is_empty() {
        return Ok(None);
    }

    if let Some(manual_policy) = applicable
        .iter()
        .find(|policy| policy.auto_approve_after_hours.is_none())
        .cloned()
    {
        return Ok(Some(manual_policy));
    }

    Ok(applicable.pop())
}

fn validate_feature_key_create(key: &str) -> Result<(), RestError> {
    let trimmed = key.trim();
    if trimmed.len() < 3 || trimmed.len() > 40 {
        return Err(RestError::invalid_input(
            "Feature key must be between 3 and 40 characters",
        ));
    }
    Ok(())
}

fn validate_feature_key_update(key: &str) -> Result<(), RestError> {
    let trimmed = key.trim();
    if trimmed.len() < 3 || trimmed.len() > 100 {
        return Err(RestError::invalid_input(
            "Feature key must be between 3 and 100 characters",
        ));
    }
    Ok(())
}

fn validate_description_create(description: &Option<String>) -> Result<(), RestError> {
    if let Some(desc) = description.as_deref() {
        let trimmed = desc.trim();
        if trimmed.len() < 3 || trimmed.len() > 255 {
            return Err(RestError::invalid_input(
                "Feature description must be between 3 and 255 characters",
            ));
        }
    }
    Ok(())
}

fn validate_variant_requests(variants: &Option<Vec<CreateFeatureVariantRequest>>) -> Result<(), RestError> {
    if let Some(list) = variants {
        for variant in list {
            let control_len = variant.control.trim().len();
            if control_len < 1 || control_len > 100 {
                return Err(RestError::invalid_input(
                    "Variant control must be between 1 and 100 characters",
                ));
            }
            if let Some(desc) = variant.description.as_deref() {
                if desc.trim().len() > 500 {
                    return Err(RestError::invalid_input(
                        "Variant description must be at most 500 characters",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn map_stage_requests(
    stages: &[CreateFeatureStageRequest],
) -> Result<Vec<CreateFeatureStageInput>, RestError> {
    if stages.is_empty() {
        return Err(RestError::invalid_input(
            "Pipeline must have at least one stage",
        ));
    }

    let mut mapped = Vec::with_capacity(stages.len());
    for stage in stages {
        if stage.order_index < 0 {
            return Err(RestError::invalid_input(
                "Stage order_index must be greater than or equal to 0",
            ));
        }
        let position_len = stage.position.trim().len();
        if position_len < 1 || position_len > 50 {
            return Err(RestError::invalid_input(
                "Stage position must be between 1 and 50 characters",
            ));
        }
        let env_uuid = parse_uuid(&stage.environment_id, "environment_id")?;
        let stage_id = match stage.id.as_deref() {
            Some(value) => Some(ID::from(parse_uuid(value, "stage id")?)),
            None => None,
        };
        mapped.push(CreateFeatureStageInput {
            id: stage_id,
            environment_id: ID::from(env_uuid),
            order_index: stage.order_index,
            position: stage.position.clone(),
            bucketing_key: stage.bucketing_key.clone(),
        });
    }

    Ok(mapped)
}

fn map_relationship_requests(
    relationships: &[CreateRelationshipRequest],
) -> Result<Vec<CreateRelationshipInput>, RestError> {
    let mut mapped = Vec::with_capacity(relationships.len());
    for rel in relationships {
        if rel.source_id < 0 {
            return Err(RestError::invalid_input(
                "Relationship source_id must be greater than or equal to 0",
            ));
        }
        if rel.target_id < 1 {
            return Err(RestError::invalid_input(
                "Relationship target_id must be greater than or equal to 1",
            ));
        }
        mapped.push(CreateRelationshipInput {
            source_id: rel.source_id,
            target_id: rel.target_id,
        });
    }
    Ok(mapped)
}

fn validate_feature_structure(
    stages: &[CreateFeatureStageInput],
    relationships: &[CreateRelationshipInput],
) -> Result<(), RestError> {
    validate_relationships_and_stages(stages, relationships)
        .map_err(RestError::invalid_input)?;
    validate_duplicate_environment_and_index(stages)
        .map_err(RestError::invalid_input)?;
    Ok(())
}

fn map_dependencies(ids: &[String]) -> Result<Vec<ID>, RestError> {
    ids.iter()
        .map(|id| Ok(ID::from(parse_uuid(id, "dependency id")?)))
        .collect()
}

async fn ensure_feature_key_unique_for_create(
    logic: &dyn PipelineLogic,
    team_id: ID,
    key: &str,
) -> Result<(), RestError> {
    let pipelines = logic
        .get_pipelines(team_id, Some(key.to_string()), Some(true), vec![])
        .await
        .map_err(RestError::from)?;

    if !pipelines.is_empty() {
        return Err(RestError::conflict(format!(
            "Feature with name '{}' already exists",
            key
        )));
    }

    Ok(())
}

async fn ensure_feature_key_unique_for_update(
    logic: &dyn FeatureLogic,
    feature_id: &ID,
    key: &str,
) -> Result<ModelFeature, RestError> {
    let feature = logic
        .get_feature_by_id(feature_id.clone())
        .await
        .map_err(RestError::from)?;

    let existing = logic
        .get_features(feature.team_id.clone(), Some(key.to_string()), None)
        .await
        .map_err(RestError::from)?;

    let has_conflict = existing
        .iter()
        .any(|item| item.id != *feature_id);

    if has_conflict {
        return Err(RestError::conflict(format!(
            "Feature with name '{}' already exists",
            key
        )));
    }

    Ok(feature)
}

fn feature_base_response(feature: &ModelFeature) -> FeatureResponse {
    FeatureResponse {
        id: feature.id.to_string(),
        key: feature.key.clone(),
        description: feature.description.clone(),
        feature_type: FeatureType::from(feature.feature_type),
        enabled: feature.enabled,
        kill_switch_enabled: feature.kill_switch_enabled,
        kill_switch_activated_at: feature.kill_switch_activated_at,
        rollback_scheduled_at: feature.rollback_scheduled_at,
        lifecycle_stage: LifecycleStage::from(feature.lifecycle_stage),
        deprecated_at: feature.deprecated_at,
        deprecation_notice: feature.deprecation_notice.clone(),
        last_evaluated_at: feature.last_evaluated_at,
        evaluation_count_7d: feature.evaluation_count_7d,
        evaluation_count_30d: feature.evaluation_count_30d,
        evaluation_count_90d: feature.evaluation_count_90d,
        dependencies: feature
            .dependencies
            .iter()
            .map(|id| id.to_string())
            .collect(),
        team_id: feature.team_id.to_string(),
        pending_approval_request_id: feature
            .pending_approval_request_id
            .as_ref()
            .map(|id| id.to_string()),
        relationships: None,
        stages: None,
        variants: None,
    }
}

fn stage_boxes(stages: &[FeaturePipelineStage]) -> Vec<Box<dyn DBStage>> {
    stages
        .iter()
        .cloned()
        .map(|stage| Box::new(stage) as Box<dyn DBStage>)
        .collect()
}

async fn load_stage_data(
    feature_repo: &dyn FeatureRepository,
    env_logic: &dyn EnvironmentLogic,
    feature_id: Uuid,
) -> Result<(Vec<FeatureStageResponse>, Vec<FeatureRelationshipResponse>), RestError> {
    let stages = feature_repo
        .get_feature_stages(feature_id)
        .await
        .map_err(RestError::from)?;

    let stages_for_env = stage_boxes(&stages);
    let stages_for_rels = stage_boxes(&stages);

    let environment_map = get_environment_map(env_logic, &stages_for_env, true)
        .await
        .map_err(RestError::from)?;

    let mut mapped_stages = Vec::with_capacity(stages.len());
    for stage in stages.iter() {
        let env = environment_map
            .get(&stage.environment_id)
            .ok_or_else(|| RestError::internal("Environment not found"))?;
        mapped_stages.push(FeatureStageResponse {
            id: stage.id.to_string(),
            environment: EnvironmentResponse::from(env.clone()),
            order_index: stage.order_index,
            position: stage.position.clone(),
            status: stage.status.clone(),
        });
    }
    mapped_stages.sort_by(|a, b| a.order_index.cmp(&b.order_index));

    let relationships = create_relationships(true, stages_for_rels, |source_id, target_id| {
        FeatureRelationshipResponse {
            source_id,
            target_id,
        }
    });

    Ok((mapped_stages, relationships))
}

async fn load_variants(
    feature_repo: &dyn FeatureRepository,
    feature_id: Uuid,
) -> Result<Vec<FeatureVariantResponse>, RestError> {
    let variants = feature_repo
        .get_feature_variants(feature_id)
        .await
        .map_err(RestError::from)?;

    Ok(variants
        .into_iter()
        .map(|variant| FeatureVariantResponse {
            id: variant.id.to_string(),
            feature_id: variant.feature_id.to_string(),
            control: variant.control,
            value: variant.value,
            value_type: VariantValueType::from(variant.value_type),
            description: variant.description,
            created_at: variant.created_at,
            updated_at: variant.updated_at,
        })
        .collect())
}

async fn build_feature_response(
    feature: &ModelFeature,
    feature_repo: &dyn FeatureRepository,
    env_logic: &dyn EnvironmentLogic,
    include_variants: bool,
    include_relationships: bool,
    include_stages: bool,
) -> Result<FeatureResponse, RestError> {
    let mut response = feature_base_response(feature);
    let feature_id = parse_uuid(&feature.id.to_string(), "feature id")?;

    if include_stages || include_relationships {
        let (stages, relationships) = load_stage_data(feature_repo, env_logic, feature_id).await?;
        if include_stages {
            response.stages = Some(stages);
        }
        if include_relationships {
            response.relationships = Some(relationships);
        }
    }

    if include_variants {
        response.variants = Some(load_variants(feature_repo, feature_id).await?);
    }

    Ok(response)
}

async fn broadcast_feature_update(
    feature_repo: &dyn FeatureRepository,
    updates_tx: &tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    feature_id: Uuid,
) {
    if let Ok(db_feature) = feature_repo.get_feature_by_id(feature_id).await {
        if let Ok(full) = map_db_feature_to_full_for_broadcast(feature_repo, db_feature).await {
            let _ = updates_tx.send(crate::grpc::pb::FeatureUpdate {
                message_id: uuid::Uuid::new_v4().to_string(),
                action: crate::grpc::pb::feature_update::Action::Upsert as i32,
                feature: Some(full),
                feature_key: String::new(),
                error: String::new(),
            });
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/features",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("name" = Option<String>, Query, description = "Filter by feature name"),
        ("featureType" = Option<FeatureType>, Query, description = "Filter by feature type"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Feature list", body = FeaturesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/teams/{team_id}/features")]
pub(crate) async fn list_features(
    logic: web::Data<Box<dyn FeatureLogic>>,
    team_id: web::Path<String>,
    query: web::Query<FeatureListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let (features, total) = logic
        .get_features_with_offset(
            ID::from(team_uuid),
            query.name.clone(),
            query.feature_type.map(ModelFeatureType::from),
            offset,
            limit,
        )
        .await
        .map_err(RestError::from)?;

    let items = features
        .iter()
        .map(feature_base_response)
        .collect::<Vec<_>>();

    Ok(HttpResponse::Ok().json(FeaturesResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/{id}",
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature detail", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/{id}")]
pub(crate) async fn get_feature(
    logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    feature_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let feature = logic
        .get_feature_by_id(ID::from(feature_uuid))
        .await
        .map_err(RestError::from)?;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/features",
    request_body = CreateFeatureRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Feature created", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/teams/{team_id}/features")]
pub(crate) async fn create_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    pipeline_logic: web::Data<Box<dyn PipelineLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    team_id: web::Path<String>,
    payload: web::Json<CreateFeatureRequest>,
) -> Result<impl Responder, RestError> {
    validate_feature_key_create(&payload.key)?;
    validate_description_create(&payload.description)?;
    validate_variant_requests(&payload.variants)?;

    let team_uuid = parse_uuid(&team_id, "team_id")?;
    ensure_feature_key_unique_for_create(
        pipeline_logic.as_ref().as_ref(),
        ID::from(team_uuid),
        payload.key.as_str(),
    )
    .await?;

    let stages = map_stage_requests(&payload.stages)?;
    let relationships = map_relationship_requests(&payload.relationships)?;
    validate_feature_structure(&stages, &relationships)?;

    let dependencies = map_dependencies(&payload.dependencies)?;
    let variants = payload
        .variants
        .as_ref()
        .map(|list| {
            list.iter()
                .cloned()
                .map(|variant| CreateFeatureVariantInput {
                    control: variant.control,
                    value: variant.value,
                    value_type: ModelVariantValueType::from(variant.value_type),
                    description: variant.description,
                })
                .collect::<Vec<_>>()
        });

    if payload.feature_type == FeatureType::Simple {
        if let Some(ref list) = variants {
            if !list.is_empty() {
                return Err(RestError::invalid_input(
                    "Variants can only be defined for Contextual features, not Simple features",
                ));
            }
        }
    }

    let input = CreateFeatureInput {
        key: payload.key.clone(),
        description: payload.description.clone(),
        feature_type: ModelFeatureType::from(payload.feature_type),
        enabled: payload.enabled,
        dependencies,
        relationships,
        stages,
        variants,
    };

    let actor = actor_from_request(&req);
    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::feature_tx::create_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(team_uuid),
        input,
        actor,
    )
    .await;

    let feature_id = match result {
        Ok(feature_id) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            feature_id
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    };

    let feature = feature_logic
        .get_feature_by_id(feature_id)
        .await
        .map_err(RestError::from)?;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Created().json(response))
}

#[utoipa::path(
    patch,
    path = "/api/v1/features/{id}",
    request_body = UpdateFeatureRequest,
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature updated", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[patch("/features/{id}")]
pub(crate) async fn update_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    feature_id: web::Path<String>,
    payload: web::Json<UpdateFeatureRequest>,
) -> Result<impl Responder, RestError> {
    validate_feature_key_update(&payload.key)?;
    validate_variant_requests(&payload.variants)?;

    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let existing_feature =
        ensure_feature_key_unique_for_update(feature_logic.as_ref().as_ref(), &ID::from(feature_uuid), &payload.key)
            .await?;

    let stages = map_stage_requests(&payload.stages)?;
    let relationships = map_relationship_requests(&payload.relationships)?;
    validate_feature_structure(&stages, &relationships)?;

    let dependencies = map_dependencies(&payload.dependencies)?;
    let variants = payload
        .variants
        .as_ref()
        .map(|list| {
            list.iter()
                .cloned()
                .map(|variant| CreateFeatureVariantInput {
                    control: variant.control,
                    value: variant.value,
                    value_type: ModelVariantValueType::from(variant.value_type),
                    description: variant.description,
                })
                .collect::<Vec<_>>()
        });

    if payload.feature_type == FeatureType::Simple {
        if let Some(ref list) = variants {
            if !list.is_empty() {
                return Err(RestError::invalid_input(
                    "Variants can only be defined for Contextual features, not Simple features",
                ));
            }
        }
    }

    let input = UpdateFeatureInput {
        key: payload.key.clone(),
        description: payload.description.clone(),
        feature_type: ModelFeatureType::from(payload.feature_type),
        enabled: payload.enabled,
        dependencies,
        relationships,
        stages,
        variants,
    };

    let actor = actor_from_request(&req);
    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::feature_tx::update_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        existing_feature.id.clone(),
        input,
        actor,
    )
    .await;

    let updated = match result {
        Ok(feature) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            feature
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    };

    if let Ok(fid) = Uuid::try_from(existing_feature.id.clone()) {
        broadcast_feature_update(feature_repo.as_ref().as_ref(), updates_tx.get_ref(), fid).await;
    }

    let response = build_feature_response(
        &updated,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/emergency-disable",
    request_body = EmergencyDisableRequest,
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature emergency disabled", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/features/{id}/emergency-disable")]
pub(crate) async fn emergency_disable_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    feature_id: web::Path<String>,
    payload: web::Json<EmergencyDisableRequest>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let actor = actor_from_request(&req);

    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let result = feature_tx::emergency_disable_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(feature_uuid),
        payload.rollback_in_minutes,
        actor,
    )
    .await;

    let feature = match result {
        Ok(feature) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature
        }
        Err(e) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(e));
        }
    };

    broadcast_feature_update(feature_repo.as_ref().as_ref(), updates_tx.get_ref(), feature_uuid).await;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/emergency-enable",
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature emergency enabled", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/features/{id}/emergency-enable")]
pub(crate) async fn emergency_enable_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    feature_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let actor = actor_from_request(&req);

    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let result = feature_tx::emergency_enable_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(feature_uuid),
        actor,
    )
    .await;

    let feature = match result {
        Ok(feature) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature
        }
        Err(e) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(e));
        }
    };

    broadcast_feature_update(feature_repo.as_ref().as_ref(), updates_tx.get_ref(), feature_uuid).await;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/stages/{id}/request-change",
    request_body = StageChangeRequestBody,
    params(
        ("id" = String, Path, description = "Stage ID")
    ),
    responses(
        (status = 200, description = "Stage change requested", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/stages/{id}/request-change")]
pub(crate) async fn request_stage_change(
    db_pool: web::Data<sqlx::PgPool>,
    approval_repo: web::Data<Box<dyn ApprovalRepository>>,
    req: HttpRequest,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    stage_id: web::Path<String>,
    payload: web::Json<StageChangeRequestBody>,
) -> Result<impl Responder, RestError> {
    let stage_uuid = parse_uuid(&stage_id, "stage id")?;
    let jwt_user = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;

    RoleAuthorizer::authorize_stage_change_request(&jwt_user.roles, payload.request.as_str())
        .map_err(|e| RestError::forbidden(e.to_string()))?;

    let request_type = StageChangeRequestType::from(payload.request);
    let next_status = match request_type {
        StageChangeRequestType::DeploymentRequested => StageStatus::DeploymentRequested.as_str(),
        StageChangeRequestType::DeploymentRejected => StageStatus::DeploymentRejected.as_str(),
        StageChangeRequestType::Deployed => StageStatus::Deployed.as_str(),
        StageChangeRequestType::RollbackRequested => StageStatus::RollbackRequested.as_str(),
        StageChangeRequestType::RollbackRejected => StageStatus::RollbackRejected.as_str(),
        StageChangeRequestType::Rollbacked => StageStatus::Rollbacked.as_str(),
    };

    let stage = feature_repo
        .get_stage_by_id(stage_uuid)
        .await
        .map_err(RestError::from)?
        .ok_or_else(|| RestError::not_found("Stage not found"))?;

    let db_feature = feature_repo
        .get_feature_by_id(stage.feature_id)
        .await
        .map_err(RestError::from)?;

    let mut approval_request_id: Option<Uuid> = None;

    if status_requires_interception(next_status) {
        if let Some(policy) = find_applicable_policy(
            approval_repo.as_ref().as_ref(),
            env_logic.as_ref().as_ref(),
            db_feature.team_id,
            stage.environment_id,
        )
        .await?
        {
            let pending_status = match next_status {
                "DEPLOYED" | "DEPLOYMENT_REJECTED" => StageStatus::DeploymentRequested.as_str(),
                "ROLLBACKED" | "ROLLBACK_REJECTED" => StageStatus::RollbackRequested.as_str(),
                other => other,
            };

            validate_stage_transition(&stage.status, pending_status)
                .map_err(RestError::invalid_input)?;

            let approval_target_status = match next_status {
                "DEPLOYMENT_REQUESTED" => StageStatus::DeploymentApproved.as_str(),
                "ROLLBACK_REQUESTED" => StageStatus::RollbackApproved.as_str(),
                other => other,
            };
            let rejection_target_status = match next_status {
                "DEPLOYMENT_REQUESTED" => StageStatus::DeploymentRejected.as_str(),
                "ROLLBACK_REQUESTED" => StageStatus::RollbackRejected.as_str(),
                other => other,
            };
            let after_status = approval_target_status;

            let change_payload = serde_json::json!({
                "stage_id": stage.id.to_string(),
                "next_status": next_status,
                "approval_target_status": approval_target_status,
                "rejection_target_status": rejection_target_status,
                "previous_status": stage.status,
                "feature_id": db_feature.id.to_string(),
                "environment_id": stage.environment_id.to_string(),
                "before": { "status": stage.status },
                "after": { "status": after_status },
            });

            let mut tx = db_pool
                .begin()
                .await
                .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;
            let approval_repo_tx = approval_repository_tx(db_pool.get_ref().clone());
            let feature_repo_tx = feature_repository_tx(db_pool.get_ref().clone());

            let request = approval_repo_tx
                .create_request_tx(
                    &mut tx,
                    CreateApprovalRequestInput {
                        policy_id: policy.id,
                        feature_id: db_feature.id,
                        environment_id: Some(stage.environment_id),
                        change_type: "stage_change".into(),
                        change_payload,
                        change_description: Some(format!(
                            "Stage {} -> {} for feature {}",
                            stage.status, next_status, db_feature.key
                        )),
                        requested_by: jwt_user.id,
                    },
                )
                .await
                .map_err(RestError::from)?;

            if pending_status == StageStatus::DeploymentRequested.as_str()
                || pending_status == StageStatus::RollbackRequested.as_str()
            {
                let now = chrono::Utc::now();
                let updated = feature_repo_tx
                    .request_stage_change_tx(&mut tx, stage_uuid, pending_status, jwt_user.id, now)
                    .await
                    .map_err(RestError::from)?;
                if !updated {
                    let _ = tx.rollback().await;
                    return Err(RestError::not_found("Stage not found"));
                }
            }

            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;

            approval_request_id = Some(request.id);
        }
    }

    if approval_request_id.is_none() {
        validate_stage_transition(&stage.status, next_status)
            .map_err(RestError::invalid_input)?;

        let mut tx = db_pool
            .begin()
            .await
            .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;
        let feature_repo_tx = feature_repository_tx(db_pool.get_ref().clone());

        let updated = match request_type {
            StageChangeRequestType::DeploymentRequested | StageChangeRequestType::RollbackRequested => {
                let now = chrono::Utc::now();
                feature_repo_tx
                    .request_stage_change_tx(&mut tx, stage_uuid, next_status, jwt_user.id, now)
                    .await
                    .map_err(RestError::from)?
            }
            _ => feature_repo_tx
                .approve_or_reject_stage_change_tx(&mut tx, stage_uuid, next_status, jwt_user.id)
                .await
                .map_err(RestError::from)?,
        };

        if !updated {
            let _ = tx.rollback().await;
            return Err(RestError::not_found("Stage not found"));
        }

        tx.commit()
            .await
            .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
    }

    let mut feature = feature_logic
        .get_feature_by_id(ID::from(db_feature.id))
        .await
        .map_err(RestError::from)?;

    if let Some(request_id) = approval_request_id {
        feature.pending_approval_request_id = Some(ID::from(request_id));
    }

    if let Ok(fid) = Uuid::try_from(feature.id.clone()) {
        broadcast_feature_update(feature_repo.as_ref().as_ref(), updates_tx.get_ref(), fid).await;
    }

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/pending-approvals",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Pending approvals", body = FeaturesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/pending-approvals")]
pub(crate) async fn pending_approvals(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    query: web::Query<FeatureRolloutQuery>,
) -> Result<impl Responder, RestError> {
    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let (features, total) = feature_logic
        .get_features_with_pending_approvals_with_offset(team_id, offset, limit)
        .await
        .map_err(RestError::from)?;

    let mut items = Vec::with_capacity(features.len());
    for feature in features.iter() {
        let mut response = feature_base_response(feature);
        response.stages = Some(
            load_stage_data(
                feature_repo.as_ref().as_ref(),
                env_logic.as_ref().as_ref(),
                parse_uuid(&feature.id.to_string(), "feature id")?,
            )
            .await?
            .0,
        );
        items.push(response);
    }

    Ok(HttpResponse::Ok().json(FeaturesResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/active-kill-switches",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Active kill switches", body = FeaturesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/active-kill-switches")]
pub(crate) async fn active_kill_switches(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    query: web::Query<FeatureRolloutQuery>,
) -> Result<impl Responder, RestError> {
    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let (features, total) = feature_logic
        .get_features_with_kill_switches_with_offset(team_id, offset, limit)
        .await
        .map_err(RestError::from)?;

    let mut items = Vec::with_capacity(features.len());
    for feature in features.iter() {
        let mut response = feature_base_response(feature);
        response.stages = Some(
            load_stage_data(
                feature_repo.as_ref().as_ref(),
                env_logic.as_ref().as_ref(),
                parse_uuid(&feature.id.to_string(), "feature id")?,
            )
            .await?
            .0,
        );
        items.push(response);
    }

    Ok(HttpResponse::Ok().json(FeaturesResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/rollout-metrics",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team")
    ),
    responses(
        (status = 200, description = "Rollout metrics", body = RolloutMetricsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/rollout-metrics")]
pub(crate) async fn rollout_metrics(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    query: web::Query<RolloutMetricsQuery>,
) -> Result<impl Responder, RestError> {
    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };

    let metrics = feature_logic
        .get_rollout_metrics(team_id)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(RolloutMetricsResponse {
        average_time_in_pipeline: metrics.average_time_in_pipeline,
        approval_rate: metrics.approval_rate,
        features_deployed_this_week: metrics.features_deployed_this_week,
        features_deployed_last_week: metrics.features_deployed_last_week,
        deployment_change: metrics.deployment_change,
        bottleneck_stage: metrics.bottleneck_stage,
        bottleneck_duration: metrics.bottleneck_duration,
        total_pending_approvals: metrics.total_pending_approvals,
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_features)
        // Register static paths before /features/{feature_id} to avoid path conflicts.
        .service(rollout_metrics)
        .service(pending_approvals)
        .service(active_kill_switches)
        .service(get_feature)
        .service(create_feature)
        .service(update_feature)
        .service(emergency_disable_feature)
        .service(emergency_enable_feature)
        .service(request_stage_change);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App, http::StatusCode};
    use crate::database::activity_log::PgActivityLogRepository;
    use crate::database::environment::environment_repository;
    use crate::database::feature::{feature_repository, MockFeatureRepository};
    use crate::database::user::user_repository;
    use crate::model::{
        Feature as ModelFeature, FeatureType as ModelFeatureType,
        LifecycleStage as ModelLifecycleStage,
    };
    use crate::logic::environment::{environment_logic, MockEnvironmentLogic};
    use crate::logic::feature::{feature_logic, MockFeatureLogic};
    use crate::logic::pipeline::MockPipelineLogic;
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    fn sample_feature(feature_id: Uuid, team_id: Uuid) -> ModelFeature {
        ModelFeature {
            id: ID::from(feature_id),
            key: "checkout".to_string(),
            description: Some("Test feature".to_string()),
            feature_type: ModelFeatureType::Simple,
            enabled: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            lifecycle_stage: ModelLifecycleStage::Active,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
            team_id: ID::from(team_id),
            pending_approval_request_id: None,
        }
    }

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    async fn insert_team(pool: &sqlx::PgPool) -> Uuid {
        let team_id = Uuid::new_v4();
        let name = format!("feature-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "feature test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_environment(pool: &sqlx::PgPool, team_id: Uuid) -> Uuid {
        let env_id = Uuid::new_v4();
        let name = format!("env-{}", env_id);
        sqlx::query!(
            r#"INSERT INTO environments (id, name, active, team_id, environment_type)
               VALUES ($1, $2, $3, $4, $5)"#,
            env_id,
            name,
            true,
            team_id,
            "Production"
        )
        .execute(pool)
        .await
        .expect("Failed to insert environment");
        env_id
    }

    #[actix_web::test]
    async fn list_features_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let feature = sample_feature(feature_id, team_id);

        let mut mock_logic = MockFeatureLogic::new();
        mock_logic
            .expect_get_features_with_offset()
            .withf(move |id, name, feature_type, offset, limit| {
                id.to_string() == team_id.to_string()
                    && name.as_deref() == Some("check")
                    && matches!(feature_type, Some(ModelFeatureType::Simple))
                    && *offset == 10
                    && *limit == 5
            })
            .times(1)
            .returning(move |_, _, _, _, _| Ok((vec![feature.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn FeatureLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/features?offset=10&limit=5&name=check&featureType=SIMPLE");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], feature_id.to_string());
        assert_eq!(json["meta"]["offset"], 10);
        assert_eq!(json["meta"]["limit"], 5);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn create_feature_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id).await;

        let mut mock_pipeline_logic = MockPipelineLogic::new();
        mock_pipeline_logic
            .expect_get_pipelines()
            .returning(|_, _, _, _| Ok(vec![]));

        let activity_repo = PgActivityLogRepository::new(pool.clone());
        let env_logic_for_handler = environment_logic(
            environment_repository(pool.clone()),
            Box::new(PgActivityLogRepository::new(pool.clone())),
        );
        let env_logic_for_feature = environment_logic(
            environment_repository(pool.clone()),
            Box::new(PgActivityLogRepository::new(pool.clone())),
        );
        let feature_logic = feature_logic(
            feature_repository(pool.clone()),
            env_logic_for_feature,
            Box::new(PgActivityLogRepository::new(pool.clone())),
            user_repository(pool.clone()),
        );
        let feature_repo = feature_repository(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(feature_logic))
                .app_data(web::Data::new(
                    Box::new(mock_pipeline_logic) as Box<dyn PipelineLogic>
                ))
                .app_data(web::Data::new(feature_repo))
                .app_data(web::Data::new(env_logic_for_handler))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/features");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateFeatureRequest {
                key: "checkout".to_string(),
                description: Some("Test feature".to_string()),
                feature_type: FeatureType::Simple,
                enabled: Some(true),
                dependencies: vec![],
                relationships: vec![],
                stages: vec![CreateFeatureStageRequest {
                    id: None,
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                    bucketing_key: None,
                }],
                variants: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["key"], "checkout");
        assert_eq!(json["teamId"], team_id.to_string());
    }

    #[actix_web::test]
    async fn update_feature_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());
        let team_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let feature = sample_feature(feature_id, team_id);
        let mut duplicate = sample_feature(Uuid::new_v4(), team_id);
        duplicate.key = "checkout".to_string();

        let mut mock_feature_logic = MockFeatureLogic::new();
        mock_feature_logic
            .expect_get_feature_by_id()
            .withf(move |id| id.to_string() == feature_id.to_string())
            .times(1)
            .returning(move |_| Ok(feature.clone()));
        mock_feature_logic
            .expect_get_features()
            .withf(move |id, name, _| {
                id.to_string() == team_id.to_string() && name.as_deref() == Some("checkout")
            })
            .times(1)
            .returning(move |_, _, _| Ok(vec![duplicate.clone()]));

        let mock_feature_repo = MockFeatureRepository::new();
        let mock_env_logic = MockEnvironmentLogic::new();
        let (updates_tx, _updates_rx) =
            tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(1);

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_feature_logic) as Box<dyn FeatureLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_feature_repo) as Box<dyn FeatureRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_env_logic) as Box<dyn EnvironmentLogic>
                ))
                .app_data(web::Data::new(updates_tx))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/features/{feature_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateFeatureRequest {
                key: "checkout".to_string(),
                description: None,
                feature_type: FeatureType::Simple,
                enabled: Some(true),
                dependencies: vec![],
                relationships: vec![],
                stages: vec![CreateFeatureStageRequest {
                    id: None,
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                    bucketing_key: None,
                }],
                variants: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
    }
}
