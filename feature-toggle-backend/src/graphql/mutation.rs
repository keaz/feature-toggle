use crate::graphql::create_user;
use crate::graphql::schema::{
    AssignUserRolesInput, CreateClientInput, CreateEnvironmentInput, CreateFeatureInput,
    CreatePipelineInput, CreateTeamInput, Environment, Feature, LoginInput as GqlLoginInput,
    LoginResponse, Pipeline, RegisterUserInput as GqlRegisterUserInput, Role, Team,
    UpdateClientInput, UpdateEnvironmentInput, UpdateFeatureInput, UpdatePipelineInput,
    UpdateTeamInput, UpdateUserInput as GqlUpdateUserInput, User,
};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::role::RoleLogic;
use crate::logic::team::TeamLogic;
use crate::logic::user::{RegisterUserInput, UpdateGqlUserInput, UserLogic};
use crate::middleware::admin_guard::AdminState;
use async_graphql::{Context, ID, Object, Result as GqlResult};
use log::info;

#[cfg(test)]
use uuid::Uuid;

#[derive(async_graphql::Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum StageChangeRequest {
    #[graphql(name = "DEPLOYMENT_REQUESTED")]
    DeploymentRequested,
    #[graphql(name = "DEPLOYMENT_REJECTED")]
    DeploymentRejected,
    #[graphql(name = "DEPLOYED")]
    Deployed,
    #[graphql(name = "ROLLBACK_REQUESTED")]
    RollbackRequested,
    #[graphql(name = "ROLLBACK_REJECTED")]
    RollbackRejected,
    #[graphql(name = "ROLLBACKED")]
    Rollbacked,
}

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_environment(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateEnvironmentInput,
    ) -> GqlResult<Environment> {
        info!("Creating environment with input: {input:?}");
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        Ok(logic.create_environment(team_id, input).await?)
    }

    async fn update_environment(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateEnvironmentInput,
    ) -> GqlResult<Environment> {
        info!("Updating environment with id: {id:?} and input: {input:?}");
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        Ok(logic.update_environment(id, input).await?)
    }

    async fn delete_environment(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting environment with id: {id:?}");
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        logic.delete_environment(id).await?;
        Ok(true)
    }

    async fn create_team(&self, ctx: &Context<'_>, input: CreateTeamInput) -> GqlResult<Team> {
        let logic = ctx.data::<Box<dyn TeamLogic>>()?;
        let team = logic.create_team(input).await?;
        Ok(team)
    }

    async fn update_team(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the Team")] id: ID,
        input: UpdateTeamInput,
    ) -> GqlResult<Team> {
        let logic = ctx.data::<Box<dyn TeamLogic>>()?;
        let team = logic.update_team(id, input).await?;
        Ok(team)
    }

    async fn create_pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Team id")] team_id: ID,
        input: CreatePipelineInput,
    ) -> GqlResult<ID> {
        info!("Creating pipeline with input: {input:?}");
        input.validate(Some(team_id.clone()), ctx).await?;
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipeline_id = logic.create_pipeline(team_id, input).await?;
        Ok(pipeline_id)
    }

    async fn update_pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the current pipeline")] id: ID,
        input: UpdatePipelineInput,
    ) -> GqlResult<Pipeline> {
        info!("Updating pipeline with input: {input:?}");
        input.validate(Some(id.clone()), ctx).await?;
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipeline = logic.update_pipeline(id, input).await?;
        Ok(pipeline)
    }

    async fn delete_pipeline(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting pipeline with id: {id:?}");
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        logic.delete_pipeline(id).await?;
        Ok(true)
    }

    async fn create_feature(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateFeatureInput,
    ) -> GqlResult<ID> {
        info!("Creating feature with input: {input:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature_id = logic.create_feature(team_id, input).await?;
        Ok(feature_id)
    }

    async fn update_feature(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateFeatureInput,
    ) -> GqlResult<Feature> {
        info!("Updating feature with input: {input:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature = logic.update_feature(id.clone(), input).await?;

        // After successful update, publish to gRPC streaming subscribers
        if let (Ok(pool), Ok(updates_tx)) = (
            ctx.data::<sqlx::PgPool>(),
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
        ) {
            // Try to load the updated feature from DB and broadcast an UPSERT
            let repo = crate::database::feature::feature_repository(pool.clone());
            if let Ok(fid) = uuid::Uuid::try_from(id.clone())
                && let Ok(db_feature) = repo.get_feature_by_id(fid).await
            {
                // Map db_feature -> pb::FeatureFull
                if let Ok(full) =
                    map_db_feature_to_full_for_broadcast(pool.clone(), db_feature).await
                {
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

        Ok(feature)
    }

    async fn delete_feature(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting feature with id: {id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        logic.delete_feature(id).await?;
        Ok(true)
    }

    async fn create_client(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateClientInput,
    ) -> GqlResult<crate::graphql::schema::Client> {
        info!("Creating client with input: {input:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.create_client(team_id, input).await?)
    }

    async fn update_client(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateClientInput,
    ) -> GqlResult<crate::graphql::schema::Client> {
        info!("Updating client with id: {id:?} and input: {input:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.update_client(id, input).await?)
    }

    async fn delete_client(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting client with id: {id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        logic.delete_client(id).await?;
        Ok(true)
    }

    // Context mutations
    async fn create_context(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: crate::graphql::schema::CreateContextInput,
    ) -> GqlResult<crate::graphql::schema::Context> {
        info!("Creating context with key: {}", input.key);
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        Ok(logic.create_context(team_id, input).await?)
    }

    async fn update_context(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: crate::graphql::schema::UpdateContextInput,
    ) -> GqlResult<crate::graphql::schema::Context> {
        info!("Updating context with id: {id:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        let updated = logic.update_context(id.clone(), input).await?;
        Ok(updated)
    }

    async fn delete_context(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting context with id: {id:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>()?;
        logic.delete_context(id).await?;
        Ok(true)
    }

    // Feature stage context bindings
    async fn set_stage_contexts(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
        #[graphql(desc = "List of context IDs to assign")] context_ids: Vec<ID>,
    ) -> GqlResult<Vec<crate::graphql::schema::Context>> {
        info!("Setting contexts for stage {stage_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        Ok(logic.set_stage_contexts(stage_id, context_ids).await?)
    }

    async fn set_stage_criteria(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
        #[graphql(desc = "Criteria to assign")] criteria: Vec<
            crate::graphql::schema::CreateStageCriterionInput,
        >,
    ) -> GqlResult<Vec<crate::graphql::schema::StageCriterion>> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let result = logic.set_stage_criteria(stage_id.clone(), criteria).await?;

        // After updating criterias for a stage, broadcast an UPSERT for the owning feature
        if let (Ok(pool), Ok(updates_tx), Ok(feature_logic)) = (
            ctx.data::<sqlx::PgPool>(),
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
            ctx.data::<Box<dyn crate::logic::feature::FeatureLogic>>(),
        ) {
            if let Ok(Some(feature_id)) = feature_logic
                .get_feature_id_by_stage_id(stage_id.clone())
                .await
            {
                let repo = crate::database::feature::feature_repository(pool.clone());
                if let Ok(db_feature) = repo.get_feature_by_id(feature_id).await
                    && let Ok(full) =
                        map_db_feature_to_full_for_broadcast(pool.clone(), db_feature).await
                {
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

        Ok(result)
    }

    // Deployment workflow: request stage change
    async fn request_stage_change(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
        #[graphql(desc = "Requested change type")] request: StageChangeRequest,
    ) -> GqlResult<Feature> {
        // Get user id from JWT user data (injected by JWT middleware)
        let jwt_user = ctx.data_opt::<crate::JwtUser>().cloned();
        let user =
            jwt_user.ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;

        // Convert request to string for authorization check
        let request_type = match request {
            StageChangeRequest::DeploymentRequested => "DEPLOYMENT_REQUESTED",
            StageChangeRequest::DeploymentRejected => "DEPLOYMENT_REJECTED",
            StageChangeRequest::Deployed => "DEPLOYED",
            StageChangeRequest::RollbackRequested => "ROLLBACK_REQUESTED",
            StageChangeRequest::RollbackRejected => "ROLLBACK_REJECTED",
            StageChangeRequest::Rollbacked => "ROLLBACKED",
        };

        // Check authorization based on user roles
        crate::logic::authorization::RoleAuthorizer::authorize_stage_change_request(
            &user.roles,
            request_type,
        )
        .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let logic = ctx.data::<Box<dyn crate::logic::feature::FeatureLogic>>()?;
        let req = match request {
            StageChangeRequest::DeploymentRequested => {
                crate::logic::feature::StageChangeRequestType::DeploymentRequested
            }
            StageChangeRequest::DeploymentRejected => {
                crate::logic::feature::StageChangeRequestType::DeploymentRejected
            }
            StageChangeRequest::Deployed => crate::logic::feature::StageChangeRequestType::Deployed,
            StageChangeRequest::RollbackRequested => {
                crate::logic::feature::StageChangeRequestType::RollbackRequested
            }
            StageChangeRequest::RollbackRejected => {
                crate::logic::feature::StageChangeRequestType::RollbackRejected
            }
            StageChangeRequest::Rollbacked => {
                crate::logic::feature::StageChangeRequestType::Rollbacked
            }
        };
        let feature = logic
            .request_stage_change(stage_id.clone(), req, user.id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(feature)
    }

    // User mutations
    async fn register_user(
        &self,
        ctx: &Context<'_>,
        input: GqlRegisterUserInput,
    ) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let created = logic
            .register_user(RegisterUserInput {
                username: input.username,
                password: input.password,
                first_name: input.first_name,
                last_name: input.last_name,
                email: input.email,
                is_admin: input.is_admin.unwrap_or(false),
            })
            .await?;

        // If an admin was created, flip the admin-exists cache so middleware stops redirecting.
        if created.is_admin
            && let Ok(state) = ctx.data::<AdminState>()
        {
            state.set_exists(true);
        }
        create_user(created)
    }

    async fn create_admin(
        &self,
        ctx: &Context<'_>,
        mut input: GqlRegisterUserInput,
    ) -> GqlResult<User> {
        input.is_admin = Some(true);
        self.register_user(ctx, input).await?
    }

    async fn login(&self, ctx: &Context<'_>, input: GqlLoginInput) -> GqlResult<LoginResponse> {
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let role_logic = ctx.data::<Box<dyn crate::logic::role::RoleLogic>>()?;
        let jwt_secret = ctx.data::<String>()?;
        let pool = ctx.data::<sqlx::PgPool>()?;
        let u = logic
            .authenticate_user(input.username, input.password)
            .await?;

        // Fetch user roles
        let user_id = uuid::Uuid::try_from(u.id.clone())
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        let roles = role_logic.get_user_roles(u.id.clone()).await?;
        let role_names: Vec<String> = roles.into_iter().map(|r| r.name).collect();

        // Generate JWT token
        let token = crate::middleware::jwt_guard::create_jwt_token(
            user_id,
            &u.username,
            u.is_admin,
            role_names,
            jwt_secret,
        )
        .map_err(|e| async_graphql::Error::new(format!("Failed to create token: {}", e)))?;

        // Store token hash in database
        let token_hash = crate::middleware::jwt_guard::hash_token(&token);
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(24);
        let token_repo = crate::database::jwt_token::jwt_token_repository(pool.clone());

        token_repo
            .store_token(user_id, token_hash, expires_at)
            .await
            .map_err(|e| async_graphql::Error::new(format!("Failed to store token: {}", e)))?;

        let user = create_user(u)?;
        Ok(LoginResponse { user, token })
    }

    async fn logout(&self, ctx: &Context<'_>) -> GqlResult<bool> {
        // Get JWT user from context (injected by middleware)
        let jwt_user = ctx.data_opt::<crate::JwtUser>().cloned();
        let user =
            jwt_user.ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;

        let pool = ctx.data::<sqlx::PgPool>()?;
        let token_repo = crate::database::jwt_token::jwt_token_repository(pool.clone());

        // Revoke all tokens for this user (logout from all devices)
        let revoked_count = token_repo
            .revoke_all_user_tokens(user.id)
            .await
            .map_err(|e| async_graphql::Error::new(format!("Failed to revoke tokens: {}", e)))?;

        info!(
            "Logged out user {} from {} devices",
            user.username, revoked_count
        );
        Ok(true)
    }

    async fn logout_current(&self, ctx: &Context<'_>) -> GqlResult<bool> {
        // Get JWT user from context (injected by middleware)
        let jwt_user = ctx.data_opt::<crate::JwtUser>().cloned();
        let user =
            jwt_user.ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;

        let pool = ctx.data::<sqlx::PgPool>()?;
        let token_repo = crate::database::jwt_token::jwt_token_repository(pool.clone());

        // Revoke the specific current token using the hash from JWT user data
        let revoked = token_repo
            .revoke_token(&user.token_hash)
            .await
            .map_err(|e| async_graphql::Error::new(format!("Failed to revoke token: {}", e)))?;

        if revoked {
            info!("Logged out user {} from current device", user.username);
        } else {
            info!(
                "Token for user {} was already revoked or not found",
                user.username
            );
        }

        Ok(true)
    }

    async fn update_user(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: GqlUpdateUserInput,
    ) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let u = logic
            .update_user(
                id,
                UpdateGqlUserInput {
                    first_name: input.first_name,
                    last_name: input.last_name,
                    email: input.email,
                    is_admin: input.is_admin,
                    enabled: input.enabled,
                },
            )
            .await?;
        create_user(u)
    }

    async fn assign_user_teams(
        &self,
        ctx: &Context<'_>,
        user_id: ID,
        team_ids: Vec<ID>,
    ) -> GqlResult<Vec<Team>> {
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let _ = logic.assign_user_teams(user_id.clone(), team_ids).await?;
        // Fetch assigned teams to return
        let pool = ctx.data::<sqlx::PgPool>()?;
        let uid =
            uuid::Uuid::try_from(user_id).map_err(|e| async_graphql::Error::new(e.to_string()))?;
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

    async fn assign_user_roles(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "User ID to assign roles to")] user_id: ID,
        input: AssignUserRolesInput,
    ) -> GqlResult<Vec<Role>> {
        info!("Assigning roles to user: {user_id:?}");

        // Get user info from JWT context for assigned_by field
        let jwt_user = ctx.data_opt::<crate::JwtUser>().cloned();
        let assigned_by = jwt_user.map(|u| ID::from(u.id));

        let logic = ctx.data::<Box<dyn RoleLogic>>()?;
        let roles = logic
            .assign_user_roles(user_id, input.role_ids, assigned_by)
            .await?;

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

async fn map_db_feature_to_full_for_broadcast(
    pool: sqlx::PgPool,
    f: crate::database::entity::Feature,
) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
    use crate::grpc::pb;
    let repo = crate::database::feature::feature_repository(pool.clone());

    // stages with criterias
    let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(f.stages.len());
    for s in f.stages.iter() {
        let crits = repo.get_stage_criteria(s.id).await?;
        let criterias = crits
            .into_iter()
            .map(|c| pb::StageCriterionFull {
                id: c.id.to_string(),
                context_key: c.context_key,
                context: Some(pb::CriterionContext {
                    key: c.context.key,
                    entries: c.context.entries.into_iter().map(|e| e.value).collect(),
                }),
                rollout_percentage: c.rollout_percentage,
            })
            .collect::<Vec<_>>();

        stage_msgs.push(pb::FeatureStageFull {
            id: s.id.to_string(),
            environment_id: s.environment_id.to_string(),
            order_index: s.order_index,
            position: s.position.clone(),
            enabled: s.enabled,
            bucketing_key: s.bucketing_key.clone().unwrap_or_default(),
            criterias,
        });
    }

    let deps = f
        .dependencies
        .iter()
        .map(|d| pb::FeatureDependencyFull {
            feature_id: d.feature_id.to_string(),
            depends_on_id: d.depends_on_id.to_string(),
        })
        .collect::<Vec<_>>();

    let feature = pb::FeatureFull {
        id: f.id.to_string(),
        key: f.key,
        description: f.description.unwrap_or_default(),
        feature_type: format!("{:?}", f.feature_type),
        team_id: f.team_id.to_string(),
        created_at: f.created_at.to_rfc3339(),
        stages: stage_msgs,
        dependencies: deps,
    };
    Ok(feature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::query::Query as GqlQuery;
    use crate::logic::context::MockContextLogic;
    use async_graphql::{EmptySubscription, Request, Schema};

    #[tokio::test]
    async fn test_create_context_mutation() {
        let mut mock = MockContextLogic::new();
        let team_id = ID::from(Uuid::new_v4());
        let input = crate::graphql::schema::CreateContextInput {
            key: "country".into(),
            entries: vec!["US".into()],
        };
        let expected = crate::graphql::schema::Context {
            id: ID::from(Uuid::new_v4()),
            team_id: team_id.clone(),
            key: "country".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from(Uuid::new_v4()),
                value: "US".into(),
            }],
        };

        let team_id_clone = team_id.clone();
        mock.expect_create_context()
            .times(1)
            .withf(move |tid, i| {
                tid == &team_id_clone && i.key == "country" && i.entries.len() == 1
            })
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::context::ContextLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($team: ID!, $key: String!, $entries: [String!]!) {
                createContext(teamId: $team, input: { key: $key, entries: $entries }) {
                    key
                    entries { value }
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "team": team_id.to_string(),
            "key": "country",
            "entries": ["US"]
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createContext"]["key"], "country");
        assert_eq!(
            data["createContext"]["entries"].as_array().unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn test_set_stage_contexts_mutation() {
        use crate::graphql::query::Query as GqlQuery;
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        let ctx1 = ID::from(Uuid::new_v4());
        let ctx2 = ID::from(Uuid::new_v4());
        let expected = vec![
            crate::graphql::schema::Context {
                id: ctx1.clone(),
                team_id: ID::from(Uuid::new_v4()),
                key: "k1".into(),
                entries: vec![],
            },
            crate::graphql::schema::Context {
                id: ctx2.clone(),
                team_id: ID::from(Uuid::new_v4()),
                key: "k2".into(),
                entries: vec![],
            },
        ];
        let stage_id_clone = stage_id.clone();
        let ids_for_match = vec![ctx1.clone(), ctx2.clone()];
        mock.expect_set_stage_contexts()
            .times(1)
            .withf(move |sid, ids| sid == &stage_id_clone && ids == &ids_for_match)
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($sid: ID!, $ids: [ID!]!) {
                setStageContexts(stageId: $sid, contextIds: $ids) { key }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string(),
            "ids": [ctx1.to_string(), ctx2.to_string()]
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["setStageContexts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_set_stage_criteria_mutation_and_validation() {
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        // success path
        let expected = vec![crate::graphql::schema::StageCriterion {
            id: ID::from(Uuid::new_v4()),
            stage_id: stage_id.clone(),
            context_key: "filter".into(),
            context: crate::graphql::schema::Context {
                id: ID::from(Uuid::new_v4()),
                team_id: ID::from(Uuid::new_v4()),
                key: "filter-alpha".into(),
                entries: vec![],
            },
            rollout_percentage: 75,
        }];
        let stage_id_clone = stage_id.clone();
        mock.expect_set_stage_criteria()
            .times(1)
            .withf(move |sid, crit| {
                sid == &stage_id_clone
                    && crit.len() == 1
                    && crit[0].context_key == "filter"
                    && crit[0].rollout_percentage == 75
            })
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .finish();

        // success
        let gql = r#"
            mutation($sid: ID!, $cid: ID!) {
                setStageCriteria(stageId: $sid, criteria: [{ contextKey: "filter", contextId: $cid, rolloutPercentage: 75 }]) { rolloutPercentage }
            }
        "#;
        let mut req = Request::new(gql);
        let cid = ID::from(Uuid::new_v4());
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string(),
            "cid": cid.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );

        // validation failure for rollout_percentage
        let gql_bad = r#"
            mutation($sid: ID!, $cid: ID!) {
                setStageCriteria(stageId: $sid, criteria: [{ contextKey: "filter", contextId: $cid, rolloutPercentage: -5 }]) { rolloutPercentage }
            }
        "#;
        let mut req_bad = Request::new(gql_bad);
        req_bad = req_bad.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string(),
            "cid": cid.to_string()
        })));
        let resp_bad = schema.execute(req_bad).await;
        assert!(!resp_bad.errors.is_empty());
    }
}

#[cfg(test)]
mod more_mutation_tests {
    use super::*;
    use crate::graphql::query::Query as GqlQuery;
    use crate::logic::context::MockContextLogic;
    use async_graphql::{EmptySubscription, Request, Schema};
    use uuid::Uuid;

    #[tokio::test]
    async fn test_update_context_mutation_calls_logic() {
        let mut mock = MockContextLogic::new();
        let ctx_id = ID::from(Uuid::new_v4());
        let expected = crate::graphql::schema::Context {
            id: ctx_id.clone(),
            team_id: ID::from(Uuid::new_v4()),
            key: "k".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from(Uuid::new_v4()),
                value: "A".into(),
            }],
        };
        let ctx_id_check = ctx_id.clone();
        mock.expect_update_context()
            .times(1)
            .withf(move |id, input| id == &ctx_id_check && input.key.as_deref() == Some("k2"))
            .return_once(move |_, _| Ok(expected));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::context::ContextLogic>>(Box::new(mock))
            .finish();

        let gql = r#"mutation($id: ID!){ updateContext(id: $id, input: { key: "k2" }) { key entries { value } } }"#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(
            serde_json::json!({"id": ctx_id.to_string()}),
        ));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["updateContext"]["key"], "k");
    }

    #[tokio::test]
    async fn test_delete_context_mutation_returns_true() {
        let mut mock = MockContextLogic::new();
        let ctx_id = ID::from(Uuid::new_v4());
        let ctx_id_check = ctx_id.clone();
        mock.expect_delete_context()
            .times(1)
            .withf(move |id| id == &ctx_id_check)
            .return_once(|_| Ok(()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::context::ContextLogic>>(Box::new(mock))
            .finish();

        let gql = r#"mutation($id: ID!){ deleteContext(id: $id) }"#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(
            serde_json::json!({"id": ctx_id.to_string()}),
        ));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["deleteContext"], true);
    }

    #[tokio::test]
    async fn test_assign_user_roles_mutation() {
        use crate::logic::role::MockRoleLogic;
        let mut mock = MockRoleLogic::new();
        let user_id = ID::from(Uuid::new_v4());
        let role_id = ID::from(Uuid::new_v4());

        // Mock the assign operation to return assigned roles
        let expected_role = crate::logic::role::GqlRole {
            id: role_id.clone(),
            name: "Approver".to_string(),
            description: "Can approve deployment requests".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        mock.expect_assign_user_roles()
            .times(1)
            .return_once(move |_, _, _| Ok(vec![expected_role]));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($userId: ID!, $roleIds: [ID!]!) {
                assignUserRoles(userId: $userId, input: { roleIds: $roleIds }) {
                    id
                    name
                    description
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "userId": user_id.to_string(),
            "roleIds": [role_id.to_string()]
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["assignUserRoles"][0]["name"], "Approver");
    }

    #[tokio::test]
    async fn test_request_stage_change_with_requester_role_allows_deployment_request() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        // Mock the expected feature to be returned
        let expected_feature = create_mock_feature();
        mock.expect_request_stage_change()
            .times(1)
            .returning(move |_, _, _| Ok(expected_feature.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .data(crate::JwtUser {
                id: user_id,
                username: "requester_user".to_string(),
                is_admin: false,
                roles: vec!["Requester".to_string()],
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($stageId: ID!, $request: StageChangeRequest!) {
                requestStageChange(stageId: $stageId, request: $request) {
                    id
                    key
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYMENT_REQUESTED"
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {}",
            serde_json::to_string(&resp.errors).unwrap()
        );
    }

    #[tokio::test]
    async fn test_request_stage_change_without_requester_role_denies_deployment_request() {
        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data(crate::JwtUser {
                id: Uuid::new_v4(),
                username: "non_requester_user".to_string(),
                is_admin: false,
                roles: vec!["Team Admin".to_string()], // No Requester role
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($stageId: ID!, $request: StageChangeRequest!) {
                requestStageChange(stageId: $stageId, request: $request) {
                    id
                    key
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": Uuid::new_v4().to_string(),
            "request": "DEPLOYMENT_REQUESTED"
        })));

        let resp = schema.execute(req).await;
        assert!(!resp.errors.is_empty(), "Expected authorization error");
        assert!(
            resp.errors[0]
                .message
                .contains("Only users with 'Requester' role")
        );
    }

    #[tokio::test]
    async fn test_request_stage_change_with_approver_role_allows_deployment_approval() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        let expected_feature = create_mock_feature();
        mock.expect_request_stage_change()
            .times(1)
            .returning(move |_, _, _| Ok(expected_feature.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .data(crate::JwtUser {
                id: user_id,
                username: "approver_user".to_string(),
                is_admin: false,
                roles: vec!["Approver".to_string()],
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($stageId: ID!, $request: StageChangeRequest!) {
                requestStageChange(stageId: $stageId, request: $request) {
                    id
                    key
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYED"
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {}",
            serde_json::to_string(&resp.errors).unwrap()
        );
    }

    #[tokio::test]
    async fn test_request_stage_change_without_approver_role_denies_deployment_approval() {
        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data(crate::JwtUser {
                id: Uuid::new_v4(),
                username: "non_approver_user".to_string(),
                is_admin: false,
                roles: vec!["Requester".to_string()], // No Approver role
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($stageId: ID!, $request: StageChangeRequest!) {
                requestStageChange(stageId: $stageId, request: $request) {
                    id
                    key
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": Uuid::new_v4().to_string(),
            "request": "DEPLOYED"
        })));

        let resp = schema.execute(req).await;
        assert!(!resp.errors.is_empty(), "Expected authorization error");
        assert!(
            resp.errors[0]
                .message
                .contains("Only users with 'Approver' role")
        );
    }

    #[tokio::test]
    async fn test_request_stage_change_with_both_roles_allows_all_operations() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        let expected_feature = create_mock_feature();
        mock.expect_request_stage_change()
            .times(2) // We'll test two operations
            .returning(move |_, _, _| Ok(expected_feature.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .data(crate::JwtUser {
                id: user_id,
                username: "both_roles_user".to_string(),
                is_admin: false,
                roles: vec!["Requester".to_string(), "Approver".to_string()],
                token_hash: "hash".to_string(),
            })
            .finish();

        // Test requester operation
        let gql = r#"
            mutation($stageId: ID!, $request: StageChangeRequest!) {
                requestStageChange(stageId: $stageId, request: $request) {
                    id
                    key
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYMENT_REQUESTED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "Requester operation should succeed");

        // Test approver operation
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "Approver operation should succeed");
    }

    // Helper function to create a mock feature for testing
    fn create_mock_feature() -> crate::graphql::schema::Feature {
        crate::graphql::schema::Feature {
            id: async_graphql::ID::from(Uuid::new_v4().to_string()),
            key: "test_feature".to_string(),
            description: Some("Test description".to_string()),
            feature_type: crate::graphql::schema::FeatureType::Simple,
            enabled: Some(true),
            dependencies: vec![],
            relationships: vec![],
            stages: vec![],
            team_id: async_graphql::ID::from(Uuid::new_v4().to_string()),
        }
    }
}
