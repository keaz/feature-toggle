use std::sync::Arc;

use async_graphql::{ComplexObject, Enum, ID, InputObject, Result as GqlResult, SimpleObject};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-export TimePeriod from subscription module
pub use crate::database::metrics::MetricType;
use crate::{
    database::{entity::DBStage, feature::FeatureRepository},
    graphql::subscription::TimePeriod,
    logic::{create_relationships, get_environment_map, map_stages},
};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum FeatureType {
    Simple,
    Contextual,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum VariantValueType {
    String,
    Number,
    Boolean,
    Json,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum RuleOperator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Contains,
    StartsWith,
    EndsWith,
    Regex,
    In,
    NotIn,
    SemverGreaterThan,
    SemverLessThan,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum LifecycleStage {
    Active,
    Deprecated,
    Archived,
    Permanent,
}

impl Default for LifecycleStage {
    fn default() -> Self {
        LifecycleStage::Active
    }
}

impl Default for RuleOperator {
    fn default() -> Self {
        RuleOperator::In
    }
}

impl RuleOperator {
    /// Convert to database format (SCREAMING_SNAKE_CASE)
    pub fn to_db_string(&self) -> String {
        match self {
            RuleOperator::Equals => "EQUALS".to_string(),
            RuleOperator::NotEquals => "NOT_EQUALS".to_string(),
            RuleOperator::GreaterThan => "GREATER_THAN".to_string(),
            RuleOperator::LessThan => "LESS_THAN".to_string(),
            RuleOperator::GreaterThanOrEqual => "GREATER_THAN_OR_EQUAL".to_string(),
            RuleOperator::LessThanOrEqual => "LESS_THAN_OR_EQUAL".to_string(),
            RuleOperator::Contains => "CONTAINS".to_string(),
            RuleOperator::StartsWith => "STARTS_WITH".to_string(),
            RuleOperator::EndsWith => "ENDS_WITH".to_string(),
            RuleOperator::Regex => "REGEX".to_string(),
            RuleOperator::In => "IN".to_string(),
            RuleOperator::NotIn => "NOT_IN".to_string(),
            RuleOperator::SemverGreaterThan => "SEMVER_GREATER_THAN".to_string(),
            RuleOperator::SemverLessThan => "SEMVER_LESS_THAN".to_string(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
#[graphql(complex)]
pub struct Feature {
    pub id: ID,
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: bool,
    pub kill_switch_enabled: bool,
    pub kill_switch_activated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub rollback_scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
    #[graphql(name = "lifecycleStage")]
    pub lifecycle_stage: LifecycleStage,
    #[graphql(name = "deprecatedAt")]
    pub deprecated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[graphql(name = "deprecationNotice")]
    pub deprecation_notice: Option<String>,
    #[graphql(name = "lastEvaluatedAt")]
    pub last_evaluated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[graphql(name = "evaluationCount7d")]
    pub evaluation_count_7d: i64,
    #[graphql(name = "evaluationCount30d")]
    pub evaluation_count_30d: i64,
    #[graphql(name = "evaluationCount90d")]
    pub evaluation_count_90d: i64,
    pub dependencies: Vec<ID>,
    pub team_id: ID,
    pub pending_approval_request_id: Option<ID>,
}

#[ComplexObject]
impl Feature {
    async fn variants(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<FeatureVariant>> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let feature_id = Uuid::try_from(self.id.clone())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let db_variants = crate::database::feature::get_feature_variants(pool, feature_id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(db_variants
            .into_iter()
            .map(|v| FeatureVariant {
                id: ID::from(v.id.to_string()),
                feature_id: ID::from(v.feature_id.to_string()),
                control: v.control,
                value: async_graphql::types::Json(v.value),
                value_type: match v.value_type {
                    crate::database::entity::VariantValueType::String => VariantValueType::String,
                    crate::database::entity::VariantValueType::Number => VariantValueType::Number,
                    crate::database::entity::VariantValueType::Boolean => VariantValueType::Boolean,
                    crate::database::entity::VariantValueType::Json => VariantValueType::Json,
                },
                description: v.description,
                created_at: v.created_at,
                updated_at: v.updated_at,
            })
            .collect())
    }

    async fn stages(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<FeatureStage>> {
        let repository = ctx.data::<Arc<Box<dyn FeatureRepository>>>().unwrap();

        let environment_logic = ctx
            .data::<Box<dyn crate::logic::environment::EnvironmentLogic>>()
            .unwrap();

        let db_stages = repository
            .get_feature_stages(Uuid::try_from(self.id.clone()).unwrap())
            .await
            .unwrap_or_default();

        // Build stage vectors: one for borrowing (environment map) and another for ownership (relationships)
        let db_stages_for_env: Vec<Box<dyn DBStage>> = db_stages
            .iter()
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect();

        let environment_map =
            get_environment_map(&**environment_logic, &db_stages_for_env, true).await?;

        let mut stages = map_stages(true, &environment_map, &db_stages_for_env, stage_factory);

        // Populate bucketing_key on stages from the database entity
        // Since there is no way to map the bucketing_key during the initial map_stages, we do it here
        use std::collections::HashMap;
        let bucketing_map: HashMap<String, Option<String>> = db_stages
            .iter()
            .map(|s| (s.id.to_string(), s.bucketing_key.clone()))
            .collect();
        for stage in stages.iter_mut() {
            if let Some(b) = bucketing_map.get(&stage.id.to_string()) {
                stage.bucketing_key = b.clone();
            }
        }
        // Populate status on stages from the database entity
        let status_map: std::collections::HashMap<String, String> = db_stages
            .iter()
            .map(|s| (s.id.to_string(), s.status.clone()))
            .collect();

        for stage in stages.iter_mut() {
            if let Some(st) = status_map.get(&stage.id.to_string()) {
                stage.status = st.clone();
            }
        }

        stages.sort_by(|a, b| a.order_index.cmp(&b.order_index));

        Ok(stages)
    }

    async fn relationships(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> GqlResult<Vec<FeatureRelationship>> {
        let repository = ctx.data::<Arc<Box<dyn FeatureRepository>>>().unwrap();

        let stages = repository
            .get_feature_stages(Uuid::try_from(self.id.clone()).unwrap())
            .await
            .unwrap_or_default();

        let db_stages_for_rels: Vec<Box<dyn DBStage>> = stages
            .iter()
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect();

        let relationships = create_relationships(true, db_stages_for_rels, relationship_factory);
        Ok(relationships)
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum ApprovalRequestStatus {
    #[graphql(name = "Pending")]
    Pending,
    #[graphql(name = "Approved")]
    Approved,
    #[graphql(name = "Rejected")]
    Rejected,
    #[graphql(name = "Cancelled")]
    Cancelled,
    #[graphql(name = "AutoApproved")]
    AutoApproved,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ID,
    pub policy_id: ID,
    pub feature_id: ID,
    pub environment_id: Option<ID>,
    pub change_type: String,
    pub change_payload: async_graphql::Json<serde_json::Value>,
    pub change_description: Option<String>,
    pub requested_by: ID,
    pub status: ApprovalRequestStatus,
    pub approved_count: i32,
    pub rejected_count: i32,
    pub executed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub fn map_approval_request(req: crate::database::entity::ApprovalRequest) -> ApprovalRequest {
    let status = match req.status {
        crate::database::entity::ApprovalStatus::Approved => ApprovalRequestStatus::Approved,
        crate::database::entity::ApprovalStatus::Rejected => ApprovalRequestStatus::Rejected,
        crate::database::entity::ApprovalStatus::Cancelled => ApprovalRequestStatus::Cancelled,
        crate::database::entity::ApprovalStatus::AutoApproved => {
            ApprovalRequestStatus::AutoApproved
        }
        crate::database::entity::ApprovalStatus::Pending => ApprovalRequestStatus::Pending,
    };

    ApprovalRequest {
        id: req.id.into(),
        policy_id: req.policy_id.into(),
        feature_id: req.feature_id.into(),
        environment_id: req.environment_id.map(ID::from),
        change_type: req.change_type,
        change_payload: async_graphql::Json(req.change_payload),
        change_description: req.change_description,
        requested_by: req.requested_by.into(),
        status,
        approved_count: req.approved_count,
        rejected_count: req.rejected_count,
        executed_at: req.executed_at,
        created_at: req.created_at,
        updated_at: req.updated_at,
    }
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRequestPage {
    pub items: Vec<ApprovalRequest>,
    pub total: i64,
    pub page_number: i32,
    pub page_size: i32,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum AppliesTo {
    #[graphql(name = "all")]
    All,
    #[graphql(name = "production_only")]
    ProductionOnly,
    #[graphql(name = "specific_environments")]
    SpecificEnvironments,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    pub id: ID,
    pub team_id: ID,
    pub name: String,
    pub description: Option<String>,
    pub applies_to: String,
    pub environment_ids: Option<Vec<ID>>,
    pub required_approvers: i32,
    pub approver_role_ids: Vec<ID>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub fn map_approval_policy(policy: crate::database::entity::ApprovalPolicy) -> ApprovalPolicy {
    ApprovalPolicy {
        id: policy.id.into(),
        team_id: policy.team_id.into(),
        name: policy.name,
        description: policy.description,
        applies_to: policy.applies_to,
        environment_ids: policy
            .environment_ids
            .map(|ids| ids.into_iter().map(ID::from).collect()),
        required_approvers: policy.required_approvers,
        approver_role_ids: policy.approver_role_ids.into_iter().map(ID::from).collect(),
        auto_approve_after_hours: policy.auto_approve_after_hours,
        enabled: policy.enabled,
        created_at: policy.created_at,
    }
}

#[derive(InputObject, Clone, Debug, Serialize, Deserialize)]
pub struct CreateApprovalPolicyInput {
    pub name: String,
    pub description: Option<String>,
    pub applies_to: String,
    pub environment_ids: Option<Vec<ID>>,
    pub required_approvers: i32,
    pub approver_role_ids: Vec<ID>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(InputObject, Clone, Debug, Serialize, Deserialize)]
pub struct UpdateApprovalPolicyInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub applies_to: Option<String>,
    pub environment_ids: Option<Vec<ID>>,
    pub required_approvers: Option<i32>,
    pub approver_role_ids: Option<Vec<ID>>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
}

fn relationship_factory(source_id: i32, target_id: i32) -> FeatureRelationship {
    FeatureRelationship {
        source_id,
        target_id,
    }
}

fn stage_factory(
    id: ID,
    environment: Environment,
    order_index: i32,
    position: String,
) -> FeatureStage {
    FeatureStage {
        id,
        environment,
        order_index,
        position,
        bucketing_key: None,
        status: "NOT_DEPLOYED".to_string(),
    }
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize, Copy)]
pub struct FeatureRelationship {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct FeatureStage {
    pub id: ID,
    pub environment: Environment,
    pub order_index: i32,
    pub position: String,
    pub bucketing_key: Option<String>,
    pub status: String,
}

// Feature variant GraphQL type
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct FeatureVariant {
    pub id: ID,
    pub feature_id: ID,
    pub control: String,
    pub value: async_graphql::types::Json<serde_json::Value>,
    pub value_type: VariantValueType,
    pub description: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ContextualType {
    pub id: ID,
    pub key: String,
    pub entries: Vec<ContextualEntry>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ContextualEntry {
    pub id: ID,
    pub value: String,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub id: ID,
    pub name: String,
    pub team_id: ID,
    pub active: bool,
    pub environment_type: String,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: ID,
    pub name: String,
    pub active: bool,
    pub team_id: ID,
    pub stages: Vec<PipelineStage>,
    pub relationships: Vec<PipelineRelationship>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize, Copy)]
pub struct PipelineRelationship {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct PipelineStage {
    pub id: ID,
    pub environment: Environment,
    pub order_index: i32,
    pub position: String,
}

// Input types for mutations
#[derive(InputObject, Debug, Clone)]
pub struct CreateFeatureVariantInput {
    #[graphql(validator(min_length = 1, max_length = 100))]
    pub control: String,
    pub value: async_graphql::types::Json<serde_json::Value>,
    pub value_type: VariantValueType,
    #[graphql(validator(max_length = 500))]
    pub description: Option<String>,
}

#[derive(InputObject, Debug)]
pub struct CreateFeatureInput {
    #[graphql(validator(min_length = 3, max_length = 40))]
    pub key: String,
    #[graphql(validator(min_length = 3, max_length = 255))]
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    #[graphql(validator(min_items = 0))]
    pub dependencies: Vec<ID>,
    #[graphql(validator(min_items = 0))]
    pub relationships: Vec<CreateRelationshipInput>,
    #[graphql(validator(min_items = 1))]
    pub stages: Vec<CreateFeatureStageInput>,
    #[graphql(validator(min_items = 0))]
    pub variants: Option<Vec<CreateFeatureVariantInput>>,
}

#[derive(InputObject, Debug)]
pub struct UpdateFeatureInput {
    #[graphql(validator(min_length = 3, max_length = 100))]
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    #[graphql(validator(min_items = 0))]
    pub dependencies: Vec<ID>,
    #[graphql(validator(min_items = 0))]
    pub relationships: Vec<CreateRelationshipInput>,
    #[graphql(validator(min_items = 1))]
    pub stages: Vec<CreateFeatureStageInput>,
    #[graphql(validator(min_items = 0))]
    pub variants: Option<Vec<CreateFeatureVariantInput>>,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateFeatureStageInput {
    pub id: Option<ID>,
    pub environment_id: ID,
    #[graphql(validator(minimum = 0))]
    pub order_index: i32,
    #[graphql(validator(min_length = 1, max_length = 50))]
    pub position: String,
    pub bucketing_key: Option<String>,
}

pub trait StageInput {
    fn environment_id(&self) -> &ID;
    fn order_index(&self) -> i32;
}

impl StageInput for CreateFeatureStageInput {
    fn environment_id(&self) -> &ID {
        &self.environment_id
    }

    fn order_index(&self) -> i32 {
        self.order_index
    }
}

#[derive(InputObject, Debug)]
pub struct CreateEnvironmentInput {
    #[graphql(validator(min_length = 3, max_length = 50))]
    pub name: String,
    pub active: bool,
    pub environment_type: Option<String>,
}

#[derive(InputObject, Debug)]
pub struct CreatePipelineInput {
    #[graphql(validator(min_length = 5, max_length = 100))]
    pub name: String,
    #[graphql(validator(min_items = 1))]
    pub stages: Vec<CreateStageInput>,
    pub relationships: Vec<CreateRelationshipInput>,
}

#[derive(InputObject, Debug)]
pub struct CreateRelationshipInput {
    #[graphql(validator(minimum = 0))]
    pub source_id: i32,
    #[graphql(validator(minimum = 1))]
    pub target_id: i32,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateStageInput {
    pub environment_id: ID,
    #[graphql(validator(minimum = 0))]
    pub order_index: i32,
    #[graphql(validator(min_length = 1, max_length = 50))]
    pub position: String,
}

impl StageInput for CreateStageInput {
    fn environment_id(&self) -> &ID {
        &self.environment_id
    }

    fn order_index(&self) -> i32 {
        self.order_index
    }
}

#[derive(InputObject, Debug)]
pub struct UpdatePipelineInput {
    #[graphql(validator(min_length = 5, max_length = 100))]
    pub name: Option<String>,
    pub active: Option<bool>,
    #[graphql(validator(min_items = 1))]
    pub stages: Vec<CreateStageInput>,
    pub relationships: Vec<CreateRelationshipInput>,
}

#[derive(InputObject, Debug)]
pub struct UpdateStageInput {
    pub id: ID,
    pub pipeline_id: ID,
    pub environment_id: ID,
    pub parent_stage_id: Option<ID>,
    #[graphql(validator(minimum = 0))]
    pub order: i32,
}

#[derive(InputObject, Debug)]
pub struct UpdateEnvironmentInput {
    #[graphql(validator(min_length = 3, max_length = 50))]
    pub name: Option<String>,
    pub active: Option<bool>,
    pub environment_type: Option<String>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Team {
    pub id: ID,
    pub name: String,
    pub description: String,
}

#[derive(InputObject)]
pub struct CreateTeamInput {
    #[graphql(validator(min_length = 3, max_length = 50))]
    pub name: String,
    #[graphql(validator(min_length = 0, max_length = 200))]
    pub description: String,
}

#[derive(InputObject)]
pub struct UpdateTeamInput {
    #[graphql(validator(min_length = 3, max_length = 50))]
    pub name: Option<String>,
    #[graphql(validator(min_length = 0, max_length = 200))]
    pub description: Option<String>,
}

// Keep the trait for backward compatibility, but don't use it with trait objects
pub trait Relationship {}

impl Relationship for PipelineRelationship {}

impl Relationship for FeatureRelationship {}

pub trait Stage {}

impl Stage for FeatureStage {}

impl Stage for PipelineStage {}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum ClientType {
    Web,
    Backend,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Client {
    pub id: ID,
    pub team_id: ID,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub client_type: ClientType,
    pub api_key: String,
    pub web_origins: Vec<String>,
}

// Team-scoped Contexts
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ContextEntry {
    pub id: ID,
    pub value: String,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    pub id: ID,
    pub team_id: ID,
    pub key: String,
    pub entries: Vec<ContextEntry>,
}

#[derive(InputObject, Debug)]
pub struct CreateContextInput {
    pub key: String,
    pub entries: Vec<String>,
}

#[derive(InputObject, Debug)]
pub struct UpdateContextInput {
    pub key: Option<String>,
    pub entries: Option<Vec<String>>,
}

#[derive(InputObject, Debug)]
pub struct CreateClientInput {
    #[graphql(validator(min_length = 3, max_length = 100))]
    pub name: String,
    #[graphql(validator(min_length = 0, max_length = 500))]
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: ClientType,
    pub web_origins: Option<Vec<String>>,
}

#[derive(InputObject, Debug)]
pub struct UpdateClientInput {
    #[graphql(validator(min_length = 3, max_length = 100))]
    pub name: Option<String>,
    #[graphql(validator(min_length = 0, max_length = 500))]
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: Option<ClientType>,
    pub web_origins: Option<Vec<String>>,
}

// Stage criteria GraphQL types
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct StageCriterion {
    pub id: ID,
    pub stage_id: ID,
    pub priority: i32,
    pub rule_groups: Vec<CompoundRuleGroup>,
    /// Weighted variant allocations for multi-variant traffic splits
    /// If present, overrides the simple serve field with weighted distribution
    pub variant_allocations: Vec<VariantAllocation>,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateStageCriterionInput {
    #[graphql(default = 0)]
    pub priority: i32,
    /// Optional weighted variant allocations for this criterion
    #[graphql(default)]
    pub variant_allocations: Option<Vec<CreateVariantAllocationInput>>,
    /// Optional compound rule groups for this criterion
    #[graphql(default)]
    pub rule_groups: Option<Vec<InlineRuleGroupInput>>,
}

// Compound rules GraphQL types
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct CompoundRuleGroup {
    pub id: ID,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CompoundRuleCondition>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct CompoundRuleCondition {
    pub id: ID,
    pub context_key: String,
    pub operator: RuleOperator,
    pub value: async_graphql::Json<serde_json::Value>,
    pub order_index: i32,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum LogicOperator {
    #[graphql(name = "AND")]
    And,
    #[graphql(name = "OR")]
    Or,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateRuleGroupInput {
    pub criteria_id: ID,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionInput>,
}

/// Rule group input used when nested under setStageCriteria (criteria_id inferred)
#[derive(InputObject, Debug, Clone)]
pub struct InlineRuleGroupInput {
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionInput>,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateRuleConditionInput {
    #[graphql(validator(min_length = 1, max_length = 100))]
    pub context_key: String,
    pub operator: RuleOperator,
    pub value: async_graphql::Json<serde_json::Value>,
    #[graphql(default = 0)]
    pub order_index: i32,
}

#[derive(InputObject, Debug, Clone)]
pub struct UpdateRuleGroupInput {
    pub logic_operator: Option<LogicOperator>,
    pub conditions: Option<Vec<CreateRuleConditionInput>>,
}

// Variant allocations GraphQL types (for weighted traffic splits)
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct VariantAllocation {
    pub id: ID,
    pub criteria_id: ID,
    pub variant_control: String,
    #[graphql(validator(minimum = 0, maximum = 100))]
    pub weight: i32,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateVariantAllocationInput {
    #[graphql(validator(min_length = 1, max_length = 100))]
    pub variant_control: String,
    #[graphql(validator(minimum = 0, maximum = 100))]
    pub weight: i32,
}

#[derive(InputObject, Debug, Clone)]
pub struct UpdateVariantAllocationInput {
    #[graphql(validator(minimum = 0, maximum = 100))]
    pub weight: i32,
}

// Users GraphQL types
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
#[graphql(complex)]
pub struct User {
    pub id: ID,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_login: Option<String>,
    pub is_temporary_password: bool,
}

#[ComplexObject]
impl User {
    pub async fn teams(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<Team>> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let uid = uuid::Uuid::try_from(self.id.clone())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let repo = crate::database::user::user_repository(pool.clone());
        let teams = repo
            .get_user_teams(uid)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(teams
            .into_iter()
            .map(|t| Team {
                id: async_graphql::ID::from(t.id),
                name: t.name,
                description: t.description,
            })
            .collect())
    }

    pub async fn team_ids(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<ID>> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let uid = uuid::Uuid::try_from(self.id.clone())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let repo = crate::database::user::user_repository(pool.clone());
        let teams = repo
            .get_user_teams(uid)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(teams.into_iter().map(|t| ID::from(t.id)).collect())
    }

    pub async fn roles(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<Role>> {
        let logic = ctx.data::<Box<dyn crate::logic::role::RoleLogic>>()?;
        let roles = logic
            .get_user_roles(self.id.clone())
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(roles
            .into_iter()
            .map(|r| Role {
                id: r.id,
                name: r.name,
                description: r.description,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect())
    }
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct UsersPage {
    pub items: Vec<User>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct FeaturesPage {
    pub items: Vec<Feature>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ClientsPage {
    pub items: Vec<Client>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentsPage {
    pub items: Vec<Environment>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct PipelinesPage {
    pub items: Vec<Pipeline>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ContextsPage {
    pub items: Vec<Context>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

/// Aggregated evaluation data grouped by feature key for dashboard analytics
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct EvaluationByFeature {
    /// Feature key
    pub feature_key: String,
    /// Total number of evaluations for this feature
    pub total_evaluations: i64,
    /// Number of evaluations that resulted in true
    pub successful_evaluations: i64,
    /// Number of evaluations from prior assignments (cached)
    pub cached_evaluations: i64,
    /// Number of unique users who had evaluations for this feature
    pub unique_users: i64,
    /// Timestamp of the last evaluation for this feature
    pub last_evaluated_at: chrono::DateTime<chrono::Utc>,
}

/// Feature growth data point for dashboard analytics showing cumulative feature creation over time
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct FeatureGrowthPoint {
    /// Time bucket for this data point (day, week, or month)
    pub time_bucket: chrono::DateTime<chrono::Utc>,
    /// Team ID (null if aggregated across all teams)
    pub team_id: Option<ID>,
    /// Team name for display purposes
    pub team_name: Option<String>,
    /// Number of features created in this time bucket
    pub feature_count: i64,
    /// Cumulative count of features up to and including this time bucket
    pub cumulative_count: i64,
}

/// Entity details for activity log enrichment
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ActivityEntityDetails {
    /// Entity ID
    pub id: String,
    /// Entity name/key
    pub name: String,
    /// Entity type
    pub entity_type: String,
    /// Additional details (e.g., environment name for stages, feature key for features)
    pub details: Option<serde_json::Value>,
}

/// Activity log entry for tracking user actions and system events
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ActivityLog {
    /// Unique identifier
    pub id: ID,
    /// Type of activity (e.g., 'feature_created', 'feature_deployed', 'user_added')
    pub activity_type: String,
    /// Type of entity affected (e.g., 'feature', 'user', 'client', 'team')
    pub entity_type: String,
    /// ID of the affected entity
    pub entity_id: String,
    /// Enriched entity details (resolved from entity_type and entity_id)
    pub entity_details: Option<ActivityEntityDetails>,
    /// User who performed the action (nullable for system events)
    pub actor_id: Option<ID>,
    /// Name of the actor for display purposes
    pub actor_name: Option<String>,
    /// Human-readable description of the activity
    pub description: String,
    /// Additional context/details about the activity
    pub metadata: Option<serde_json::Value>,
    /// Timestamp when the activity occurred
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Paginated response for activity logs
#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ActivityLogPage {
    pub items: Vec<ActivityLog>,
    pub page_number: i32,
    pub page_size: i32,
    pub total: i64,
}

#[derive(InputObject, Debug)]
pub struct RegisterUserInput {
    pub username: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    #[graphql(validator(email))]
    pub email: String,
    pub is_admin: Option<bool>,
    pub is_temporary_password: Option<bool>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub user: User,
    pub token: String,
    pub is_temporary: bool,
}

#[derive(InputObject, Debug)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(InputObject, Debug)]
pub struct UpdateUserInput {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    #[graphql(validator(email))]
    pub email: Option<String>,
    pub is_admin: Option<bool>,
    pub enabled: Option<bool>,
}

#[derive(InputObject, Debug)]
pub struct ResetPasswordInput {
    pub current_password: String,
    pub new_password: String,
}

#[derive(InputObject, Debug)]
pub struct SetTemporaryPasswordInput {
    pub user_id: ID,
    pub temporary_password: String,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Role {
    pub id: ID,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(InputObject, Debug)]
pub struct AssignUserRolesInput {
    pub role_ids: Vec<ID>,
}

#[derive(InputObject, Debug)]
pub struct CreateRoleInput {
    pub name: String,
    pub description: String,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct JwtSecretResponse {
    pub id: ID,
    pub is_active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<ID>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub secret_preview: String, // Truncated version for security
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationStatus {
    pub admin_configured: bool,
}

// Input type for evaluation count filtering
#[derive(InputObject, Debug, Clone)]
pub struct EvaluationCountFilter {
    pub from_date: chrono::DateTime<chrono::Utc>,
    pub to_date: chrono::DateTime<chrono::Utc>,
    pub environment_id: Option<String>,
    pub client_id: Option<ID>,
    pub feature_key: Option<String>,
}

// Input type for evaluation summary query
#[derive(InputObject, Debug, Clone)]
pub struct EvaluationSummaryQueryInput {
    pub period: TimePeriod,
    pub environment_id: Option<String>,
    pub client_id: Option<ID>,
    pub feature_key: Option<String>,
}

// Output type for evaluation summary
#[derive(SimpleObject, Debug, Clone)]
pub struct EvaluationSummaryOutput {
    /// Total number of evaluations
    pub total_evaluations: i64,

    /// Number of evaluations that resulted in true
    pub successful_evaluations: i64,

    /// Number of evaluations from prior assignments (cached)
    pub cached_evaluations: i64,

    /// Number of unique users who had evaluations
    pub unique_users: i64,

    /// Most frequently evaluated feature key
    pub top_feature_key: Option<String>,

    /// Success rate as percentage (0-100)
    pub success_rate: f64,

    /// Cache hit rate as percentage (0-100)
    pub cache_hit_rate: f64,
}

#[derive(SimpleObject, Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub id: ID,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    #[graphql(name = "metricType")]
    pub metric_type: MetricType,
    pub unit: Option<String>,
}

#[derive(InputObject, Debug, Clone, Serialize, Deserialize)]
pub struct CreateMetricInput {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    #[graphql(name = "metricType")]
    pub metric_type: MetricType,
    pub unit: Option<String>,
    pub success_criteria: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(SimpleObject, Debug, Clone, Serialize, Deserialize)]
pub struct MetricResult {
    pub metric_key: String,
    pub variant: Option<String>,
    pub sample_size: i32,
    pub conversion_rate: Option<f64>,
    pub mean_value: Option<f64>,
    pub p95_value: Option<f64>,
    pub time_bucket: chrono::DateTime<chrono::Utc>,
    pub confidence_interval: Option<Vec<f64>>,
}

#[derive(SimpleObject, Debug, Clone, Serialize, Deserialize)]
pub struct MetricAnalysis {
    pub metric_key: String,
    pub results: Vec<MetricResult>,
    pub winner: Option<String>,
    pub statistical_significance: Option<f64>,
}

#[derive(SimpleObject, Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentAnalysis {
    pub feature_key: String,
    pub metrics: Vec<MetricAnalysis>,
}

// Rollout metrics for dashboard
#[derive(SimpleObject, Debug, Clone)]
pub struct RolloutMetrics {
    /// Average time features spend in the pipeline (hours)
    pub average_time_in_pipeline: f64,

    /// Approval rate as a percentage (0-100)
    pub approval_rate: f64,

    /// Number of features deployed this week
    pub features_deployed_this_week: i32,

    /// Number of features deployed last week
    pub features_deployed_last_week: i32,

    /// Week-over-week deployment change percentage
    pub deployment_change: f64,

    /// Name of the stage causing the biggest bottleneck
    pub bottleneck_stage: String,

    /// Average wait time at the bottleneck stage (hours)
    pub bottleneck_duration: f64,

    /// Total number of features waiting for approval
    pub total_pending_approvals: i32,
}
