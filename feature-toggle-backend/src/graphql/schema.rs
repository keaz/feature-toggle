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
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct PipelineStage {
    id: ID,
    pub environment: Environment,
    pub order: i32,
    pub parent_stage_id: Option<Box<PipelineStage>>,
    pub child_stages: Vec<PipelineStage>,
    pub team: Option<Team>,
}

// Input types for mutations
#[derive(InputObject, Debug)]
pub struct CreateFeatureInput {
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub rules: Option<Vec<ContextRuleInput>>,
    pub dependencies: Vec<ID>,
}

#[derive(InputObject, Debug)]
pub struct ContextRuleInput {
    pub key: String,
    pub value: String,
    pub operator: RuleOperator,
}

#[derive(InputObject, Debug)]
pub struct CreateEnvironmentInput {
    pub name: String,
    pub active: bool,
}

#[derive(InputObject, Debug)]
pub struct CreatePipelineInput {
    pub name: String,
    pub stages: Vec<CreateStageInput>,
    pub relationships: Vec<CreateRelationshipInput>,
}

#[derive(InputObject, Debug)]
pub struct CreateRelationshipInput {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateStageInput {
    pub environment_id: ID,
    pub order: i32,
    pub position: String,
}

#[derive(InputObject, Debug)]
pub struct UpdatePipelineInput {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub stages: Vec<UpdateStageInput>,
}

#[derive(InputObject, Debug)]
pub struct UpdateStageInput {
    pub id: ID,
    pub pipeline_id: ID,
    pub environment_id: ID,
    pub parent_stage_id: Option<ID>,
    pub order: i32,
}

#[derive(InputObject, Debug)]
pub struct UpdateEnvironmentInput {
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
    pub name: String,
    pub description: String,
}

#[derive(InputObject)]
pub struct UpdateTeamInput {
    pub name: Option<String>,
    pub description: Option<String>,
}
