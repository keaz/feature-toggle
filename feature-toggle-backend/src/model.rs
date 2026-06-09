use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ID(String);

impl ID {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(&self) -> Result<Uuid, uuid::Error> {
        Uuid::parse_str(&self.0)
    }
}

impl Default for ID {
    fn default() -> Self {
        ID::from(Uuid::nil())
    }
}

impl From<Uuid> for ID {
    fn from(value: Uuid) -> Self {
        Self(value.to_string())
    }
}

impl From<&Uuid> for ID {
    fn from(value: &Uuid) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ID {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ID {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for ID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<ID> for Uuid {
    type Error = uuid::Error;

    fn try_from(value: ID) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value.0)
    }
}

impl TryFrom<&ID> for Uuid {
    type Error = uuid::Error;

    fn try_from(value: &ID) -> Result<Self, Self::Error> {
        Uuid::parse_str(&value.0)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeatureType {
    Simple,
    Contextual,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VariantValueType {
    String,
    Number,
    Boolean,
    Json,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
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
    #[default]
    In,
    NotIn,
    SemverGreaterThan,
    SemverLessThan,
}

impl RuleOperator {
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

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
pub enum LifecycleStage {
    Draft,
    #[default]
    Active,
    Deprecated,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub id: ID,
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub kill_switch_enabled: bool,
    pub kill_switch_activated_at: Option<DateTime<Utc>>,
    pub rollback_scheduled_at: Option<DateTime<Utc>>,
    pub emergency_override_reason: Option<String>,
    pub emergency_override_expires_at: Option<DateTime<Utc>>,
    pub emergency_override_actor_id: Option<ID>,
    pub emergency_override_applied_at: Option<DateTime<Utc>>,
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
    pub dependencies: Vec<ID>,
    pub team_id: ID,
    pub pending_approval_request_id: Option<ID>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct FeatureRelationship {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureStage {
    pub id: ID,
    pub environment: Environment,
    pub order_index: i32,
    pub position: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureVariant {
    pub id: ID,
    pub feature_id: ID,
    pub control: String,
    pub value: JsonValue,
    pub value_type: VariantValueType,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextualType {
    pub id: ID,
    pub key: String,
    pub entries: Vec<ContextualEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextualEntry {
    pub id: ID,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: ID,
    pub name: String,
    pub team_id: ID,
    pub active: bool,
    pub environment_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: ID,
    pub name: String,
    pub active: bool,
    pub team_id: ID,
    pub stages: Vec<PipelineStage>,
    pub relationships: Vec<PipelineRelationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct PipelineRelationship {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    pub id: ID,
    pub environment: Environment,
    pub order_index: i32,
    pub position: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: ID,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientType {
    Web,
    Backend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Client {
    pub id: ID,
    pub team_id: ID,
    pub environment_id: ID,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub client_type: ClientType,
    pub api_key: String,
    pub web_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemClient {
    pub id: ID,
    pub team_id: ID,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub id: ID,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context {
    pub id: ID,
    pub team_id: ID,
    pub key: String,
    pub entries: Vec<ContextEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageCriterion {
    pub id: ID,
    pub stage_id: ID,
    pub priority: i32,
    pub rule_groups: Vec<CompoundRuleGroup>,
    pub variant_allocations: Vec<VariantAllocation>,
    pub variant_selection_mode: VariantSelectionMode,
    pub selected_variant_control: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundRuleGroup {
    pub id: ID,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CompoundRuleCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundRuleCondition {
    pub id: ID,
    pub context_key: String,
    pub operator: RuleOperator,
    pub value: JsonValue,
    pub order_index: i32,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogicOperator {
    And,
    Or,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
pub enum VariantSelectionMode {
    #[default]
    WeightedSplit,
    SpecificVariant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantAllocation {
    pub id: ID,
    pub criteria_id: ID,
    pub variant_control: String,
    pub weight: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeatureVariantInput {
    pub control: String,
    pub value: JsonValue,
    pub value_type: VariantValueType,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeatureInput {
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub lifecycle_stage: Option<LifecycleStage>,
    pub owner: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub cleanup_reason: Option<String>,
    pub dependencies: Vec<ID>,
    pub relationships: Vec<CreateRelationshipInput>,
    pub stages: Vec<CreateFeatureStageInput>,
    pub variants: Option<Vec<CreateFeatureVariantInput>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFeatureInput {
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub lifecycle_stage: Option<LifecycleStage>,
    pub owner: Option<Option<String>>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
    pub cleanup_reason: Option<Option<String>>,
    pub archive_confirmation: bool,
    pub dependencies: Vec<ID>,
    pub relationships: Vec<CreateRelationshipInput>,
    pub stages: Vec<CreateFeatureStageInput>,
    pub variants: Option<Vec<CreateFeatureVariantInput>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeatureStageInput {
    pub id: Option<ID>,
    pub environment_id: ID,
    pub order_index: i32,
    pub position: String,
    pub bucketing_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnvironmentInput {
    pub name: String,
    pub active: bool,
    pub environment_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePipelineInput {
    pub name: String,
    pub stages: Vec<CreateStageInput>,
    pub relationships: Vec<CreateRelationshipInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRelationshipInput {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStageInput {
    pub environment_id: ID,
    pub order_index: i32,
    pub position: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePipelineInput {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub stages: Vec<CreateStageInput>,
    pub relationships: Vec<CreateRelationshipInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStageInput {
    pub id: ID,
    pub pipeline_id: ID,
    pub environment_id: ID,
    pub parent_stage_id: Option<ID>,
    pub order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEnvironmentInput {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub environment_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTeamInput {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTeamInput {
    pub name: Option<String>,
    pub description: Option<String>,
}

pub trait Relationship {}

impl Relationship for PipelineRelationship {}
impl Relationship for FeatureRelationship {}

pub trait Stage {}

impl Stage for FeatureStage {}
impl Stage for PipelineStage {}

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

impl StageInput for CreateStageInput {
    fn environment_id(&self) -> &ID {
        &self.environment_id
    }

    fn order_index(&self) -> i32 {
        self.order_index
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateContextInput {
    pub key: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateContextInput {
    pub key: Option<String>,
    pub entries: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClientInput {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: ClientType,
    pub web_origins: Option<Vec<String>>,
    pub environment_id: ID,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateClientInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: Option<ClientType>,
    pub web_origins: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSystemClientInput {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSystemClientInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStageCriterionInput {
    pub priority: i32,
    pub variant_allocations: Option<Vec<CreateVariantAllocationInput>>,
    pub rule_groups: Option<Vec<InlineRuleGroupInput>>,
    pub variant_selection_mode: Option<VariantSelectionMode>,
    pub selected_variant_control: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRuleGroupInput {
    pub criteria_id: ID,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineRuleGroupInput {
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRuleConditionInput {
    pub context_key: String,
    pub operator: RuleOperator,
    pub value: JsonValue,
    pub order_index: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRuleGroupInput {
    pub logic_operator: Option<LogicOperator>,
    pub conditions: Option<Vec<CreateRuleConditionInput>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVariantAllocationInput {
    pub variant_control: String,
    pub weight: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateVariantAllocationInput {
    pub weight: i32,
}
