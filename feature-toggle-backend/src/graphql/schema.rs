use async_graphql::{Enum, InputObject, SimpleObject, ID};
use serde::{Deserialize, Serialize};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum FeatureType {
    Simple,
    Contextual,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum RuleOperator {
    Equals,
    NotEquals,
    Contains,
    GreaterThan,
    LessThan,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Feature {
    pub id: ID,
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub rules: Option<Vec<ContextRule>>,
    pub dependencies: Vec<ID>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ContextRule {
    pub key: String,
    pub value: String,
    pub operator: RuleOperator,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub id: ID,
    pub name: String,
    pub active: bool,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: ID,
    pub name: String,
    pub active: bool,
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
#[derive(InputObject, Debug)]
pub struct CreateFeatureInput {
    #[graphql(validator(min_length = 3, max_length = 100))]
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub rules: Option<Vec<ContextRuleInput>>,
    #[graphql(validator(min_items = 0))]
    pub dependencies: Vec<ID>,
    #[graphql(validator(min_items = 0))]
    pub relationships: Vec<CreateRelationshipInput>,
    #[graphql(validator(min_items = 1))]
    pub stages: Vec<CreateFeatureStageInput>,
}

#[derive(InputObject, Debug)]
pub struct UpdateFeatureInput {
    pub id: ID,
    #[graphql(validator(min_length = 3, max_length = 100))]
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub rules: Option<Vec<ContextRuleInput>>,
    #[graphql(validator(min_items = 0))]
    pub dependencies: Vec<ID>,
    #[graphql(validator(min_items = 0))]
    pub relationships: Vec<CreateRelationshipInput>,
    #[graphql(validator(min_items = 1))]
    pub stages: Vec<CreateFeatureStageInput>,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateFeatureStageInput {
    pub id: Option<ID>,
    pub environment_id: ID,
    #[graphql(validator(minimum = 0))]
    pub order_index: i32,
    #[graphql(validator(min_length = 1, max_length = 50))]
    pub position: String,
}

#[derive(InputObject, Debug)]
pub struct ContextRuleInput {
    #[graphql(validator(min_length = 1, max_length = 50))]
    pub key: String,
    #[graphql(validator(min_length = 1, max_length = 100))]
    pub value: String,
    pub operator: RuleOperator,
}

#[derive(InputObject, Debug)]
pub struct CreateEnvironmentInput {
    #[graphql(validator(min_length = 3, max_length = 50))]
    pub name: String,
    pub active: bool,
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
