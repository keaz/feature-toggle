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
    pub stages: Vec<PipelineStage>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct PipelineStage {
    pub environment: Environment,
    pub order: i32,
}

// Input types for mutations

#[derive(InputObject)]
pub struct CreateFeatureInput {
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub rules: Option<Vec<ContextRuleInput>>,
    pub dependencies: Vec<ID>,
}

#[derive(InputObject)]
pub struct ContextRuleInput {
    pub key: String,
    pub value: String,
    pub operator: RuleOperator,
}

#[derive(InputObject)]
pub struct CreateEnvironmentInput {
    pub name: String,
}

#[derive(InputObject)]
pub struct CreatePipelineInput {
    pub name: String,
}

#[derive(InputObject)]
pub struct UpdatePipelineInput {
    pub id: ID,
    pub name: Option<String>,
    pub active: Option<bool>,
}

#[derive(InputObject)]
pub struct UpdateEnvironmentInput {
    pub id: ID,
    pub name: Option<String>,
    pub active: Option<bool>,
}

#[derive(InputObject)]
pub struct PipelineStageInput {
    pub environment_id: ID,
    pub order: i32,
}
