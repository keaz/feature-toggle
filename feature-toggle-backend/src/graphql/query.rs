use crate::graphql::create_user;
use crate::graphql::schema::{
    ActivityLog, ActivityLogPage, ApplicationStatus, Client, ClientType, ClientsPage, ContextsPage,
    Environment, EnvironmentsPage, EvaluationByFeature, EvaluationCountFilter,
    EvaluationSummaryOutput, EvaluationSummaryQueryInput, ExperimentAnalysis, Feature,
    FeatureGrowthPoint, FeatureType, FeaturesPage, JwtSecretResponse, Metric, MetricAnalysis,
    MetricResult, Pipeline, PipelinesPage, Role, RolloutMetrics, Team, User, UsersPage,
};
use crate::graphql::subscription::calculate_time_range;
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::metrics::MetricLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::role::RoleLogic;
use crate::logic::team::TeamLogic;
use crate::logic::user::UserLogic;
use async_graphql::{Context, ID, Object, Result as GqlResult};
use chrono::{DateTime, Utc};
use log::debug;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub struct Query;

#[Object]
impl Query {
    async fn environment(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of object")] id: ID,
    ) -> GqlResult<Environment> {
        debug!("Fetching environment with id: {id:?}");
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        Ok(repository.get_environment_by_id(id).await?)
    }

    async fn environments(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the environment")] name: Option<String>,
        #[graphql(desc = "Active status of the environment")] active: Option<bool>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<EnvironmentsPage> {
        debug!("Fetching environments with name: {name:?} and active: {active:?}");
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>()?;

        // If pagination parameters are provided, use paginated version
        if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let (items, total) = repository
                .get_environments_paginated(team_id, name, active, page_num, page_sz)
                .await?;
            Ok(EnvironmentsPage {
                items,
                page_number: page_num,
                page_size: page_sz,
                total,
            })
        } else {
            // Fallback to non-paginated for backward compatibility
            let items = repository.get_environments(team_id, name, active).await?;
            let total = items.len() as i64;
            Ok(EnvironmentsPage {
                items,
                page_number: 1,
                page_size: total as i32,
                total,
            })
        }
    }

    async fn teams(&self, ctx: &Context<'_>) -> GqlResult<Vec<Team>> {
        let jwt_user = ctx.data::<crate::JwtUser>()?;

        if jwt_user.is_admin {
            debug!("Fetching all teams for admin user: {}", jwt_user.username);
            let team_logic = ctx.data::<Box<dyn TeamLogic>>()?;
            Ok(team_logic.get_teams(None).await?)
        } else {
            debug!(
                "Fetching teams for user: {} (user_id: {})",
                jwt_user.username, jwt_user.id
            );
            let user_logic = ctx.data::<Box<dyn UserLogic>>()?;
            Ok(user_logic.get_user_teams(ID::from(jwt_user.id)).await?)
        }
    }

    async fn pipelines(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the environment")] name: Option<String>,
        #[graphql(desc = "Active status of the environment")] active: Option<bool>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<PipelinesPage> {
        debug!("Fetching pipelines for team with id: {team_id:?}");

        let mut fields = vec![];
        if ctx.look_ahead().field("items").field("stages").exists() {
            fields.push("stages".to_string());
        }

        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;

        // If pagination parameters are provided, use paginated version
        if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let (items, total) = logic
                .get_pipelines_paginated(team_id, name, active, fields, page_num, page_sz)
                .await?;
            Ok(PipelinesPage {
                items,
                page_number: page_num,
                page_size: page_sz,
                total,
            })
        } else {
            // Fallback to non-paginated for backward compatibility
            let items = logic.get_pipelines(team_id, name, active, fields).await?;
            let total = items.len() as i64;
            Ok(PipelinesPage {
                items,
                page_number: 1,
                page_size: total as i32,
                total,
            })
        }
    }

    async fn pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the Pipeline")] id: ID,
    ) -> GqlResult<Pipeline> {
        debug!("Fetching pipeline with id: {id:?}");
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        Ok(logic.get_pipeline_by_id(id).await?)
    }

    async fn feature(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature")] id: ID,
    ) -> GqlResult<Feature> {
        debug!("Fetching feature with id: {id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        Ok(logic.get_feature_by_id(id).await?)
    }

    async fn features(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the feature")] name: Option<String>,
        #[graphql(desc = "Type of the feature")] feature_type: Option<FeatureType>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<FeaturesPage> {
        debug!("Fetching features for team with id: {team_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;

        // If pagination parameters are provided, use paginated version
        if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let (items, total) = logic
                .get_features_paginated(team_id, name, feature_type, page_num, page_sz)
                .await?;
            Ok(FeaturesPage {
                items,
                page_number: page_num,
                page_size: page_sz,
                total,
            })
        } else {
            // Fallback to non-paginated for backward compatibility
            let items = logic.get_features(team_id, name, feature_type).await?;
            let total = items.len() as i64;
            Ok(FeaturesPage {
                items,
                page_number: 1,
                page_size: total as i32,
                total,
            })
        }
    }

    /// Count features - useful for dashboard metrics
    async fn features_count(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Optional team ID to filter by")] team_id: Option<ID>,
    ) -> GqlResult<i64> {
        debug!("Counting features with team_id: {team_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        Ok(logic.count_features(team_id).await?)
    }

    /// Get features with pending approvals (DEPLOYMENT_REQUESTED or ROLLBACK_REQUESTED status)
    async fn pending_approvals(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Optional team ID to filter by")] team_id: Option<ID>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<FeaturesPage> {
        debug!("Fetching features with pending approvals, team_id: {team_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;

        let (items, total) = logic
            .get_features_with_pending_approvals(team_id, page_number, page_size)
            .await?;

        let page_num = page_number.unwrap_or(1);
        let page_sz = page_size.unwrap_or(items.len() as i32);

        Ok(FeaturesPage {
            items,
            page_number: page_num,
            page_size: page_sz,
            total,
        })
    }

    /// Get features with active kill switches
    async fn active_kill_switches(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Optional team ID to filter by")] team_id: Option<ID>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<FeaturesPage> {
        debug!("Fetching features with active kill switches, team_id: {team_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;

        let (items, total) = logic
            .get_features_with_kill_switches(team_id, page_number, page_size)
            .await?;

        let page_num = page_number.unwrap_or(1);
        let page_sz = page_size.unwrap_or(items.len() as i32);

        Ok(FeaturesPage {
            items,
            page_number: page_num,
            page_size: page_sz,
            total,
        })
    }

    async fn client(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the client")] id: ID,
    ) -> GqlResult<Client> {
        debug!("Fetching client with id: {id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>()?;
        Ok(logic.get_client_by_id(id).await?)
    }

    async fn clients(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the client")] name: Option<String>,
        #[graphql(desc = "Enabled status of the client")] enabled: Option<bool>,
        #[graphql(desc = "Type of the client")] client_type: Option<ClientType>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<ClientsPage> {
        debug!("Fetching clients for team with id: {team_id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>()?;

        // If pagination parameters are provided, use paginated version
        if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let (items, total) = logic
                .get_clients_paginated(team_id, name, enabled, client_type, page_num, page_sz)
                .await?;
            Ok(ClientsPage {
                items,
                page_number: page_num,
                page_size: page_sz,
                total,
            })
        } else {
            // Fallback to non-paginated for backward compatibility
            let items = logic
                .get_clients(team_id, name, enabled, client_type)
                .await?;
            let total = items.len() as i64;
            Ok(ClientsPage {
                items,
                page_number: 1,
                page_size: total as i32,
                total,
            })
        }
    }

    /// Count clients - useful for dashboard metrics
    async fn clients_count(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Optional team ID to filter by")] team_id: Option<ID>,
        #[graphql(desc = "Filter by enabled status")] enabled: Option<bool>,
    ) -> GqlResult<i64> {
        debug!("Counting clients with team_id: {team_id:?}, enabled: {enabled:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>()?;
        Ok(logic.count_clients(team_id, enabled).await?)
    }

    async fn context(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the context")] id: ID,
    ) -> GqlResult<crate::graphql::schema::Context> {
        debug!("Fetching context with id: {id:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>()?;
        Ok(logic.get_context_by_id(id).await?)
    }

    async fn contexts(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Filter by key (ILIKE)")] key: Option<String>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size")] page_size: Option<i32>,
    ) -> GqlResult<ContextsPage> {
        debug!("Fetching contexts for team with id: {team_id:?} key={key:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();

        // If pagination parameters are provided, use paginated version
        if let (Some(page_num), Some(page_sz)) = (page_number, page_size) {
            let (items, total) = logic
                .get_contexts_paginated(team_id, key, page_num, page_sz)
                .await?;
            Ok(ContextsPage {
                items,
                page_number: page_num,
                page_size: page_sz,
                total,
            })
        } else {
            // Fallback to non-paginated for backward compatibility
            let items = logic.get_contexts(team_id, key).await?;
            let total = items.len() as i64;
            Ok(ContextsPage {
                items,
                page_number: 1,
                page_size: total as i32,
                total,
            })
        }
    }

    async fn stage_contexts(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
    ) -> GqlResult<Vec<crate::graphql::schema::Context>> {
        debug!("Fetching contexts for stage id: {stage_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        Ok(logic.get_stage_contexts(stage_id).await?)
    }

    async fn get_stage_criteria(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
    ) -> GqlResult<Vec<crate::graphql::schema::StageCriterion>> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.get_stage_criteria(stage_id).await?)
    }

    // Users
    async fn user(&self, ctx: &Context<'_>, id: ID) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let u = logic.get_user_by_id(id).await?;
        create_user(u)
    }

    async fn user_by_username(&self, ctx: &Context<'_>, username: String) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let u = logic.get_user_by_username(username).await?;
        create_user(u)
    }

    async fn users(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Filter by team id")] team_id: Option<ID>,
        #[graphql(desc = "Search by first/last/username (ILIKE)")] name: Option<String>,
        #[graphql(desc = "Page number (1-based)")] page_number: i32,
        #[graphql(desc = "Page size")] page_size: i32,
    ) -> GqlResult<UsersPage> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let (items, total) = logic
            .search_users(team_id, name, page_number, page_size)
            .await?;
        let items: Vec<User> = items
            .into_iter()
            .map(create_user)
            .collect::<Result<_, _>>()?;
        Ok(UsersPage {
            items,
            page_number,
            page_size,
            total,
        })
    }

    async fn roles(&self, ctx: &Context<'_>) -> GqlResult<Vec<Role>> {
        debug!("Fetching all roles");
        let logic = ctx.data::<Box<dyn RoleLogic>>()?;
        let roles = logic.get_all_roles().await?;
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

    async fn user_roles(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "User ID to get roles for")] user_id: ID,
    ) -> GqlResult<Vec<Role>> {
        debug!("Fetching roles for user: {user_id:?}");
        let logic = ctx.data::<Box<dyn RoleLogic>>()?;
        let roles = logic.get_user_roles(user_id).await?;
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

    async fn application_status(&self, ctx: &Context<'_>) -> GqlResult<ApplicationStatus> {
        debug!("Checking application status (admin configuration)");
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let admin_configured = logic.admin_exists().await?;
        Ok(ApplicationStatus { admin_configured })
    }

    /// Get aggregated evaluation data grouped by feature key for dashboard analytics
    /// Returns top features by evaluation count with comprehensive metrics
    async fn evaluations_by_feature(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Start time for evaluation data")] from_time: DateTime<Utc>,
        #[graphql(desc = "End time for evaluation data")] to_time: DateTime<Utc>,
        #[graphql(desc = "Filter by environment ID")] environment_id: Option<String>,
        #[graphql(desc = "Filter by client ID")] client_id: Option<ID>,
        #[graphql(desc = "Maximum number of results to return")] limit: Option<i32>,
        #[graphql(desc = "Number of results to skip (for pagination)")] offset: Option<i32>,
    ) -> GqlResult<Vec<EvaluationByFeature>> {
        debug!(
            "Fetching evaluations by feature from {:?} to {:?} with environment: {:?}, client: {:?}",
            from_time, to_time, environment_id, client_id
        );

        // Retrieve feature evaluation logic (preferred over direct repository access)
        let logic = ctx
            .data::<Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>>()
            .map_err(|_| {
                async_graphql::Error::new("Feature evaluation logic not found in context")
            })?;

        // Convert client_id from GraphQL ID to UUID if provided
        let client_uuid = if let Some(id) = client_id {
            Some(uuid::Uuid::parse_str(&id.to_string()).map_err(|e| {
                async_graphql::Error::new(format!("Invalid client ID format: {}", e))
            })?)
        } else {
            None
        };

        // Use logic to fetch evaluations grouped by feature; delegate to repository internally
        let results = logic
            .get_evaluations_by_feature(
                from_time,
                to_time,
                environment_id,
                client_uuid,
                limit,
                offset,
            )
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to fetch evaluations by feature: {}", e))
            })?;

        // Convert database results to GraphQL types
        Ok(results
            .into_iter()
            .map(|r| EvaluationByFeature {
                feature_key: r.feature_key,
                total_evaluations: r.total_evaluations,
                successful_evaluations: r.successful_evaluations,
                cached_evaluations: r.cached_evaluations,
                unique_users: r.unique_users,
                last_evaluated_at: r.last_evaluated_at,
            })
            .collect())
    }

    /// Get recent activity logs with optional filtering and pagination
    /// Returns a paginated list of user activities and system events
    async fn recent_activities(
        &self,
        ctx: &Context<'_>,
        #[graphql(
            desc = "Filter by activity types (e.g., ['stage_deployed', 'stage_rollbacked'])"
        )]
        activity_types: Option<Vec<String>>,
        #[graphql(desc = "Filter by entity type (e.g., 'feature', 'user')")] entity_type: Option<
            String,
        >,
        #[graphql(desc = "Filter by entity ID")] entity_id: Option<String>,
        #[graphql(desc = "Filter by actor (user) ID")] actor_id: Option<ID>,
        #[graphql(desc = "Filter activities from this date")] from_date: Option<DateTime<Utc>>,
        #[graphql(desc = "Filter activities until this date")] to_date: Option<DateTime<Utc>>,
        #[graphql(desc = "Page number (1-based)")] page_number: Option<i32>,
        #[graphql(desc = "Page size (default 20)")] page_size: Option<i32>,
    ) -> GqlResult<ActivityLogPage> {
        debug!(
            "Fetching recent activities with filters - types: {:?}, entity: {:?}",
            activity_types, entity_type
        );

        // Get the activity log repository
        let repo = ctx
            .data::<std::sync::Arc<Box<dyn crate::database::activity_log::ActivityLogRepository>>>(
            )?;
        let repo = repo.as_ref();

        // Convert actor_id from GraphQL ID to UUID if provided
        let actor_uuid = if let Some(id) = actor_id {
            Some(uuid::Uuid::parse_str(&id.to_string()).map_err(|e| {
                async_graphql::Error::new(format!("Invalid actor ID format: {}", e))
            })?)
        } else {
            None
        };

        // Set default pagination values
        let page_num = page_number.unwrap_or(1);
        let page_sz = page_size.unwrap_or(20);

        // Calculate offset
        let offset = (page_num - 1) * page_sz;

        // Build filter
        let filter = crate::database::activity_log::ActivityLogFilter {
            activity_types,
            entity_type,
            entity_id,
            actor_id: actor_uuid,
            from_date,
            to_date,
            limit: Some(page_sz),
            offset: Some(offset),
        };

        // Call the repository method
        let (activities, total) = repo
            .get_activities_paginated(filter)
            .await
            .map_err(|e| async_graphql::Error::new(format!("Database error: {}", e)))?;

        // Get feature repository and environment logic to resolve entity details
        let feature_repo =
            ctx.data::<std::sync::Arc<Box<dyn crate::database::feature::FeatureRepository>>>()?;
        let feature_repo = feature_repo.as_ref();

        let environment_logic =
            ctx.data::<Box<dyn crate::logic::environment::EnvironmentLogic>>()?;

        // Convert database results to GraphQL types and enrich with entity details
        let mut items = Vec::new();
        for a in activities {
            let entity_details = resolve_entity_details(
                &a.entity_type,
                &a.entity_id,
                &a.metadata,
                feature_repo.as_ref(),
                environment_logic.as_ref(),
            )
            .await;

            items.push(ActivityLog {
                id: a.id.into(),
                activity_type: a.activity_type,
                entity_type: a.entity_type.clone(),
                entity_id: a.entity_id.clone(),
                entity_details,
                actor_id: a.actor_id.map(|id| id.into()),
                actor_name: a.actor_name,
                description: a.description,
                metadata: a.metadata,
                created_at: a.created_at,
            });
        }

        Ok(ActivityLogPage {
            items,
            page_number: page_num,
            page_size: page_sz,
            total,
        })
    }

    /// Get feature growth over time for dashboard analytics
    /// Returns time-series data showing feature creation trends with optional team breakdown
    async fn feature_growth(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Start time for feature growth data")] from_time: DateTime<Utc>,
        #[graphql(desc = "End time for feature growth data")] to_time: DateTime<Utc>,
        #[graphql(desc = "Time interval: 'day', 'week', or 'month'")] interval: String,
        #[graphql(desc = "Filter by team ID (optional)")] team_id: Option<ID>,
    ) -> GqlResult<Vec<FeatureGrowthPoint>> {
        debug!(
            "Fetching feature growth from {:?} to {:?} with interval: {} and team: {:?}",
            from_time, to_time, interval, team_id
        );

        // Validate interval
        let valid_intervals = ["day", "week", "month"];
        if !valid_intervals.contains(&interval.as_str()) {
            return Err(async_graphql::Error::new(
                "Invalid interval. Must be 'day', 'week', or 'month'",
            ));
        }

        // Get the feature repository (wrapped in Arc)
        let repo =
            ctx.data::<std::sync::Arc<Box<dyn crate::database::feature::FeatureRepository>>>()?;
        let repo = repo.as_ref();

        // Convert team_id from GraphQL ID to UUID if provided
        let team_uuid =
            if let Some(id) = team_id {
                Some(uuid::Uuid::parse_str(&id.to_string()).map_err(|e| {
                    async_graphql::Error::new(format!("Invalid team ID format: {}", e))
                })?)
            } else {
                None
            };

        // Call the repository method
        let results = repo
            .get_feature_growth(from_time, to_time, interval, team_uuid)
            .await
            .map_err(|e| async_graphql::Error::new(format!("Database error: {}", e)))?;

        // Convert database results to GraphQL types
        Ok(results
            .into_iter()
            .map(|r| FeatureGrowthPoint {
                time_bucket: r.time_bucket,
                team_id: r.team_id.map(|id| id.into()),
                team_name: r.team_name,
                feature_count: r.feature_count,
                cumulative_count: r.cumulative_count,
            })
            .collect())
    }

    /// Count feature evaluations with filtering - useful for dashboard metrics
    async fn evaluation_count(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Filter options for counting evaluations")] filter: EvaluationCountFilter,
    ) -> GqlResult<i64> {
        debug!(
            "Counting evaluations from {:?} to {:?} with filters: env={:?}, client={:?}, feature={:?}",
            filter.from_date,
            filter.to_date,
            filter.environment_id,
            filter.client_id,
            filter.feature_key
        );

        let logic =
            ctx.data::<Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>>()?;

        // Convert client_id from GraphQL ID to UUID if provided
        let client_uuid = if let Some(id) = filter.client_id {
            Some(uuid::Uuid::parse_str(&id.to_string()).map_err(|e| {
                async_graphql::Error::new(format!("Invalid client ID format: {}", e))
            })?)
        } else {
            None
        };

        let count = logic
            .count_evaluations(
                filter.from_date,
                filter.to_date,
                filter.environment_id,
                client_uuid,
                filter.feature_key,
            )
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to count evaluations: {}", e))
            })?;

        Ok(count)
    }

    /// Get evaluation summary statistics - provides aggregated metrics for dashboard
    async fn evaluation_summary(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Filter options for evaluation summary")]
        input: EvaluationSummaryQueryInput,
    ) -> GqlResult<EvaluationSummaryOutput> {
        debug!(
            "Fetching evaluation summary for period: {:?} with filters: env={:?}, client={:?}, feature={:?}",
            input.period, input.environment_id, input.client_id, input.feature_key
        );

        let logic =
            ctx.data::<Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>>()?;

        // Convert client_id from GraphQL ID to UUID if provided
        let client_uuid = if let Some(id) = input.client_id {
            Some(uuid::Uuid::parse_str(&id.to_string()).map_err(|e| {
                async_graphql::Error::new(format!("Invalid client ID format: {}", e))
            })?)
        } else {
            None
        };

        // Calculate time range based on period
        let now = chrono::Utc::now();
        let (from_time, to_time) =
            crate::graphql::subscription::calculate_time_range(input.period, now);

        let summary = logic
            .get_evaluation_summary(
                input.feature_key,
                input.environment_id,
                client_uuid,
                from_time,
                to_time,
            )
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to get evaluation summary: {}", e))
            })?;

        Ok(EvaluationSummaryOutput {
            total_evaluations: summary.total_evaluations,
            successful_evaluations: summary.successful_evaluations,
            cached_evaluations: summary.cached_evaluations,
            unique_users: summary.unique_users,
            top_feature_key: summary.top_feature_key,
            success_rate: summary.success_rate,
            cache_hit_rate: summary.cache_hit_rate,
        })
    }

    /// Get evaluation rates with period - provides time-series evaluation metrics for dashboard charts
    /// Returns aggregated evaluation data bucketed by the specified time interval for the given period
    async fn evaluation_rates_with_period(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Filter options for evaluation rates with period")]
        input: crate::graphql::subscription::EvaluationRatesInputWithPeriod,
    ) -> GqlResult<Vec<crate::graphql::subscription::GqlEvaluationRatePoint>> {
        debug!(
            "Fetching evaluation rates for period: {:?} with interval {} minutes, filters: env={:?}, client={:?}, feature={:?}",
            input.period,
            input.interval_minutes,
            input.environment_id,
            input.client_id,
            input.feature_key
        );

        // Validate interval bounds
        if !(1..=60).contains(&input.interval_minutes) {
            return Err(async_graphql::Error::new(
                "Interval must be between 1 and 60 minutes",
            ));
        }

        // Parse client_id if provided
        let client_id = match input.client_id.as_ref().map(|s| uuid::Uuid::parse_str(s)) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => {
                return Err(async_graphql::Error::new("Invalid client ID format"));
            }
            None => None,
        };

        let logic =
            ctx.data::<Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>>()?;

        // Calculate time range based on period
        let now = chrono::Utc::now();
        let (from_time, to_time) =
            crate::graphql::subscription::calculate_time_range(input.period, now);

        // Fetch evaluation rates from logic layer
        let rates = logic
            .get_evaluation_rates(
                input.feature_key,
                input.environment_id,
                client_id,
                from_time,
                to_time,
                input.interval_minutes,
            )
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to get evaluation rates: {}", e))
            })?;

        // Map to GraphQL output type with calculated percentages
        let mapped = rates
            .into_iter()
            .map(|rate| {
                let success_rate = if rate.evaluation_count > 0 {
                    (rate.success_count as f64 / rate.evaluation_count as f64) * 100.0
                } else {
                    0.0
                };
                let cache_hit_rate = if rate.evaluation_count > 0 {
                    (rate.prior_assignment_count as f64 / rate.evaluation_count as f64) * 100.0
                } else {
                    0.0
                };
                crate::graphql::subscription::GqlEvaluationRatePoint {
                    time_bucket: rate.time_bucket.to_rfc3339(),
                    evaluation_count: rate.evaluation_count,
                    success_count: rate.success_count,
                    prior_assignment_count: rate.prior_assignment_count,
                    success_rate: ((success_rate * 100.0).round() / 100.0),
                    cache_hit_rate: ((cache_hit_rate * 100.0).round() / 100.0),
                }
            })
            .collect();

        Ok(mapped)
    }

    /// Get evaluation rates - provides time-series evaluation metrics for dashboard charts
    /// Returns aggregated evaluation data bucketed by the specified time interval
    async fn evaluation_rates(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Filter options for evaluation rates")]
        input: crate::graphql::subscription::EvaluationRatesInput,
    ) -> GqlResult<Vec<crate::graphql::subscription::GqlEvaluationRatePoint>> {
        debug!(
            "Fetching evaluation rates from {:?} to {:?} with interval {} minutes, filters: env={:?}, client={:?}, feature={:?}",
            input.from_time,
            input.to_time,
            input.interval_minutes,
            input.environment_id,
            input.client_id,
            input.feature_key
        );

        // Validate interval bounds
        if !(1..=60).contains(&input.interval_minutes) {
            return Err(async_graphql::Error::new(
                "Interval must be between 1 and 60 minutes",
            ));
        }

        // Validate time range
        if input.to_time < input.from_time {
            return Err(async_graphql::Error::new("toTime must be >= fromTime"));
        }

        let duration_hours = (input.to_time - input.from_time).num_hours();
        if duration_hours > 24 {
            return Err(async_graphql::Error::new(
                "Time range cannot exceed 24 hours",
            ));
        }

        // Parse client_id if provided
        let client_id = match input.client_id.as_ref().map(|s| uuid::Uuid::parse_str(s)) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => {
                return Err(async_graphql::Error::new("Invalid client ID format"));
            }
            None => None,
        };

        let logic =
            ctx.data::<Box<dyn crate::logic::feature_evaluation::FeatureEvaluationLogic>>()?;

        // Fetch evaluation rates from logic layer
        let rates = logic
            .get_evaluation_rates(
                input.feature_key,
                input.environment_id,
                client_id,
                input.from_time,
                input.to_time,
                input.interval_minutes,
            )
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to get evaluation rates: {}", e))
            })?;

        // Map to GraphQL output type with calculated percentages
        let mapped = rates
            .into_iter()
            .map(|rate| {
                let success_rate = if rate.evaluation_count > 0 {
                    (rate.success_count as f64 / rate.evaluation_count as f64) * 100.0
                } else {
                    0.0
                };
                let cache_hit_rate = if rate.evaluation_count > 0 {
                    (rate.prior_assignment_count as f64 / rate.evaluation_count as f64) * 100.0
                } else {
                    0.0
                };
                crate::graphql::subscription::GqlEvaluationRatePoint {
                    time_bucket: rate.time_bucket.to_rfc3339(),
                    evaluation_count: rate.evaluation_count,
                    success_count: rate.success_count,
                    prior_assignment_count: rate.prior_assignment_count,
                    success_rate: ((success_rate * 100.0).round() / 100.0),
                    cache_hit_rate: ((cache_hit_rate * 100.0).round() / 100.0),
                }
            })
            .collect();

        Ok(mapped)
    }

    /// Get rollout metrics for dashboard - provides insights into pipeline performance
    async fn rollout_metrics(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Optional team ID to filter metrics by team")] team_id: Option<ID>,
    ) -> GqlResult<RolloutMetrics> {
        debug!("Fetching rollout metrics for team_id={:?}", team_id);

        let logic = ctx.data::<Box<dyn crate::logic::feature::FeatureLogic>>()?;

        let metrics = logic.get_rollout_metrics(team_id).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to fetch rollout metrics: {}", e))
        })?;

        // Convert from logic layer RolloutMetrics to GraphQL RolloutMetrics
        Ok(RolloutMetrics {
            average_time_in_pipeline: metrics.average_time_in_pipeline,
            approval_rate: metrics.approval_rate,
            features_deployed_this_week: metrics.features_deployed_this_week,
            features_deployed_last_week: metrics.features_deployed_last_week,
            deployment_change: metrics.deployment_change,
            bottleneck_stage: metrics.bottleneck_stage,
            bottleneck_duration: metrics.bottleneck_duration,
            total_pending_approvals: metrics.total_pending_approvals,
        })
    }

    /// List metrics defined for a team (experiment KPIs)
    async fn metrics(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Team ID that owns the metrics")] team_id: ID,
    ) -> GqlResult<Vec<Metric>> {
        let team_uuid = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| async_graphql::Error::new(format!("Invalid team id: {}", e)))?;
        let logic = ctx.data::<Box<dyn MetricLogic>>()?;
        let metrics = logic
            .list_metrics(team_uuid)
            .await
            .map_err(|e| async_graphql::Error::new(format!("Failed to load metrics: {}", e)))?;

        Ok(metrics
            .into_iter()
            .map(|m| Metric {
                id: ID::from(m.id.to_string()),
                key: m.key,
                name: m.name,
                description: m.description,
                metric_type: m.metric_type,
                unit: m.unit,
            })
            .collect())
    }

    /// Time-series metrics by feature and environment (pre-aggregated buckets)
    async fn metrics_by_feature(
        &self,
        ctx: &Context<'_>,
        feature_key: String,
        #[graphql(desc = "Environment to scope results")] environment_id: ID,
        #[graphql(desc = "Time window for aggregation")]
        time_period: crate::graphql::subscription::TimePeriod,
    ) -> GqlResult<Vec<MetricResult>> {
        let env_uuid = Uuid::parse_str(&environment_id.to_string())
            .map_err(|e| async_graphql::Error::new(format!("Invalid environment id: {}", e)))?;

        let logic = ctx.data::<Box<dyn MetricLogic>>()?;
        let now = Utc::now();
        let (from, to) = calculate_time_range(time_period, now);

        let rows = logic
            .get_metric_results(&feature_key, Some(env_uuid), from, to)
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to load metric results: {}", e))
            })?;

        Ok(rows
            .into_iter()
            .map(|row| MetricResult {
                metric_key: row.metric_key,
                variant: row.variant,
                sample_size: std::cmp::min(row.sample_size, i32::MAX as i64) as i32,
                conversion_rate: row.conversion_rate,
                mean_value: row.mean_value,
                p95_value: row.p95_value,
                time_bucket: row.time_bucket,
                confidence_interval: None,
            })
            .collect())
    }

    /// Experiment summary grouped by metric and variant
    async fn experiment_results(
        &self,
        ctx: &Context<'_>,
        feature_key: String,
        #[graphql(desc = "Metric keys to include")] metric_keys: Vec<String>,
        #[graphql(desc = "Optional environment to scope results")] environment_id: Option<ID>,
        #[graphql(desc = "Time window for aggregation")] time_period: Option<
            crate::graphql::subscription::TimePeriod,
        >,
    ) -> GqlResult<ExperimentAnalysis> {
        if metric_keys.is_empty() {
            return Err(async_graphql::Error::new(
                "metricKeys must include at least one entry",
            ));
        }

        let env_uuid = match environment_id {
            Some(id) => Some(Uuid::parse_str(&id.to_string()).map_err(|e| {
                async_graphql::Error::new(format!("Invalid environment id: {}", e))
            })?),
            None => None,
        };

        let logic = ctx.data::<Box<dyn MetricLogic>>()?;
        let period = time_period.unwrap_or(crate::graphql::subscription::TimePeriod::D7);
        let (from, to) = calculate_time_range(period, Utc::now());

        let rows = logic
            .get_metric_results(&feature_key, env_uuid, from, to)
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to load metric results: {}", e))
            })?;

        let requested: HashSet<String> = metric_keys.iter().cloned().collect();
        let mut aggregated: HashMap<
            String,
            HashMap<Option<String>, (i64, i64, f64, Option<f64>, DateTime<Utc>)>,
        > = HashMap::new();

        for row in rows
            .into_iter()
            .filter(|r| requested.contains(&r.metric_key))
        {
            let metric_entry = aggregated
                .entry(row.metric_key.clone())
                .or_insert_with(HashMap::new);
            let entry = metric_entry.entry(row.variant.clone()).or_insert((
                0,
                0,
                0.0,
                None,
                row.time_bucket,
            ));

            entry.0 += row.sample_size;
            entry.1 += row.conversion_count.unwrap_or(0);
            entry.2 += row.sum_value.unwrap_or(0.0);
            if row.p95_value.is_some() {
                entry.3 = row.p95_value;
            }
            if row.time_bucket > entry.4 {
                entry.4 = row.time_bucket;
            }
        }

        let mut analyses = Vec::new();
        for key in metric_keys {
            let mut results = Vec::new();
            if let Some(variants) = aggregated.get(&key) {
                for (variant, (sample_size, conversion_count, sum_value, p95_value, time_bucket)) in
                    variants.iter()
                {
                    let sample_size_i32 = std::cmp::min(*sample_size, i32::MAX as i64) as i32;
                    let conversion_rate = if *sample_size > 0 {
                        Some(*conversion_count as f64 / *sample_size as f64)
                    } else {
                        None
                    };
                    let mean_value = if *sample_size > 0 {
                        Some(*sum_value / *sample_size as f64)
                    } else {
                        None
                    };

                    results.push(MetricResult {
                        metric_key: key.clone(),
                        variant: variant.clone(),
                        sample_size: sample_size_i32,
                        conversion_rate,
                        mean_value,
                        p95_value: *p95_value,
                        time_bucket: *time_bucket,
                        confidence_interval: None,
                    });
                }
            }

            // Simple heuristic winner: highest conversion_rate, else highest mean_value
            let mut winner: Option<String> = None;
            let mut best_score = f64::MIN;
            for r in &results {
                let score = r.conversion_rate.or(r.mean_value).unwrap_or(f64::MIN / 2.0);
                if score > best_score {
                    best_score = score;
                    winner = r.variant.clone();
                }
            }

            analyses.push(MetricAnalysis {
                metric_key: key,
                results,
                winner,
                statistical_significance: None,
            });
        }

        Ok(ExperimentAnalysis {
            feature_key,
            metrics: analyses,
        })
    }

    /// Check JWT secret status (admin only)
    async fn jwt_secret_status(&self, ctx: &Context<'_>) -> GqlResult<Vec<JwtSecretResponse>> {
        debug!("Checking JWT secret status");

        // Get user info from JWT context
        let jwt_user = ctx.data::<crate::JwtUser>()?;

        // Check if user is admin
        if !jwt_user.is_admin {
            return Err(async_graphql::Error::new(
                "Unauthorized: Admin access required",
            ));
        }

        let logic = ctx.data::<Box<dyn crate::logic::jwt_secret::JwtSecretLogic>>()?;
        let secrets = logic.get_all_secrets().await?;

        Ok(secrets
            .into_iter()
            .map(|secret| JwtSecretResponse {
                id: secret.id.into(),
                is_active: secret.is_active,
                created_at: secret.created_at,
                created_by: secret.created_by.map(|id| id.into()),
                expires_at: secret.expires_at,
                // Don't return the actual secret for security
                secret_preview: format!(
                    "{}...{}",
                    &secret.secret[..8],
                    &secret.secret[secret.secret.len() - 4..]
                ),
            })
            .collect())
    }
}

/// Helper function to resolve entity details based on entity type
async fn resolve_entity_details(
    entity_type: &str,
    entity_id: &str,
    metadata: &Option<serde_json::Value>,
    feature_repo: &dyn crate::database::feature::FeatureRepository,
    environment_logic: &dyn crate::logic::environment::EnvironmentLogic,
) -> Option<crate::graphql::schema::ActivityEntityDetails> {
    match entity_type {
        "stage" => {
            // For stages, try to get feature and environment details
            if let Ok(stage_uuid) = uuid::Uuid::parse_str(entity_id) {
                // Try to get feature ID from stage
                if let Ok(Some(feature_id)) =
                    feature_repo.get_feature_id_by_stage_id(stage_uuid).await
                {
                    // Get the feature to access stage details
                    if let Ok(feature) = feature_repo.get_feature_by_id(feature_id).await {
                        // Find the stage in the feature
                        if let Ok(stages) = feature_repo.get_feature_stages(feature_id).await {
                            if let Some(stage) = stages.iter().find(|s| s.id == stage_uuid) {
                                // Fetch environment details
                                let environment = environment_logic
                                    .get_environment_by_id(async_graphql::ID::from(
                                        stage.environment_id,
                                    ))
                                    .await
                                    .ok();

                                let environment_name = environment
                                    .as_ref()
                                    .map(|env| env.name.clone())
                                    .unwrap_or_else(|| format!("Stage ({})", stage.status));

                                // Build stage object with environment
                                let stage_details = serde_json::json!({
                                    "id": stage.id.to_string(),
                                    "status": stage.status,
                                    "order_index": stage.order_index,
                                    "position": stage.position,
                                    "bucketing_key": stage.bucketing_key,
                                    "environment": environment.as_ref().map(|env| serde_json::json!({
                                        "id": env.id.to_string(),
                                        "name": env.name,
                                        "active": env.active,
                                    }))
                                });

                                return Some(crate::graphql::schema::ActivityEntityDetails {
                                    id: entity_id.to_string(),
                                    name: format!("{} - {}", feature.key, environment_name),
                                    entity_type: entity_type.to_string(),
                                    details: Some(serde_json::json!({
                                        "feature_key": feature.key,
                                        "feature_id": feature_id.to_string(),
                                        "stage": stage_details,
                                    })),
                                });
                            }
                        }
                    }
                }
            }
            // Fallback: use metadata if available
            if let Some(meta) = metadata {
                if let (Some(feature_key), Some(status)) = (
                    meta.get("feature_key").and_then(|v| v.as_str()),
                    meta.get("status").and_then(|v| v.as_str()),
                ) {
                    return Some(crate::graphql::schema::ActivityEntityDetails {
                        id: entity_id.to_string(),
                        name: format!("{} ({})", feature_key, status),
                        entity_type: entity_type.to_string(),
                        details: Some(meta.clone()),
                    });
                }
            }
            None
        }
        "feature" => {
            // For features, get the feature name/key
            if let Ok(feature_uuid) = uuid::Uuid::parse_str(entity_id) {
                if let Ok(feature) = feature_repo.get_feature_by_id(feature_uuid).await {
                    return Some(crate::graphql::schema::ActivityEntityDetails {
                        id: entity_id.to_string(),
                        name: feature.key.clone(),
                        entity_type: entity_type.to_string(),
                        details: Some(serde_json::json!({
                            "feature_key": feature.key,
                            "feature_id": feature_uuid.to_string(),
                            "description": feature.description,
                        })),
                    });
                }
            }
            // Fallback: use metadata
            if let Some(meta) = metadata {
                if let Some(feature_key) = meta.get("feature_key").and_then(|v| v.as_str()) {
                    return Some(crate::graphql::schema::ActivityEntityDetails {
                        id: entity_id.to_string(),
                        name: feature_key.to_string(),
                        entity_type: entity_type.to_string(),
                        details: Some(meta.clone()),
                    });
                }
            }
            None
        }
        _ => {
            // For other entity types (user, team, client, etc.), just use entity_id as name
            // or extract from metadata
            let name = metadata
                .as_ref()
                .and_then(|m| m.get("name").or_else(|| m.get("key")))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| entity_id.to_string());

            Some(crate::graphql::schema::ActivityEntityDetails {
                id: entity_id.to_string(),
                name,
                entity_type: entity_type.to_string(),
                details: metadata.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::context::MockContextLogic;
    use async_graphql::{EmptySubscription, Request, Schema};
    #[tokio::test]
    async fn test_contexts_query() {
        let mut mock = MockContextLogic::new();
        let team_id = ID::from("11111111-1111-1111-1111-111111111111");
        let expected = vec![crate::graphql::schema::Context {
            id: ID::from("22222222-2222-2222-2222-222222222222"),
            team_id: team_id.clone(),
            key: "country".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from("33333333-3333-3333-3333-333333333333"),
                value: "US".into(),
            }],
        }];
        let team_id_clone = team_id.clone();
        mock.expect_get_contexts()
            .times(1)
            .withf(move |tid, key| tid == &team_id_clone && key.is_none())
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::context::ContextLogic>>(Box::new(mock))
        .finish();

        let gql = r#"
            query($team: ID!) {
                contexts(teamId: $team) { items { key entries { value } } }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "team": team_id.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["contexts"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(data["contexts"]["items"][0]["key"], "country");
    }

    #[tokio::test]
    async fn test_stage_contexts_query() {
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from("11111111-1111-1111-1111-111111111111");
        let expected = vec![crate::graphql::schema::Context {
            id: ID::from("22222222-2222-2222-2222-222222222222"),
            team_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
            key: "alpha".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from("33333333-3333-3333-3333-333333333333"),
                value: "X".into(),
            }],
        }];
        let stage_id_clone = stage_id.clone();
        mock.expect_get_stage_contexts()
            .times(1)
            .withf(move |sid| sid == &stage_id_clone)
            .return_once(move |_| Ok(expected.clone()));

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
        .finish();

        let gql = r#"
            query($sid: ID!) {
                stageContexts(stageId: $sid) { key entries { value } }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["stageContexts"].as_array().unwrap().len(), 1);
        assert_eq!(data["stageContexts"][0]["key"], "alpha");
    }
}

#[cfg(test)]
mod more_query_tests {
    use super::*;
    use async_graphql::{EmptySubscription, Request, Schema};

    // Use stub for PipelineLogic (no automock) and mock for EnvironmentLogic
    use crate::logic::environment::MockEnvironmentLogic;
    use std::sync::{Arc, Mutex};

    struct StubPipelineLogic {
        pub captured_fields: Arc<Mutex<Option<Vec<String>>>>,
    }
    #[async_trait::async_trait]
    impl crate::logic::pipeline::PipelineLogic for StubPipelineLogic {
        async fn get_pipelines(
            &self,
            _team_id: ID,
            _name: Option<String>,
            _active: Option<bool>,
            fields: Vec<String>,
        ) -> Result<Vec<Pipeline>, crate::Error> {
            *self.captured_fields.lock().unwrap() = Some(fields);
            Ok(Vec::new())
        }
        async fn get_pipelines_paginated(
            &self,
            _team_id: ID,
            _name: Option<String>,
            _active: Option<bool>,
            _fields: Vec<String>,
            _page_number: i32,
            _page_size: i32,
        ) -> Result<(Vec<Pipeline>, i64), crate::Error> {
            Ok((Vec::new(), 0))
        }
        async fn get_pipeline_by_id(&self, _id: ID) -> Result<Pipeline, crate::Error> {
            unreachable!()
        }
        async fn create_pipeline(
            &self,
            _team_id: ID,
            _input: crate::graphql::schema::CreatePipelineInput,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<ID, crate::Error> {
            unreachable!()
        }
        async fn update_pipeline(
            &self,
            _id: ID,
            _input: crate::graphql::schema::UpdatePipelineInput,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<Pipeline, crate::Error> {
            unreachable!()
        }
        async fn delete_pipeline(
            &self,
            _id: ID,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<(), crate::Error> {
            unreachable!()
        }
        fn clone_box(&self) -> Box<dyn crate::logic::pipeline::PipelineLogic> {
            Box::new(Self {
                captured_fields: self.captured_fields.clone(),
            })
        }
    }

    struct StubUserLogic {
        items: Vec<crate::logic::user::GqlUser>,
        total: i64,
    }
    #[async_trait::async_trait]
    impl crate::logic::user::UserLogic for StubUserLogic {
        async fn get_user_by_id(
            &self,
            _id: ID,
        ) -> Result<crate::logic::user::GqlUser, crate::Error> {
            unreachable!()
        }
        async fn get_user_by_username(
            &self,
            _username: String,
        ) -> Result<crate::logic::user::GqlUser, crate::Error> {
            unreachable!()
        }
        async fn register_user(
            &self,
            _input: crate::logic::user::RegisterUserInput,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<crate::logic::user::GqlUser, crate::Error> {
            unreachable!()
        }
        async fn authenticate_user(
            &self,
            _username: String,
            _password: String,
        ) -> Result<crate::logic::user::GqlUser, crate::Error> {
            unreachable!()
        }
        async fn update_user(
            &self,
            _id: ID,
            _input: crate::logic::user::UpdateGqlUserInput,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<crate::logic::user::GqlUser, crate::Error> {
            unreachable!()
        }
        async fn reset_password(
            &self,
            _id: ID,
            _current_password: String,
            _new_password: String,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<(), crate::Error> {
            unreachable!()
        }
        async fn set_temporary_password(
            &self,
            _user_id: ID,
            _temporary_password: String,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<(), crate::Error> {
            unreachable!()
        }
        async fn assign_user_teams(
            &self,
            _id: ID,
            _team_ids: Vec<ID>,
            _actor: Option<crate::logic::ActorContext>,
        ) -> Result<bool, crate::Error> {
            unreachable!()
        }
        async fn get_user_teams(&self, _id: ID) -> Result<Vec<Team>, crate::Error> {
            unreachable!()
        }
        async fn search_users(
            &self,
            _team_id: Option<ID>,
            _name: Option<String>,
            _page_number: i32,
            _page_size: i32,
        ) -> Result<(Vec<crate::logic::user::GqlUser>, i64), crate::Error> {
            Ok((self.items.clone(), self.total))
        }
        async fn admin_exists(&self) -> Result<bool, crate::Error> {
            unreachable!()
        }
        fn clone_box(&self) -> Box<dyn crate::logic::user::UserLogic> {
            Box::new(Self {
                items: self.items.clone(),
                total: self.total,
            })
        }
    }

    #[tokio::test]
    async fn test_pipelines_lookahead_includes_stages_field() {
        let team_id = ID::from("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
        let captured = Arc::new(Mutex::new(None));
        let stub = StubPipelineLogic {
            captured_fields: captured.clone(),
        };

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::pipeline::PipelineLogic>>(Box::new(stub))
        .finish();

        let q = r#"query($tid: ID!){ pipelines(teamId: $tid){ items { id stages { id } } } }"#;
        let mut req = Request::new(q);
        req = req.variables(async_graphql::Variables::from_json(
            serde_json::json!({"tid": team_id.to_string()}),
        ));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let fields = captured.lock().unwrap().clone().unwrap();
        assert!(fields.contains(&"stages".to_string()));
    }

    #[tokio::test]
    async fn test_users_pagination_maps_items_and_total() {
        use chrono::Utc;
        let u1 = crate::logic::user::GqlUser {
            id: ID::from("11111111-1111-1111-1111-111111111111"),
            username: "u1".into(),
            first_name: "F1".into(),
            last_name: "L1".into(),
            email: "u1@example.com".into(),
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
            is_temporary_password: false,
        };
        let u2 = crate::logic::user::GqlUser {
            id: ID::from("22222222-2222-2222-2222-222222222222"),
            username: "u2".into(),
            first_name: "F2".into(),
            last_name: "L2".into(),
            email: "u2@example.com".into(),
            is_admin: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
            is_temporary_password: false,
        };
        let stub = StubUserLogic {
            items: vec![u1, u2],
            total: 42,
        };

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::user::UserLogic>>(Box::new(stub))
        .finish();

        let q = r#"query{ users(pageNumber: 2, pageSize: 10){ pageNumber pageSize total items { username isAdmin } } }"#;
        let resp = schema.execute(Request::new(q)).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["users"]["pageNumber"], 2);
        assert_eq!(data["users"]["pageSize"], 10);
        assert_eq!(data["users"]["total"], 42);
        let items = data["users"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["username"], "u1");
        assert_eq!(items[1]["isAdmin"], true);
    }

    #[tokio::test]
    async fn test_environment_query_calls_logic() {
        let mut mock = MockEnvironmentLogic::new();
        let env_id = ID::from("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
        let expected = Environment {
            id: env_id.clone(),
            name: "prod".into(),
            active: true,
            team_id: ID::from("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"),
        };
        let env_id_for_check = env_id.clone();
        mock.expect_get_environment_by_id()
            .times(1)
            .withf(move |id| id.to_string() == env_id_for_check.to_string())
            .return_once(move |_| Ok(expected));

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::environment::EnvironmentLogic>>(Box::new(mock))
        .finish();

        let q = r#"query($id: ID!){ environment(id: $id){ id name active } }"#;
        let mut req = Request::new(q);
        req = req.variables(async_graphql::Variables::from_json(
            serde_json::json!({"id": env_id.to_string()}),
        ));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_roles_query() {
        use crate::logic::role::MockRoleLogic;
        let mut mock = MockRoleLogic::new();
        let expected_roles = vec![
            crate::logic::role::GqlRole {
                id: ID::from(uuid::Uuid::new_v4()),
                name: "Approver".to_string(),
                description: "Can approve deployment requests".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
            crate::logic::role::GqlRole {
                id: ID::from(uuid::Uuid::new_v4()),
                name: "Requester".to_string(),
                description: "Can request deployments".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        ];

        mock.expect_get_all_roles()
            .times(1)
            .return_once(move || Ok(expected_roles));

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
        .finish();

        let gql = r#"
            query {
                roles {
                    id
                    name
                    description
                }
            }
        "#;
        let resp = schema.execute(Request::new(gql)).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["roles"].as_array().unwrap().len(), 2);
        assert_eq!(data["roles"][0]["name"], "Approver");
        assert_eq!(data["roles"][1]["name"], "Requester");
    }

    #[tokio::test]
    async fn test_teams_query_admin_user() {
        use crate::logic::team::TeamLogic;

        struct StubTeamLogic {
            teams: Vec<Team>,
        }

        #[async_trait::async_trait]
        impl TeamLogic for StubTeamLogic {
            async fn get_team_by_id(&self, _env_id: uuid::Uuid) -> Result<Team, crate::Error> {
                unreachable!()
            }
            async fn get_teams(&self, _name: Option<String>) -> Result<Vec<Team>, crate::Error> {
                Ok(self.teams.clone())
            }
            async fn create_team(
                &self,
                _input: crate::graphql::schema::CreateTeamInput,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<Team, crate::Error> {
                unreachable!()
            }
            async fn update_team(
                &self,
                _id: ID,
                _input: crate::graphql::schema::UpdateTeamInput,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<Team, crate::Error> {
                unreachable!()
            }
            async fn delete_team(&self, _id: uuid::Uuid) -> Result<(), crate::Error> {
                unreachable!()
            }
            fn clone_box(&self) -> Box<dyn TeamLogic> {
                Box::new(Self {
                    teams: self.teams.clone(),
                })
            }
        }

        let expected_teams = vec![
            Team {
                id: ID::from("team1"),
                name: "Team 1".to_string(),
                description: "First team".to_string(),
            },
            Team {
                id: ID::from("team2"),
                name: "Team 2".to_string(),
                description: "Second team".to_string(),
            },
        ];

        let jwt_user = crate::JwtUser {
            id: uuid::Uuid::new_v4(),
            username: "admin".to_string(),
            is_admin: true,
            roles: vec!["Admin".to_string()],
            token_hash: "hash123".to_string(),
        };

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn TeamLogic>>(Box::new(StubTeamLogic {
            teams: expected_teams.clone(),
        }))
        .data(jwt_user)
        .finish();

        let gql = r#"
            query {
                teams {
                    id
                    name
                    description
                }
            }
        "#;
        let resp = schema.execute(Request::new(gql)).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["teams"].as_array().unwrap().len(), 2);
        assert_eq!(data["teams"][0]["name"], "Team 1");
        assert_eq!(data["teams"][1]["name"], "Team 2");
    }

    #[tokio::test]
    async fn test_teams_query_regular_user() {
        use crate::logic::user::UserLogic;

        struct StubUserLogicForTeams {
            user_teams: Vec<Team>,
        }

        #[async_trait::async_trait]
        impl UserLogic for StubUserLogicForTeams {
            async fn get_user_by_id(
                &self,
                _id: ID,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn get_user_by_username(
                &self,
                _username: String,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn register_user(
                &self,
                _input: crate::logic::user::RegisterUserInput,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn authenticate_user(
                &self,
                _username: String,
                _password: String,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn update_user(
                &self,
                _id: ID,
                _input: crate::logic::user::UpdateGqlUserInput,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn reset_password(
                &self,
                _id: ID,
                _current_password: String,
                _new_password: String,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<(), crate::Error> {
                unreachable!()
            }
            async fn set_temporary_password(
                &self,
                _user_id: ID,
                _temporary_password: String,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<(), crate::Error> {
                unreachable!()
            }
            async fn assign_user_teams(
                &self,
                _id: ID,
                _team_ids: Vec<ID>,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<bool, crate::Error> {
                unreachable!()
            }
            async fn get_user_teams(&self, _id: ID) -> Result<Vec<Team>, crate::Error> {
                Ok(self.user_teams.clone())
            }
            async fn search_users(
                &self,
                _team_id: Option<ID>,
                _name: Option<String>,
                _page_number: i32,
                _page_size: i32,
            ) -> Result<(Vec<crate::logic::user::GqlUser>, i64), crate::Error> {
                unreachable!()
            }
            async fn admin_exists(&self) -> Result<bool, crate::Error> {
                unreachable!()
            }
            fn clone_box(&self) -> Box<dyn UserLogic> {
                Box::new(Self {
                    user_teams: self.user_teams.clone(),
                })
            }
        }

        let expected_teams = vec![Team {
            id: ID::from("team1"),
            name: "User Team".to_string(),
            description: "User's assigned team".to_string(),
        }];

        let jwt_user = crate::JwtUser {
            id: uuid::Uuid::new_v4(),
            username: "regular_user".to_string(),
            is_admin: false,
            roles: vec!["User".to_string()],
            token_hash: "hash123".to_string(),
        };

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn UserLogic>>(Box::new(StubUserLogicForTeams {
            user_teams: expected_teams.clone(),
        }))
        .data(jwt_user)
        .finish();

        let gql = r#"
            query {
                teams {
                    id
                    name
                    description
                }
            }
        "#;
        let resp = schema.execute(Request::new(gql)).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["teams"].as_array().unwrap().len(), 1);
        assert_eq!(data["teams"][0]["name"], "User Team");
    }

    #[tokio::test]
    async fn test_jwt_secret_status_query_admin_user() {
        use crate::database::entity::JwtSecret;
        use crate::logic::jwt_secret::MockJwtSecretLogic;
        use chrono::Utc;

        let mut mock = MockJwtSecretLogic::new();
        let expected_secrets = vec![
            JwtSecret {
                id: uuid::Uuid::new_v4(),
                secret: "test_secret_123456789012345678901234567890".to_string(),
                is_active: true,
                created_at: Utc::now(),
                created_by: Some(uuid::Uuid::new_v4()),
                expires_at: None,
            },
            JwtSecret {
                id: uuid::Uuid::new_v4(),
                secret: "old_secret_abcdefghijklmnopqrstuvwxyz1234".to_string(),
                is_active: false,
                created_at: Utc::now(),
                created_by: Some(uuid::Uuid::new_v4()),
                expires_at: None,
            },
        ];

        mock.expect_get_all_secrets()
            .times(1)
            .return_once(move || Ok(expected_secrets));

        let jwt_user = crate::JwtUser {
            id: uuid::Uuid::new_v4(),
            username: "admin".to_string(),
            is_admin: true,
            roles: vec!["Admin".to_string()],
            token_hash: "hash123".to_string(),
        };

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::jwt_secret::JwtSecretLogic>>(Box::new(mock))
        .data(jwt_user)
        .finish();

        let gql = r#"
            query {
                jwtSecretStatus {
                    id
                    isActive
                    createdAt
                    createdBy
                    expiresAt
                    secretPreview
                }
            }
        "#;
        let resp = schema.execute(Request::new(gql)).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        let secrets = data["jwtSecretStatus"].as_array().unwrap();
        assert_eq!(secrets.len(), 2);
        assert_eq!(secrets[0]["isActive"], true);
        assert_eq!(secrets[1]["isActive"], false);
        // Check that secret previews are truncated
        assert!(
            secrets[0]["secretPreview"]
                .as_str()
                .unwrap()
                .contains("test_sec...7890")
        );
        assert!(
            secrets[1]["secretPreview"]
                .as_str()
                .unwrap()
                .contains("old_secr...1234")
        );
    }

    #[tokio::test]
    async fn test_jwt_secret_status_query_non_admin_user() {
        let jwt_user = crate::JwtUser {
            id: uuid::Uuid::new_v4(),
            username: "regular_user".to_string(),
            is_admin: false,
            roles: vec!["User".to_string()],
            token_hash: "hash123".to_string(),
        };

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data(jwt_user)
        .finish();

        let gql = r#"
            query {
                jwtSecretStatus {
                    id
                    isActive
                }
            }
        "#;
        let resp = schema.execute(Request::new(gql)).await;
        assert!(!resp.errors.is_empty());
        assert!(
            resp.errors[0]
                .message
                .contains("Unauthorized: Admin access required")
        );
    }

    #[tokio::test]
    async fn test_application_status_query() {
        use crate::logic::user::UserLogic;

        struct MockUserLogicStatus {
            admin_exists: bool,
        }

        #[async_trait::async_trait]
        impl UserLogic for MockUserLogicStatus {
            async fn get_user_by_id(
                &self,
                _id: ID,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn get_user_by_username(
                &self,
                _username: String,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn register_user(
                &self,
                _input: crate::logic::user::RegisterUserInput,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn authenticate_user(
                &self,
                _username: String,
                _password: String,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn update_user(
                &self,
                _id: ID,
                _input: crate::logic::user::UpdateGqlUserInput,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<crate::logic::user::GqlUser, crate::Error> {
                unreachable!()
            }
            async fn reset_password(
                &self,
                _id: ID,
                _current_password: String,
                _new_password: String,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<(), crate::Error> {
                unreachable!()
            }
            async fn set_temporary_password(
                &self,
                _user_id: ID,
                _temporary_password: String,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<(), crate::Error> {
                unreachable!()
            }
            async fn assign_user_teams(
                &self,
                _id: ID,
                _team_ids: Vec<ID>,
                _actor: Option<crate::logic::ActorContext>,
            ) -> Result<bool, crate::Error> {
                unreachable!()
            }
            async fn get_user_teams(&self, _id: ID) -> Result<Vec<Team>, crate::Error> {
                unreachable!()
            }
            async fn search_users(
                &self,
                _team_id: Option<ID>,
                _name: Option<String>,
                _page_number: i32,
                _page_size: i32,
            ) -> Result<(Vec<crate::logic::user::GqlUser>, i64), crate::Error> {
                unreachable!()
            }
            async fn admin_exists(&self) -> Result<bool, crate::Error> {
                Ok(self.admin_exists)
            }
            fn clone_box(&self) -> Box<dyn UserLogic> {
                Box::new(MockUserLogicStatus {
                    admin_exists: self.admin_exists,
                })
            }
        }

        // Test with admin configured (true)
        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn UserLogic>>(Box::new(MockUserLogicStatus { admin_exists: true }))
        .finish();

        let query = r#"
            query {
                applicationStatus {
                    adminConfigured
                }
            }
        "#;

        let response = schema.execute(Request::new(query)).await;
        assert!(
            response.errors.is_empty(),
            "GraphQL errors: {:?}",
            response.errors
        );
        let data = response.data.into_json().unwrap();
        assert_eq!(data["applicationStatus"]["adminConfigured"], true);

        // Test with no admin configured (false)
        let schema_no_admin = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn UserLogic>>(Box::new(MockUserLogicStatus {
            admin_exists: false,
        }))
        .finish();

        let response_no_admin = schema_no_admin.execute(Request::new(query)).await;
        assert!(
            response_no_admin.errors.is_empty(),
            "GraphQL errors: {:?}",
            response_no_admin.errors
        );
        let data_no_admin = response_no_admin.data.into_json().unwrap();
        assert_eq!(data_no_admin["applicationStatus"]["adminConfigured"], false);
    }
}
