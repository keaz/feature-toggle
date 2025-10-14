use async_graphql::{ComplexObject, Enum, ID, InputObject, Result as GqlResult, SimpleObject};
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
    pub kill_switch_enabled: bool,
    pub kill_switch_activated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub rollback_scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
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
    pub from_time: chrono::DateTime<chrono::Utc>,
    pub to_time: chrono::DateTime<chrono::Utc>,
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
