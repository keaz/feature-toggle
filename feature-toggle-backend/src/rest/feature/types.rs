use crate::database::entity::VariantValueType as DbVariantValueType;
use crate::logic::feature::StageChangeRequestType;
use crate::model::{
    FeatureType as ModelFeatureType, LifecycleStage as ModelLifecycleStage,
    VariantValueType as ModelVariantValueType,
};
use crate::rest::environment::EnvironmentResponse;
use crate::rest::pagination::PageMeta;
use crate::rest::pipeline::CreateRelationshipRequest;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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
    Draft,
    Active,
    Deprecated,
    Archived,
}

impl From<ModelLifecycleStage> for LifecycleStage {
    fn from(value: ModelLifecycleStage) -> Self {
        match value {
            ModelLifecycleStage::Draft => LifecycleStage::Draft,
            ModelLifecycleStage::Active => LifecycleStage::Active,
            ModelLifecycleStage::Deprecated => LifecycleStage::Deprecated,
            ModelLifecycleStage::Archived => LifecycleStage::Archived,
        }
    }
}

impl From<LifecycleStage> for ModelLifecycleStage {
    fn from(value: LifecycleStage) -> Self {
        match value {
            LifecycleStage::Draft => ModelLifecycleStage::Draft,
            LifecycleStage::Active => ModelLifecycleStage::Active,
            LifecycleStage::Deprecated => ModelLifecycleStage::Deprecated,
            LifecycleStage::Archived => ModelLifecycleStage::Archived,
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
    pub lifecycle_stage: Option<LifecycleStage>,
    pub stale: Option<bool>,
    pub include_archived: Option<bool>,
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
    pub created_at: DateTime<Utc>,
    pub kill_switch_enabled: bool,
    pub kill_switch_activated_at: Option<DateTime<Utc>>,
    pub rollback_scheduled_at: Option<DateTime<Utc>>,
    pub lifecycle_stage: LifecycleStage,
    pub owner: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
    pub archived_at: Option<DateTime<Utc>>,
    pub deprecated_at: Option<DateTime<Utc>>,
    pub deprecation_notice: Option<String>,
    pub last_evaluated_at: Option<DateTime<Utc>>,
    pub evaluation_count_7d: i64,
    pub evaluation_count_30d: i64,
    pub evaluation_count_90d: i64,
    pub is_stale: bool,
    pub stale_reasons: Vec<String>,
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
    pub lifecycle_stage: Option<LifecycleStage>,
    pub owner: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
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
    pub lifecycle_stage: Option<LifecycleStage>,
    pub owner: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
    pub archive_confirmation: Option<bool>,
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
    pub fn as_str(&self) -> &'static str {
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
