use async_graphql::{ComplexObject, Enum, InputObject, Result as GqlResult, SimpleObject, ID};
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
    pub key: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub enabled: Option<bool>,
    pub dependencies: Vec<ID>,
    pub relationships: Vec<FeatureRelationship>,
    pub stages: Vec<FeatureStage>,
    pub team_id: ID,
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
    pub context_key: String,
    pub context: super::schema::Context,
    pub rollout_percentage: i32,
}

#[derive(InputObject, Debug, Clone)]
pub struct CreateStageCriterionInput {
    #[graphql(validator(min_length = 1, max_length = 100))]
    pub context_key: String,
    pub context_id: ID,
    #[graphql(validator(minimum = 0, maximum = 100))]
    pub rollout_percentage: i32,
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
}

#[ComplexObject]
impl User {
    pub async fn teams(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<Team>> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let uid = uuid::Uuid::try_from(self.id.clone()).map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let repo = crate::database::user::user_repository(pool.clone());
        let teams = repo
            .get_user_teams(uid)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(teams
            .into_iter()
            .map(|t| Team { id: async_graphql::ID::from(t.id), name: t.name, description: t.description })
            .collect())
    }

    pub async fn team_ids(&self, ctx: &async_graphql::Context<'_>) -> GqlResult<Vec<ID>> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let uid = uuid::Uuid::try_from(self.id.clone()).map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let repo = crate::database::user::user_repository(pool.clone());
        let teams = repo
            .get_user_teams(uid)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(teams.into_iter().map(|t| ID::from(t.id)).collect())
    }
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct UsersPage {
    pub items: Vec<User>,
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
