use std::sync::Arc;

use crate::database::feature::FeatureRepository;
use crate::graphql::create_user;
use crate::graphql::schema::map_approval_request;
use crate::graphql::schema::{
    ApprovalPolicy, ApprovalRequest, AssignUserRolesInput,
    CreateApprovalPolicyInput as GqlCreateApprovalPolicyInput, CreateClientInput,
    CreateEnvironmentInput, CreateFeatureInput, CreateMetricInput, CreatePipelineInput,
    CreateRoleInput, CreateTeamInput, CreateVariantAllocationInput, Environment, Feature,
    LoginInput as GqlLoginInput, LoginResponse, Metric, Pipeline,
    RegisterUserInput as GqlRegisterUserInput, ResetPasswordInput as GqlResetPasswordInput, Role,
    SetTemporaryPasswordInput as GqlSetTemporaryPasswordInput, Team,
    UpdateApprovalPolicyInput as GqlUpdateApprovalPolicyInput, UpdateClientInput,
    UpdateEnvironmentInput, UpdateFeatureInput, UpdatePipelineInput, UpdateTeamInput,
    UpdateUserInput as GqlUpdateUserInput, UpdateVariantAllocationInput, User, VariantAllocation,
    map_approval_policy,
};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::approval::ApprovalLogic;
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::metrics::MetricLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::role::RoleLogic;
use crate::logic::team::TeamLogic;
use crate::logic::user::{RegisterUserInput, UpdateGqlUserInput, UserLogic};
use crate::middleware::admin_guard::AdminState;
use async_graphql::{Context, ID, Object, Result as GqlResult};
use log::info;

#[cfg(test)]
use chrono::Utc;
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
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        Ok(logic.create_environment(team_id, input, actor).await?)
    }

    async fn update_environment(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateEnvironmentInput,
    ) -> GqlResult<Environment> {
        info!("Updating environment with id: {id:?} and input: {input:?}");
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        Ok(logic.update_environment(id, input, actor).await?)
    }

    async fn delete_environment(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting environment with id: {id:?}");
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        logic.delete_environment(id, actor).await?;
        Ok(true)
    }

    async fn create_team(&self, ctx: &Context<'_>, input: CreateTeamInput) -> GqlResult<Team> {
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn TeamLogic>>()?;
        let team = logic.create_team(input, actor).await?;
        Ok(team)
    }

    async fn update_team(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the Team")] id: ID,
        input: UpdateTeamInput,
    ) -> GqlResult<Team> {
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn TeamLogic>>()?;
        let team = logic.update_team(id, input, actor).await?;
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
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipeline_id = logic.create_pipeline(team_id, input, actor).await?;
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
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipeline = logic.update_pipeline(id, input, actor).await?;
        Ok(pipeline)
    }

    async fn create_metric(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateMetricInput,
    ) -> GqlResult<Metric> {
        let logic = ctx.data::<Box<dyn MetricLogic>>()?;
        let team_uuid = uuid::Uuid::parse_str(&team_id.to_string())
            .map_err(|e| async_graphql::Error::new(format!("Invalid team id: {e}")))?;

        let success_criteria = input.success_criteria.as_ref().map(|json| json.0.clone());

        let created = logic
            .create_metric(
                team_uuid,
                input.key,
                input.name,
                input.description,
                input.metric_type,
                input.unit,
                success_criteria,
            )
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(Metric {
            id: ID::from(created.id.to_string()),
            key: created.key,
            name: created.name,
            description: created.description,
            metric_type: created.metric_type,
            unit: created.unit,
        })
    }

    async fn delete_pipeline(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting pipeline with id: {id:?}");
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        logic.delete_pipeline(id, actor).await?;
        Ok(true)
    }

    async fn create_feature(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateFeatureInput,
    ) -> GqlResult<ID> {
        info!("Creating feature with input: {input:?}");

        // Validate that variants are only provided for Contextual features
        use crate::graphql::schema::FeatureType;
        if input.feature_type == FeatureType::Simple {
            if let Some(ref variants) = input.variants {
                if !variants.is_empty() {
                    return Err(async_graphql::Error::new(
                        "Variants can only be defined for Contextual features, not Simple features",
                    ));
                }
            }
        }

        // Extract actor information from JWT token
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });

        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature_id = logic.create_feature(team_id, input, actor).await?;
        Ok(feature_id)
    }

    async fn update_feature(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateFeatureInput,
    ) -> GqlResult<Feature> {
        info!("Updating feature with input: {input:?}");

        // Validate that variants are only provided for Contextual features
        use crate::graphql::schema::FeatureType;
        if input.feature_type == FeatureType::Simple {
            if let Some(ref variants) = input.variants {
                if !variants.is_empty() {
                    return Err(async_graphql::Error::new(
                        "Variants can only be defined for Contextual features, not Simple features",
                    ));
                }
            }
        }

        // Extract actor information from JWT token
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });

        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature = logic.update_feature(id.clone(), input, actor).await?;

        // After successful update, publish to gRPC streaming subscribers
        if let Ok(updates_tx) =
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>()
        {
            // Try to load the updated feature from DB and broadcast an UPSERT
            let feature_repository = ctx.data::<Arc<Box<dyn FeatureRepository>>>()?;
            if let Ok(fid) = uuid::Uuid::try_from(id.clone())
                && let Ok(db_feature) = feature_repository.get_feature_by_id(fid).await
            {
                // Map db_feature -> pb::FeatureFull
                if let Ok(full) =
                    map_db_feature_to_full_for_broadcast(&**feature_repository.as_ref(), db_feature)
                        .await
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

        // Extract actor information from JWT token
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });

        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        logic.delete_feature(id, actor).await?;
        Ok(true)
    }

    // Kill switch functionality for emergency feature disable/enable
    async fn emergency_disable_feature(
        &self,
        ctx: &Context<'_>,
        id: ID,
        rollback_in_minutes: Option<i32>,
    ) -> GqlResult<Feature> {
        info!(
            "Emergency disabling feature with id: {id:?}, rollback_in_minutes: {rollback_in_minutes:?}"
        );

        // Extract actor information from JWT token
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });

        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature = logic
            .emergency_disable_feature(id.clone(), rollback_in_minutes, actor)
            .await?;

        // Broadcast feature update for gRPC clients
        if let (Ok(pool), Ok(updates_tx)) = (
            ctx.data::<sqlx::PgPool>(),
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
        ) {
            // Try to load the updated feature from DB and broadcast an UPSERT
            let feature_repository = ctx.data::<Arc<Box<dyn FeatureRepository>>>()?;
            if let Ok(fid) = uuid::Uuid::try_from(id.clone())
                && let Ok(db_feature) = feature_repository.get_feature_by_id(fid).await
            {
                // Map db_feature -> pb::FeatureFull
                if let Ok(full) =
                    map_db_feature_to_full_for_broadcast(&**feature_repository.as_ref(), db_feature)
                        .await
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

    async fn emergency_enable_feature(&self, ctx: &Context<'_>, id: ID) -> GqlResult<Feature> {
        info!("Emergency enabling feature with id: {id:?}");

        // Extract actor information from JWT token
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });

        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature = logic.emergency_enable_feature(id.clone(), actor).await?;

        // Broadcast feature update for gRPC clients
        if let (Ok(pool), Ok(updates_tx)) = (
            ctx.data::<sqlx::PgPool>(),
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
        ) {
            // Try to load the updated feature from DB and broadcast an UPSERT
            let feature_repository =
                ctx.data::<Box<dyn crate::database::feature::FeatureRepository>>()?;

            if let Ok(fid) = uuid::Uuid::try_from(id.clone())
                && let Ok(db_feature) = feature_repository.get_feature_by_id(fid).await
            {
                // Map db_feature -> pb::FeatureFull
                if let Ok(full) =
                    map_db_feature_to_full_for_broadcast(&**feature_repository, db_feature).await
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

    async fn create_client(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateClientInput,
    ) -> GqlResult<crate::graphql::schema::Client> {
        info!("Creating client with input: {input:?}");
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.create_client(team_id, input, actor).await?)
    }

    async fn update_client(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateClientInput,
    ) -> GqlResult<crate::graphql::schema::Client> {
        info!("Updating client with id: {id:?} and input: {input:?}");
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.update_client(id, input, actor).await?)
    }

    async fn delete_client(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting client with id: {id:?}");
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        logic.delete_client(id, actor).await?;
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
        let feature_repository =
            ctx.data::<Box<dyn crate::database::feature::FeatureRepository>>()?;
        let criteria_with_details = criteria.clone();
        let mut result = logic
            .set_stage_criteria(stage_id.clone(), criteria_with_details.clone())
            .await?;

        // Inline handling of variant allocations and rule groups (best-effort if pool is available)
        if let Ok(pool) = ctx.data::<sqlx::PgPool>() {
            let variant_repo =
                crate::database::variant_allocations::variant_allocations_repository(pool.clone());
            let rules_repo =
                crate::database::compound_rules::compound_rules_repository(pool.clone());

            // Align created criteria with provided inputs by priority
            let mut created_sorted = result.clone();
            created_sorted.sort_by_key(|c| c.priority);
            let mut input_sorted = criteria_with_details.clone();
            input_sorted.sort_by_key(|c| c.priority);

            for (created, provided) in created_sorted.iter().zip(input_sorted.iter()) {
                let crit_uuid = uuid::Uuid::try_from(created.id.clone()).map_err(|e| {
                    async_graphql::Error::new(format!("Invalid criterion id: {}", e))
                })?;

                if let Some(allocs) = provided.variant_allocations.as_ref() {
                    if !allocs.is_empty() {
                        let db_allocs: Vec<
                            crate::database::variant_allocations::CreateVariantAllocationInput,
                        > = allocs
                            .iter()
                            .map(|a| {
                                crate::database::variant_allocations::CreateVariantAllocationInput {
                                    criteria_id: crit_uuid,
                                    variant_control: a.variant_control.clone(),
                                    weight: a.weight,
                                }
                            })
                            .collect();

                        variant_repo
                            .set_allocations(crit_uuid, db_allocs)
                            .await
                            .map_err(|e| {
                                async_graphql::Error::new(format!(
                                    "Failed to set variant allocations: {}",
                                    e
                                ))
                            })?;
                    }
                }

                if let Some(groups) = provided.rule_groups.as_ref() {
                    for group in groups {
                        let db_input = crate::database::compound_rules::CreateRuleGroupInput {
                            criteria_id: crit_uuid,
                            logic_operator: match group.logic_operator {
                                crate::graphql::schema::LogicOperator::And => {
                                    crate::database::entity::LogicOperator::And
                                }
                                crate::graphql::schema::LogicOperator::Or => {
                                    crate::database::entity::LogicOperator::Or
                                }
                            },
                            conditions: group
                                .conditions
                                .iter()
                                .map(|c| {
                                    crate::database::compound_rules::CreateRuleConditionInput {
                                        context_key: c.context_key.clone(),
                                        operator: c.operator.to_db_string(),
                                        value: c.value.0.clone(),
                                        order_index: c.order_index,
                                    }
                                })
                                .collect(),
                        };

                        rules_repo.create_rule_group(db_input).await.map_err(|e| {
                            async_graphql::Error::new(format!("Failed to create rule group: {}", e))
                        })?;
                    }
                }
            }

            // Reload criteria so the response (and broadcast) includes persisted allocations/groups
            result = logic.get_stage_criteria(stage_id.clone()).await?;
        }

        // After updating criterias for a stage, broadcast an UPSERT for the owning feature
        if let (Ok(updates_tx), Ok(feature_logic)) = (
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
            ctx.data::<Box<dyn FeatureLogic>>(),
        ) && let Ok(Some(feature_id)) = feature_logic
            .get_feature_id_by_stage_id(stage_id.clone())
            .await
        {
            if let Ok(db_feature) = feature_repository.get_feature_by_id(feature_id).await
                && let Ok(full) =
                    map_db_feature_to_full_for_broadcast(&**feature_repository, db_feature).await
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

        Ok(result)
    }

    // Compound rules mutations
    async fn create_rule_group(
        &self,
        ctx: &Context<'_>,
        input: crate::graphql::schema::CreateRuleGroupInput,
    ) -> GqlResult<crate::graphql::schema::CompoundRuleGroup> {
        info!("Creating rule group for criteria: {:?}", input.criteria_id);

        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo = crate::database::compound_rules::compound_rules_repository(pool.clone());

        let criteria_id = uuid::Uuid::try_from(input.criteria_id.clone())
            .map_err(|e| async_graphql::Error::new(format!("Invalid criteria ID: {}", e)))?;

        let db_input = crate::database::compound_rules::CreateRuleGroupInput {
            criteria_id,
            logic_operator: match input.logic_operator {
                crate::graphql::schema::LogicOperator::And => {
                    crate::database::entity::LogicOperator::And
                }
                crate::graphql::schema::LogicOperator::Or => {
                    crate::database::entity::LogicOperator::Or
                }
            },
            conditions: input
                .conditions
                .into_iter()
                .map(
                    |c| crate::database::compound_rules::CreateRuleConditionInput {
                        context_key: c.context_key,
                        operator: c.operator.to_db_string(),
                        value: c.value.0,
                        order_index: c.order_index,
                    },
                )
                .collect(),
        };

        let rule_group = repo.create_rule_group(db_input).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to create rule group: {}", e))
        })?;

        // Get conditions for the created group
        let conditions = repo.get_rule_conditions(rule_group.id).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to get rule conditions: {}", e))
        })?;

        Ok(crate::graphql::schema::CompoundRuleGroup {
            id: ID::from(rule_group.id),
            logic_operator: match rule_group.logic_operator {
                crate::database::entity::LogicOperator::And => {
                    crate::graphql::schema::LogicOperator::And
                }
                crate::database::entity::LogicOperator::Or => {
                    crate::graphql::schema::LogicOperator::Or
                }
            },
            conditions: conditions
                .into_iter()
                .map(|c| {
                    let operator = match c.operator.to_uppercase().as_str() {
                        "EQUALS" => crate::graphql::schema::RuleOperator::Equals,
                        "NOTEQUALS" | "NOT_EQUALS" => {
                            crate::graphql::schema::RuleOperator::NotEquals
                        }
                        "GREATERTHAN" | "GREATER_THAN" => {
                            crate::graphql::schema::RuleOperator::GreaterThan
                        }
                        "LESSTHAN" | "LESS_THAN" => crate::graphql::schema::RuleOperator::LessThan,
                        "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => {
                            crate::graphql::schema::RuleOperator::GreaterThanOrEqual
                        }
                        "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => {
                            crate::graphql::schema::RuleOperator::LessThanOrEqual
                        }
                        "CONTAINS" => crate::graphql::schema::RuleOperator::Contains,
                        "STARTSWITH" | "STARTS_WITH" => {
                            crate::graphql::schema::RuleOperator::StartsWith
                        }
                        "ENDSWITH" | "ENDS_WITH" => crate::graphql::schema::RuleOperator::EndsWith,
                        "REGEX" => crate::graphql::schema::RuleOperator::Regex,
                        "IN" => crate::graphql::schema::RuleOperator::In,
                        "NOTIN" | "NOT_IN" => crate::graphql::schema::RuleOperator::NotIn,
                        "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => {
                            crate::graphql::schema::RuleOperator::SemverGreaterThan
                        }
                        "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => {
                            crate::graphql::schema::RuleOperator::SemverLessThan
                        }
                        _ => crate::graphql::schema::RuleOperator::In,
                    };
                    crate::graphql::schema::CompoundRuleCondition {
                        id: ID::from(c.id),
                        context_key: c.context_key,
                        operator,
                        value: async_graphql::Json(c.value),
                        order_index: c.order_index,
                    }
                })
                .collect(),
        })
    }

    async fn update_rule_group(
        &self,
        ctx: &Context<'_>,
        group_id: ID,
        input: crate::graphql::schema::UpdateRuleGroupInput,
    ) -> GqlResult<crate::graphql::schema::CompoundRuleGroup> {
        info!("Updating rule group: {:?}", group_id);

        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo = crate::database::compound_rules::compound_rules_repository(pool.clone());

        let group_uuid = uuid::Uuid::try_from(group_id.clone())
            .map_err(|e| async_graphql::Error::new(format!("Invalid group ID: {}", e)))?;

        let db_input = crate::database::compound_rules::UpdateRuleGroupInput {
            logic_operator: input.logic_operator.map(|op| match op {
                crate::graphql::schema::LogicOperator::And => {
                    crate::database::entity::LogicOperator::And
                }
                crate::graphql::schema::LogicOperator::Or => {
                    crate::database::entity::LogicOperator::Or
                }
            }),
            conditions: input.conditions.map(|conds| {
                conds
                    .into_iter()
                    .map(
                        |c| crate::database::compound_rules::CreateRuleConditionInput {
                            context_key: c.context_key,
                            operator: c.operator.to_db_string(),
                            value: c.value.0,
                            order_index: c.order_index,
                        },
                    )
                    .collect()
            }),
        };

        let rule_group = repo
            .update_rule_group(group_uuid, db_input)
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to update rule group: {}", e))
            })?;

        // Get conditions for the updated group
        let conditions = repo.get_rule_conditions(rule_group.id).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to get rule conditions: {}", e))
        })?;

        Ok(crate::graphql::schema::CompoundRuleGroup {
            id: ID::from(rule_group.id),
            logic_operator: match rule_group.logic_operator {
                crate::database::entity::LogicOperator::And => {
                    crate::graphql::schema::LogicOperator::And
                }
                crate::database::entity::LogicOperator::Or => {
                    crate::graphql::schema::LogicOperator::Or
                }
            },
            conditions: conditions
                .into_iter()
                .map(|c| {
                    let operator = match c.operator.to_uppercase().as_str() {
                        "EQUALS" => crate::graphql::schema::RuleOperator::Equals,
                        "NOTEQUALS" | "NOT_EQUALS" => {
                            crate::graphql::schema::RuleOperator::NotEquals
                        }
                        "GREATERTHAN" | "GREATER_THAN" => {
                            crate::graphql::schema::RuleOperator::GreaterThan
                        }
                        "LESSTHAN" | "LESS_THAN" => crate::graphql::schema::RuleOperator::LessThan,
                        "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => {
                            crate::graphql::schema::RuleOperator::GreaterThanOrEqual
                        }
                        "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => {
                            crate::graphql::schema::RuleOperator::LessThanOrEqual
                        }
                        "CONTAINS" => crate::graphql::schema::RuleOperator::Contains,
                        "STARTSWITH" | "STARTS_WITH" => {
                            crate::graphql::schema::RuleOperator::StartsWith
                        }
                        "ENDSWITH" | "ENDS_WITH" => crate::graphql::schema::RuleOperator::EndsWith,
                        "REGEX" => crate::graphql::schema::RuleOperator::Regex,
                        "IN" => crate::graphql::schema::RuleOperator::In,
                        "NOTIN" | "NOT_IN" => crate::graphql::schema::RuleOperator::NotIn,
                        "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => {
                            crate::graphql::schema::RuleOperator::SemverGreaterThan
                        }
                        "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => {
                            crate::graphql::schema::RuleOperator::SemverLessThan
                        }
                        _ => crate::graphql::schema::RuleOperator::In,
                    };
                    crate::graphql::schema::CompoundRuleCondition {
                        id: ID::from(c.id),
                        context_key: c.context_key,
                        operator,
                        value: async_graphql::Json(c.value),
                        order_index: c.order_index,
                    }
                })
                .collect(),
        })
    }

    async fn delete_rule_group(&self, ctx: &Context<'_>, group_id: ID) -> GqlResult<bool> {
        info!("Deleting rule group: {:?}", group_id);

        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo = crate::database::compound_rules::compound_rules_repository(pool.clone());

        let group_uuid = uuid::Uuid::try_from(group_id)
            .map_err(|e| async_graphql::Error::new(format!("Invalid group ID: {}", e)))?;

        repo.delete_rule_group(group_uuid).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to delete rule group: {}", e))
        })?;

        Ok(true)
    }

    // ========================
    // Variant Allocation Mutations (for weighted traffic splits)
    // ========================

    /// Set variant allocations for a criterion (replaces existing allocations atomically)
    /// This is the recommended way to update allocations to ensure consistency
    async fn set_variant_allocations(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "ID of the stage criterion")] criteria_id: ID,
        #[graphql(desc = "List of variant allocations (weights must sum to 100 or less)")]
        allocations: Vec<CreateVariantAllocationInput>,
    ) -> GqlResult<Vec<VariantAllocation>> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo =
            crate::database::variant_allocations::variant_allocations_repository(pool.clone());

        let criteria_uuid = uuid::Uuid::try_from(criteria_id.clone())
            .map_err(|e| async_graphql::Error::new(format!("Invalid criteria ID: {}", e)))?;

        // Validate that weights sum to 100 or less
        let total_weight: i32 = allocations.iter().map(|a| a.weight).sum();
        if total_weight > 100 {
            return Err(async_graphql::Error::new(format!(
                "Total weight exceeds 100: got {}",
                total_weight
            )));
        }

        // Convert to database input structs
        let db_allocations: Vec<
            crate::database::variant_allocations::CreateVariantAllocationInput,
        > = allocations
            .into_iter()
            .map(
                |input| crate::database::variant_allocations::CreateVariantAllocationInput {
                    criteria_id: criteria_uuid,
                    variant_control: input.variant_control,
                    weight: input.weight,
                },
            )
            .collect();

        let result = repo
            .set_allocations(criteria_uuid, db_allocations)
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to set variant allocations: {}", e))
            })?;

        // Convert to GraphQL types
        Ok(result
            .into_iter()
            .map(|alloc| VariantAllocation {
                id: alloc.id.to_string().into(),
                criteria_id: criteria_id.clone(),
                variant_control: alloc.variant_control,
                weight: alloc.weight,
            })
            .collect())
    }

    /// Create a single variant allocation
    async fn create_variant_allocation(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "ID of the stage criterion")] criteria_id: ID,
        #[graphql(desc = "Variant allocation data")] input: CreateVariantAllocationInput,
    ) -> GqlResult<VariantAllocation> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo =
            crate::database::variant_allocations::variant_allocations_repository(pool.clone());

        let criteria_uuid = uuid::Uuid::try_from(criteria_id.clone())
            .map_err(|e| async_graphql::Error::new(format!("Invalid criteria ID: {}", e)))?;

        let db_input = crate::database::variant_allocations::CreateVariantAllocationInput {
            criteria_id: criteria_uuid,
            variant_control: input.variant_control,
            weight: input.weight,
        };

        let alloc = repo.create_allocation(db_input).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to create variant allocation: {}", e))
        })?;

        Ok(VariantAllocation {
            id: alloc.id.to_string().into(),
            criteria_id,
            variant_control: alloc.variant_control,
            weight: alloc.weight,
        })
    }

    /// Update the weight of a variant allocation
    async fn update_variant_allocation(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "ID of the variant allocation")] allocation_id: ID,
        #[graphql(desc = "Updated allocation data")] input: UpdateVariantAllocationInput,
    ) -> GqlResult<VariantAllocation> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo =
            crate::database::variant_allocations::variant_allocations_repository(pool.clone());

        let alloc_uuid = uuid::Uuid::try_from(allocation_id)
            .map_err(|e| async_graphql::Error::new(format!("Invalid allocation ID: {}", e)))?;

        let db_input = crate::database::variant_allocations::UpdateVariantAllocationInput {
            weight: input.weight,
        };

        let alloc = repo
            .update_allocation(alloc_uuid, db_input)
            .await
            .map_err(|e| {
                async_graphql::Error::new(format!("Failed to update variant allocation: {}", e))
            })?;

        Ok(VariantAllocation {
            id: alloc.id.to_string().into(),
            criteria_id: alloc.criteria_id.to_string().into(),
            variant_control: alloc.variant_control,
            weight: alloc.weight,
        })
    }

    /// Delete a variant allocation
    async fn delete_variant_allocation(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "ID of the variant allocation to delete")] allocation_id: ID,
    ) -> GqlResult<bool> {
        let pool = ctx.data::<sqlx::PgPool>()?;
        let repo =
            crate::database::variant_allocations::variant_allocations_repository(pool.clone());

        let alloc_uuid = uuid::Uuid::try_from(allocation_id)
            .map_err(|e| async_graphql::Error::new(format!("Invalid allocation ID: {}", e)))?;

        repo.delete_allocation(alloc_uuid).await.map_err(|e| {
            async_graphql::Error::new(format!("Failed to delete variant allocation: {}", e))
        })?;

        Ok(true)
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

        // After successful stage change, publish to gRPC streaming subscribers
        if let (Ok(pool), Ok(updates_tx)) = (
            ctx.data::<sqlx::PgPool>(),
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
        ) {
            // Get the feature ID from the returned feature
            if let Ok(fid) = uuid::Uuid::try_from(feature.id.clone()) {
                let feature_repository = ctx.data::<Arc<Box<dyn FeatureRepository>>>()?;
                if let Ok(db_feature) = feature_repository.get_feature_by_id(fid).await {
                    // Map db_feature -> pb::FeatureFull
                    if let Ok(full) = map_db_feature_to_full_for_broadcast(
                        &**feature_repository.as_ref(),
                        db_feature,
                    )
                    .await
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
        }

        Ok(feature)
    }

    /// Approve a pending change request and execute when quorum reached
    async fn approve_change_request(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Change request ID")] request_id: ID,
        #[graphql(desc = "Optional approval comment")] comment: Option<String>,
    ) -> GqlResult<ApprovalRequest> {
        let user = ctx
            .data_opt::<crate::JwtUser>()
            .cloned()
            .ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;
        let logic = ctx.data::<Box<dyn ApprovalLogic>>()?;

        let request_uuid = uuid::Uuid::try_from(request_id)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let updated = logic
            .approve_request(request_uuid, user.id, comment)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(crate::graphql::schema::map_approval_request(updated))
    }

    /// Reject a pending change request
    async fn reject_change_request(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Change request ID")] request_id: ID,
        #[graphql(desc = "Reason for rejection")] comment: String,
    ) -> GqlResult<ApprovalRequest> {
        let user = ctx
            .data_opt::<crate::JwtUser>()
            .cloned()
            .ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;
        let logic = ctx.data::<Box<dyn ApprovalLogic>>()?;

        let request_uuid = uuid::Uuid::try_from(request_id)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let updated = logic
            .reject_request(request_uuid, user.id, Some(comment))
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(map_approval_request(updated))
    }

    /// Cancel a pending change request (by requester or admin)
    async fn cancel_change_request(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Change request ID")] request_id: ID,
    ) -> GqlResult<ApprovalRequest> {
        let user = ctx
            .data_opt::<crate::JwtUser>()
            .cloned()
            .ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;
        let logic = ctx.data::<Box<dyn ApprovalLogic>>()?;

        let request_uuid = uuid::Uuid::try_from(request_id)
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        let updated = logic
            .cancel_request(request_uuid, user.id)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(map_approval_request(updated))
    }

    /// Create a new approval policy
    async fn create_approval_policy(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Team ID")] team_id: ID,
        #[graphql(desc = "Approval policy input")] input: GqlCreateApprovalPolicyInput,
    ) -> GqlResult<ApprovalPolicy> {
        let team_uuid = uuid::Uuid::try_from(team_id)
            .map_err(|e| async_graphql::Error::new(format!("Invalid team id: {e}")))?;

        let env_ids = input
            .environment_ids
            .map(|ids| {
                ids.into_iter()
                    .map(|id| {
                        uuid::Uuid::try_from(id).map_err(|e| {
                            async_graphql::Error::new(format!("Invalid environment id: {e}"))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let role_ids = input
            .approver_role_ids
            .into_iter()
            .map(|id| {
                uuid::Uuid::try_from(id)
                    .map_err(|e| async_graphql::Error::new(format!("Invalid role id: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let repo = ctx.data::<Box<dyn crate::database::approval::ApprovalRepository>>()?;

        let policy = repo
            .create_policy(crate::database::approval::CreateApprovalPolicyInput {
                team_id: team_uuid,
                name: input.name,
                description: input.description,
                applies_to: input.applies_to,
                environment_ids: env_ids,
                required_approvers: input.required_approvers,
                approver_role_ids: role_ids,
                auto_approve_after_hours: input.auto_approve_after_hours,
                enabled: input.enabled.unwrap_or(true),
            })
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(map_approval_policy(policy))
    }

    /// Update an existing approval policy
    async fn update_approval_policy(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Policy ID")] policy_id: ID,
        #[graphql(desc = "Update input")] input: GqlUpdateApprovalPolicyInput,
    ) -> GqlResult<ApprovalPolicy> {
        let policy_uuid = uuid::Uuid::try_from(policy_id)
            .map_err(|e| async_graphql::Error::new(format!("Invalid policy id: {e}")))?;

        let env_ids = input
            .environment_ids
            .map(|ids| {
                ids.into_iter()
                    .map(|id| {
                        uuid::Uuid::try_from(id).map_err(|e| {
                            async_graphql::Error::new(format!("Invalid environment id: {e}"))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let role_ids = input
            .approver_role_ids
            .map(|ids| {
                ids.into_iter()
                    .map(|id| {
                        uuid::Uuid::try_from(id)
                            .map_err(|e| async_graphql::Error::new(format!("Invalid role id: {e}")))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let repo = ctx.data::<Box<dyn crate::database::approval::ApprovalRepository>>()?;

        let policy = repo
            .update_policy(
                policy_uuid,
                crate::database::approval::UpdateApprovalPolicyInput {
                    name: input.name,
                    description: input.description,
                    applies_to: input.applies_to,
                    environment_ids: env_ids,
                    required_approvers: input.required_approvers,
                    approver_role_ids: role_ids,
                    auto_approve_after_hours: input.auto_approve_after_hours,
                    enabled: input.enabled,
                },
            )
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;

        Ok(map_approval_policy(policy))
    }

    /// Delete an approval policy
    async fn delete_approval_policy(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Policy ID")] policy_id: ID,
    ) -> GqlResult<bool> {
        let policy_uuid = uuid::Uuid::try_from(policy_id)
            .map_err(|e| async_graphql::Error::new(format!("Invalid policy id: {e}")))?;

        let repo = ctx.data::<Box<dyn crate::database::approval::ApprovalRepository>>()?;

        repo.delete_policy(policy_uuid)
            .await
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    // User mutations
    async fn register_user(
        &self,
        ctx: &Context<'_>,
        input: GqlRegisterUserInput,
    ) -> GqlResult<User> {
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let created = logic
            .register_user(
                RegisterUserInput {
                    username: input.username,
                    password: input.password,
                    first_name: input.first_name,
                    last_name: input.last_name,
                    email: input.email,
                    is_admin: input.is_admin.unwrap_or(false),
                    is_temporary_password: input.is_temporary_password.unwrap_or(true), // Default to temporary password
                },
                actor,
            )
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
        input: GqlRegisterUserInput,
    ) -> GqlResult<User> {
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let created = logic
            .register_user(
                RegisterUserInput {
                    username: input.username,
                    password: input.password,
                    first_name: input.first_name,
                    last_name: input.last_name,
                    email: input.email,
                    is_admin: true,               // Force admin to true
                    is_temporary_password: false, // Default to temporary password
                },
                actor,
            )
            .await?;

        // If an admin was created, flip the admin-exists cache so middleware stops redirecting.
        if created.is_admin
            && let Ok(state) = ctx.data::<AdminState>()
        {
            state.set_exists(true);
        }
        create_user(created)
    }

    async fn login(&self, ctx: &Context<'_>, input: GqlLoginInput) -> GqlResult<LoginResponse> {
        let jwt_token_logic = ctx.data::<Box<dyn crate::logic::jwt_token::JwtTokenLogic>>()?;
        let login_result = jwt_token_logic
            .login_user(input.username, input.password)
            .await?;

        let user = create_user(login_result.user)?;
        Ok(LoginResponse {
            user,
            token: login_result.token,
            is_temporary: login_result.is_temporary,
        })
    }

    async fn logout(&self, ctx: &Context<'_>) -> GqlResult<bool> {
        // Get JWT user from context (injected by middleware)
        let jwt_user = ctx.data_opt::<crate::JwtUser>().cloned();
        let user =
            jwt_user.ok_or_else(|| async_graphql::Error::new("User authentication not found"))?;

        let jwt_token_logic = ctx.data::<Box<dyn crate::logic::jwt_token::JwtTokenLogic>>()?;

        // Revoke all tokens for this user (logout from all devices)
        let revoked_count = jwt_token_logic
            .logout_user(user.id)
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

        let jwt_token_logic = ctx.data::<Box<dyn crate::logic::jwt_token::JwtTokenLogic>>()?;

        // Revoke the specific current token using the hash from JWT user data
        let revoked = jwt_token_logic
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
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
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
                actor,
            )
            .await?;
        create_user(u)
    }

    async fn reset_password(
        &self,
        ctx: &Context<'_>,
        input: GqlResetPasswordInput,
    ) -> GqlResult<bool> {
        let jwt_user = ctx.data::<crate::JwtUser>()?;
        let user_id = ID::from(jwt_user.id);

        // For reset_password, the actor is the user themselves (self-service)
        let actor = Some(crate::logic::ActorContext::new(
            jwt_user.id,
            jwt_user.username.clone(),
        ));

        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        logic
            .reset_password(user_id, input.current_password, input.new_password, actor)
            .await?;

        Ok(true)
    }

    async fn set_temporary_password(
        &self,
        ctx: &Context<'_>,
        input: GqlSetTemporaryPasswordInput,
    ) -> GqlResult<bool> {
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        logic
            .set_temporary_password(input.user_id, input.temporary_password, actor)
            .await?;

        Ok(true)
    }

    async fn assign_user_teams(
        &self,
        ctx: &Context<'_>,
        user_id: ID,
        team_ids: Vec<ID>,
    ) -> GqlResult<Vec<Team>> {
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });
        let logic = ctx.data::<Box<dyn UserLogic>>()?;
        let _ = logic
            .assign_user_teams(user_id.clone(), team_ids, actor)
            .await?;
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

    async fn create_role(&self, ctx: &Context<'_>, input: CreateRoleInput) -> GqlResult<Role> {
        info!("Creating role '{}'", input.name);
        let jwt_user = ctx.data::<crate::JwtUser>()?;
        if !jwt_user.is_admin {
            return Err(async_graphql::Error::new(
                "Unauthorized: Admin access required",
            ));
        }

        let actor = Some(crate::logic::ActorContext::new(
            jwt_user.id,
            jwt_user.username.clone(),
        ));
        let logic = ctx.data::<Box<dyn RoleLogic>>()?;
        let role = logic
            .create_role(input.name, input.description, actor)
            .await?;

        Ok(Role {
            id: role.id,
            name: role.name,
            description: role.description,
            created_at: role.created_at,
            updated_at: role.updated_at,
        })
    }

    async fn delete_role(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting role: {id:?}");
        let jwt_user = ctx.data::<crate::JwtUser>()?;
        if !jwt_user.is_admin {
            return Err(async_graphql::Error::new(
                "Unauthorized: Admin access required",
            ));
        }

        let actor = Some(crate::logic::ActorContext::new(
            jwt_user.id,
            jwt_user.username.clone(),
        ));
        let logic = ctx.data::<Box<dyn RoleLogic>>()?;
        logic.delete_role(id, actor).await?;

        Ok(true)
    }

    async fn assign_user_roles(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "User ID to assign roles to")] user_id: ID,
        input: AssignUserRolesInput,
    ) -> GqlResult<Vec<Role>> {
        info!("Assigning roles to user: {user_id:?}");

        // Get user info from JWT context and create ActorContext
        let actor = ctx.data_opt::<crate::JwtUser>().map(|jwt_user| {
            crate::logic::ActorContext::new(jwt_user.id, jwt_user.username.clone())
        });

        let logic = ctx.data::<Box<dyn RoleLogic>>()?;
        let roles = logic
            .assign_user_roles(user_id, input.role_ids, actor)
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

    /// Generate a new JWT signing secret (admin only)
    async fn generate_jwt_secret(
        &self,
        ctx: &Context<'_>,
    ) -> GqlResult<crate::graphql::schema::JwtSecretResponse> {
        info!("Generating new JWT secret");

        // Get user info from JWT context
        let jwt_user = ctx.data::<crate::JwtUser>()?;

        // Check if user is admin
        if !jwt_user.is_admin {
            return Err(async_graphql::Error::new(
                "Unauthorized: Admin access required",
            ));
        }

        let logic = ctx.data::<Box<dyn crate::logic::jwt_secret::JwtSecretLogic>>()?;
        let secret = logic.generate_new_secret(Some(jwt_user.id)).await?;

        Ok(crate::graphql::schema::JwtSecretResponse {
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
    }

    /// Check JWT secret status (admin only)
    async fn jwt_secret_status(
        &self,
        ctx: &Context<'_>,
    ) -> GqlResult<Vec<crate::graphql::schema::JwtSecretResponse>> {
        info!("Checking JWT secret status");

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
            .map(|secret| crate::graphql::schema::JwtSecretResponse {
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

    /// Emergency deactivate all JWT secrets (admin only)
    async fn deactivate_all_jwt_secrets(&self, ctx: &Context<'_>) -> GqlResult<bool> {
        info!("Deactivating all JWT secrets");

        // Get user info from JWT context
        let jwt_user = ctx.data::<crate::JwtUser>()?;

        // Check if user is admin
        if !jwt_user.is_admin {
            return Err(async_graphql::Error::new(
                "Unauthorized: Admin access required",
            ));
        }

        let logic = ctx.data::<Box<dyn crate::logic::jwt_secret::JwtSecretLogic>>()?;
        logic.deactivate_all_secrets().await?;

        Ok(true)
    }
}

async fn map_db_feature_to_full_for_broadcast(
    feature_repository: &dyn crate::database::feature::FeatureRepository,
    f: crate::database::entity::Feature,
) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
    use crate::grpc::pb;

    // stages with criterias
    let stages = feature_repository.get_feature_stages(f.id).await?;
    let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(stages.len());
    for s in stages.iter() {
        let crits = feature_repository.get_stage_criteria(s.id).await?;
        let criterias = crits
            .into_iter()
            .map(|c| {
                // Map rule groups
                let rule_groups = c
                    .rule_groups
                    .into_iter()
                    .map(|group| crate::grpc::pb::RuleGroup {
                        id: group.id.to_string(),
                        logic_operator: match group.logic_operator {
                            crate::database::entity::LogicOperator::And => "AND".to_string(),
                            crate::database::entity::LogicOperator::Or => "OR".to_string(),
                        },
                        conditions: group
                            .conditions
                            .into_iter()
                            .map(|cond| crate::grpc::pb::RuleCondition {
                                id: cond.id.to_string(),
                                context_key: cond.context_key,
                                operator: cond.operator,
                                value: cond.value.to_string(),
                                order_index: cond.order_index,
                            })
                            .collect(),
                    })
                    .collect();

                // Map variant allocations
                let variant_allocations = c
                    .variant_allocations
                    .into_iter()
                    .map(|alloc| pb::VariantAllocation {
                        variant_control: alloc.variant_control,
                        weight: alloc.weight,
                    })
                    .collect();

                pb::StageCriterionFull {
                    id: c.id.to_string(),
                    stage_id: c.stage_id.to_string(),
                    priority: c.priority,
                    rule_groups,
                    variant_allocations,
                }
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

    // Load variants from database only for Contextual features
    use crate::database::entity::FeatureType as EntityFeatureType;
    let variant_msgs = if matches!(f.feature_type, EntityFeatureType::Contextual) {
        let db_variants = feature_repository.get_feature_variants(f.id).await?;

        db_variants
            .into_iter()
            .map(|v| pb::FeatureVariant {
                control: v.control,
                value: serde_json::to_string(&v.value).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };

    let feature = pb::FeatureFull {
        id: f.id.to_string(),
        key: f.key,
        description: f.description.unwrap_or_default(),
        feature_type: format!("{:?}", f.feature_type),
        team_id: f.team_id.to_string(),
        active: f.active,
        created_at: f.created_at.to_rfc3339(),
        kill_switch_enabled: f.kill_switch_enabled,
        kill_switch_activated_at: f
            .kill_switch_activated_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(),
        rollback_scheduled_at: f
            .rollback_scheduled_at
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default(),
        stages: stage_msgs,
        dependencies: deps,
        variants: variant_msgs,
    };
    Ok(feature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::feature::MockFeatureRepository;
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

        let feature_repo = MockFeatureRepository::new();

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .data::<Box<dyn crate::database::feature::FeatureRepository>>(Box::new(feature_repo))
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
            priority: 1,
            rule_groups: vec![],
            variant_allocations: vec![],
        }];
        let stage_id_clone = stage_id.clone();
        mock.expect_set_stage_criteria()
            .times(1)
            .withf(move |sid, crit| {
                sid == &stage_id_clone && crit.len() == 1 && crit[0].priority == 1
            })
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .finish();

        // success
        let gql = r#"
            mutation($sid: ID!) {
                setStageCriteria(stageId: $sid, criteria: [{ priority: 1 }]) { priority }
            }
        "#;
        let mut req = Request::new(gql);
        req = req
            .variables(async_graphql::Variables::from_json(serde_json::json!({
                "sid": stage_id.to_string()
            })))
            .data::<Box<dyn crate::database::feature::FeatureRepository>>(Box::new(
                MockFeatureRepository::new(),
            ));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
    }
}

#[cfg(test)]
mod more_mutation_tests {
    use super::*;
    use crate::database::entity::FeaturePipelineStage;
    use crate::database::feature::MockFeatureRepository;
    use crate::graphql::query::Query as GqlQuery;
    use crate::grpc::pb;
    use crate::logic::context::MockContextLogic;
    use crate::logic::environment::MockEnvironmentLogic;
    use crate::logic::feature::MockFeatureLogic;
    use async_graphql::{EmptySubscription, Request, Schema};
    use std::sync::Arc;
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
    async fn set_stage_criteria_broadcasts_feature_update_with_allocations() {
        let stage_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();

        // Logic mock
        let mut logic = MockFeatureLogic::new();
        logic
            .expect_set_stage_criteria()
            .returning(|_, _| Ok(vec![]));
        logic
            .expect_get_feature_id_by_stage_id()
            .returning(move |_| Ok(Some(feature_id)));

        // Repo mock
        let mut repo = MockFeatureRepository::new();
        repo.expect_get_feature_by_id().returning(move |_| {
            Ok(crate::database::entity::Feature {
                id: feature_id,
                key: "flag".into(),
                description: None,
                feature_type: crate::database::entity::FeatureType::Contextual,
                team_id: Uuid::new_v4(),
                active: true,
                created_at: chrono::Utc::now(),
                kill_switch_enabled: false,
                kill_switch_activated_at: None,
                rollback_scheduled_at: None,
                lifecycle_stage: "active".to_string(),
                deprecated_at: None,
                deprecation_notice: None,
                last_evaluated_at: None,
                evaluation_count_7d: 0,
                evaluation_count_30d: 0,
                evaluation_count_90d: 0,
                dependencies: vec![],
            })
        });
        repo.expect_get_feature_stages().returning(move |_| {
            Ok(vec![FeaturePipelineStage {
                id: stage_id,
                feature_id,
                environment_id: Uuid::new_v4(),
                order_index: 0,
                parent_stage_id: None,
                position: "{}\"".trim_matches('"').to_string(),
                enabled: true,
                bucketing_key: None,
                status: "DEPLOYED".into(),
            }])
        });
        repo.expect_get_stage_criteria().returning(move |_| {
            Ok(vec![crate::database::entity::StageCriterion {
                id: Uuid::new_v4(),
                stage_id,
                priority: 0,
                rule_groups: vec![],
                variant_allocations: vec![crate::database::entity::VariantAllocationSimple {
                    variant_control: "treatment".into(),
                    weight: 100,
                }],
            }])
        });
        repo.expect_get_feature_variants().returning(move |_| {
            Ok(vec![crate::database::entity::FeatureVariant {
                id: Uuid::new_v4(),
                feature_id,
                control: "treatment".into(),
                value: serde_json::json!(true),
                value_type: crate::database::entity::VariantValueType::Boolean,
                description: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }])
        });

        let (updates_tx, mut updates_rx) = tokio::sync::broadcast::channel::<pb::FeatureUpdate>(4);

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(logic))
            .data::<Box<dyn crate::database::feature::FeatureRepository>>(Box::new(repo))
            .data(updates_tx.clone())
            .finish();

        let gql = r#"
            mutation($sid: ID!) {
                setStageCriteria(stageId: $sid, criteria: [{ priority: 0 }]) { priority }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(
            serde_json::json!({"sid": stage_id.to_string()}),
        ));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );

        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), updates_rx.recv())
            .await
            .expect("timed out waiting for feature update")
            .expect("expected feature update");
        assert_eq!(msg.action, pb::feature_update::Action::Upsert as i32);
        let feature = msg.feature.expect("missing feature payload");
        assert_eq!(feature.stages.len(), 1);
        assert_eq!(
            feature.stages[0].criterias[0].variant_allocations[0].variant_control,
            "treatment"
        );
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
    async fn test_create_role_mutation() {
        use crate::logic::role::MockRoleLogic;
        let mut mock = MockRoleLogic::new();
        let admin_id = Uuid::new_v4();
        let role_id = ID::from(Uuid::new_v4());

        mock.expect_create_role()
            .times(1)
            .return_once(move |name, description, actor| {
                assert_eq!(name, "Observer");
                assert_eq!(description, "Read only");
                assert_eq!(actor.as_ref().map(|a| a.id), Some(admin_id));
                Ok(crate::logic::role::GqlRole {
                    id: role_id.clone(),
                    name: "Observer".to_string(),
                    description: "Read only".to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                })
            });

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
            .data(crate::JwtUser {
                id: admin_id,
                username: "admin".to_string(),
                is_admin: true,
                roles: vec![],
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($input: CreateRoleInput!) {
                createRole(input: $input) {
                    id
                    name
                    description
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "input": { "name": "Observer", "description": "Read only" }
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createRole"]["name"], "Observer");
    }

    #[tokio::test]
    async fn test_create_role_requires_admin() {
        use crate::logic::role::MockRoleLogic;
        let mock = MockRoleLogic::new();

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
            .data(crate::JwtUser {
                id: Uuid::new_v4(),
                username: "user".to_string(),
                is_admin: false,
                roles: vec![],
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($input: CreateRoleInput!) {
                createRole(input: $input) {
                    id
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "input": { "name": "Observer", "description": "Read only" }
        })));

        let resp = schema.execute(req).await;
        assert!(
            !resp.errors.is_empty(),
            "expected admin guard to block createRole"
        );
    }

    #[tokio::test]
    async fn test_delete_role_mutation() {
        use crate::logic::role::MockRoleLogic;
        let mut mock = MockRoleLogic::new();
        let admin_id = Uuid::new_v4();
        let role_id = ID::from(Uuid::new_v4());
        let expected_role_id = role_id.clone();

        mock.expect_delete_role()
            .times(1)
            .return_once(move |id, actor| {
                assert_eq!(id, expected_role_id);
                assert_eq!(actor.as_ref().map(|a| a.id), Some(admin_id));
                Ok(())
            });

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::role::RoleLogic>>(Box::new(mock))
            .data(crate::JwtUser {
                id: admin_id,
                username: "admin".to_string(),
                is_admin: true,
                roles: vec![],
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($id: ID!) {
                deleteRole(id: $id)
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": role_id.to_string()
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["deleteRole"], true);
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
                    killSwitchEnabled
                    rollbackScheduledAt
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
                    killSwitchEnabled
                    rollbackScheduledAt
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
                    killSwitchEnabled
                    rollbackScheduledAt
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
                    killSwitchEnabled
                    rollbackScheduledAt
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
                    killSwitchEnabled
                    rollbackScheduledAt
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

    #[tokio::test]
    async fn test_request_stage_change_deployment_requested_updates_status() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        mock.expect_request_stage_change()
            .times(1)
            .withf(move |sid, req, uid| {
                Uuid::try_from(sid.clone()).unwrap() == stage_id
                    && matches!(
                        req,
                        crate::logic::feature::StageChangeRequestType::DeploymentRequested
                    )
                    && *uid == user_id
            })
            .returning(move |_, _, _| Ok(create_mock_feature()));

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
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYMENT_REQUESTED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["requestStageChange"]["key"], "test_feature");
        assert!(
            data["requestStageChange"]["killSwitchEnabled"]
                .as_bool()
                .unwrap(),
            "Expected killSwitchEnabled to be true"
        );
        assert!(
            data["requestStageChange"]["rollbackScheduledAt"].is_string(),
            "Expected rollbackScheduledAt to be a timestamp string"
        );
    }

    #[tokio::test]
    async fn test_request_stage_change_deployment_rejected_updates_status() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        mock.expect_request_stage_change()
            .times(1)
            .withf(move |sid, req, uid| {
                Uuid::try_from(sid.clone()).unwrap() == stage_id
                    && matches!(
                        req,
                        crate::logic::feature::StageChangeRequestType::DeploymentRejected
                    )
                    && *uid == user_id
            })
            .returning(move |_, _, _| Ok(create_mock_feature()));

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
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYMENT_REJECTED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_request_stage_change_deployed_updates_status() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        mock.expect_request_stage_change()
            .times(1)
            .withf(move |sid, req, uid| {
                Uuid::try_from(sid.clone()).unwrap() == stage_id
                    && matches!(req, crate::logic::feature::StageChangeRequestType::Deployed)
                    && *uid == user_id
            })
            .returning(move |_, _, _| Ok(create_mock_feature()));

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
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "DEPLOYED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_request_stage_change_rollback_requested_updates_status() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        mock.expect_request_stage_change()
            .times(1)
            .withf(move |sid, req, uid| {
                Uuid::try_from(sid.clone()).unwrap() == stage_id
                    && matches!(
                        req,
                        crate::logic::feature::StageChangeRequestType::RollbackRequested
                    )
                    && *uid == user_id
            })
            .returning(move |_, _, _| Ok(create_mock_feature()));

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
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "ROLLBACK_REQUESTED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_request_stage_change_rollback_rejected_updates_status() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        mock.expect_request_stage_change()
            .times(1)
            .withf(move |sid, req, uid| {
                Uuid::try_from(sid.clone()).unwrap() == stage_id
                    && matches!(
                        req,
                        crate::logic::feature::StageChangeRequestType::RollbackRejected
                    )
                    && *uid == user_id
            })
            .returning(move |_, _, _| Ok(create_mock_feature()));

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
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "ROLLBACK_REJECTED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_request_stage_change_rollbacked_updates_status() {
        use crate::logic::feature::MockFeatureLogic;

        let mut mock = MockFeatureLogic::new();
        let stage_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        mock.expect_request_stage_change()
            .times(1)
            .withf(move |sid, req, uid| {
                Uuid::try_from(sid.clone()).unwrap() == stage_id
                    && matches!(
                        req,
                        crate::logic::feature::StageChangeRequestType::Rollbacked
                    )
                    && *uid == user_id
            })
            .returning(move |_, _, _| Ok(create_mock_feature()));

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
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": stage_id.to_string(),
            "request": "ROLLBACKED"
        })));

        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_request_stage_change_without_authentication_fails() {
        use crate::logic::feature::MockFeatureLogic;

        let mock = MockFeatureLogic::new();

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            // No JWT user data
            .finish();

        let gql = r#"
            mutation($stageId: ID!, $request: StageChangeRequest!) {
                requestStageChange(stageId: $stageId, request: $request) {
                    id
                    key
                    killSwitchEnabled
                    rollbackScheduledAt
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "stageId": Uuid::new_v4().to_string(),
            "request": "DEPLOYMENT_REQUESTED"
        })));

        let resp = schema.execute(req).await;
        assert!(!resp.errors.is_empty());
        assert!(
            resp.errors[0]
                .message
                .contains("User authentication not found")
        );
    }

    #[tokio::test]
    async fn test_request_stage_change_enum_conversion_matches_logic_type() {
        // This test ensures GraphQL enum variants map correctly to logic enum variants
        use crate::logic::feature::StageChangeRequestType;

        // Test conversion from GraphQL enum to logic enum
        let conversions = vec![
            (
                StageChangeRequest::DeploymentRequested,
                StageChangeRequestType::DeploymentRequested,
            ),
            (
                StageChangeRequest::DeploymentRejected,
                StageChangeRequestType::DeploymentRejected,
            ),
            (
                StageChangeRequest::Deployed,
                StageChangeRequestType::Deployed,
            ),
            (
                StageChangeRequest::RollbackRequested,
                StageChangeRequestType::RollbackRequested,
            ),
            (
                StageChangeRequest::RollbackRejected,
                StageChangeRequestType::RollbackRejected,
            ),
            (
                StageChangeRequest::Rollbacked,
                StageChangeRequestType::Rollbacked,
            ),
        ];

        for (gql_enum, logic_enum) in conversions {
            let logic_converted = match gql_enum {
                StageChangeRequest::DeploymentRequested => {
                    StageChangeRequestType::DeploymentRequested
                }
                StageChangeRequest::DeploymentRejected => {
                    StageChangeRequestType::DeploymentRejected
                }
                StageChangeRequest::Deployed => StageChangeRequestType::Deployed,
                StageChangeRequest::RollbackRequested => StageChangeRequestType::RollbackRequested,
                StageChangeRequest::RollbackRejected => StageChangeRequestType::RollbackRejected,
                StageChangeRequest::Rollbacked => StageChangeRequestType::Rollbacked,
            };

            assert_eq!(
                std::mem::discriminant(&logic_converted),
                std::mem::discriminant(&logic_enum),
                "GraphQL enum {:?} should map to logic enum {:?}",
                gql_enum,
                logic_enum
            );
        }
    }

    #[tokio::test]
    async fn test_approve_change_request_maps_response() {
        #[derive(Clone)]
        struct StubApprovalLogic {
            expected_id: Uuid,
            expected_user: Uuid,
            response: crate::database::entity::ApprovalRequest,
        }

        #[async_trait::async_trait]
        impl ApprovalLogic for StubApprovalLogic {
            async fn maybe_create_stage_change_request(
                &self,
                _: &crate::database::entity::Feature,
                _: &crate::database::entity::FeaturePipelineStage,
                _: &str,
                _: Uuid,
            ) -> Result<Option<crate::database::entity::ApprovalRequest>, crate::Error>
            {
                Ok(None)
            }

            async fn approve_request(
                &self,
                request_id: Uuid,
                approver_id: Uuid,
                _comment: Option<String>,
            ) -> Result<crate::database::entity::ApprovalRequest, crate::Error> {
                assert_eq!(request_id, self.expected_id);
                assert_eq!(approver_id, self.expected_user);
                Ok(self.response.clone())
            }

            async fn reject_request(
                &self,
                _request_id: Uuid,
                _approver_id: Uuid,
                _comment: Option<String>,
            ) -> Result<crate::database::entity::ApprovalRequest, crate::Error> {
                Err(crate::Error::InvalidInput("not used".into()))
            }

            async fn cancel_request(
                &self,
                _request_id: Uuid,
                _cancelled_by: Uuid,
            ) -> Result<crate::database::entity::ApprovalRequest, crate::Error> {
                Err(crate::Error::InvalidInput("not used".into()))
            }

            async fn get_request(
                &self,
                _request_id: Uuid,
            ) -> Result<crate::database::entity::ApprovalRequest, crate::Error> {
                Err(crate::Error::InvalidInput("not used".into()))
            }

            async fn list_requests_for_team(
                &self,
                _team_id: Option<Uuid>,
                _statuses: Option<Vec<crate::database::entity::ApprovalStatus>>,
                _page_number: Option<i32>,
                _page_size: Option<i32>,
            ) -> Result<(Vec<crate::database::entity::ApprovalRequest>, i64), crate::Error>
            {
                Err(crate::Error::InvalidInput("not used".into()))
            }

            async fn auto_approve_request(
                &self,
                _request: crate::database::entity::ApprovalRequest,
            ) -> Result<crate::database::entity::ApprovalRequest, crate::Error> {
                Err(crate::Error::InvalidInput("not used".into()))
            }

            fn clone_box(&self) -> Box<dyn ApprovalLogic> {
                Box::new(self.clone())
            }
        }

        let request_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();
        let approver_id = Uuid::new_v4();

        let db_request = crate::database::entity::ApprovalRequest {
            id: request_id,
            policy_id,
            feature_id,
            environment_id: Some(environment_id),
            change_type: "stage_change".into(),
            change_payload: serde_json::json!({
                "stage_id": "stage-1",
                "next_status": "DEPLOYED"
            }),
            change_description: Some("Deploy to prod".into()),
            requested_by: Uuid::new_v4(),
            status: crate::database::entity::ApprovalStatus::Approved,
            approved_count: 2,
            rejected_count: 0,
            executed_at: Some(Utc::now()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn ApprovalLogic>>(Box::new(StubApprovalLogic {
                expected_id: request_id,
                expected_user: approver_id,
                response: db_request.clone(),
            }))
            .data(crate::JwtUser {
                id: approver_id,
                username: "approver".to_string(),
                is_admin: false,
                roles: vec!["Approver".to_string()],
                token_hash: "hash".to_string(),
            })
            .finish();

        let gql = r#"
            mutation($id: ID!) {
                approveChangeRequest(requestId: $id, comment: "Ship it") {
                    id
                    policyId
                    featureId
                    status
                    approvedCount
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": request_id.to_string()
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["approveChangeRequest"]["status"], "Approved");
        assert_eq!(data["approveChangeRequest"]["approvedCount"], 2);
        assert_eq!(
            data["approveChangeRequest"]["policyId"],
            policy_id.to_string()
        );
        assert_eq!(
            data["approveChangeRequest"]["featureId"],
            feature_id.to_string()
        );
    }

    // Helper function to create a mock feature for testing
    fn create_mock_feature() -> crate::graphql::schema::Feature {
        crate::graphql::schema::Feature {
            id: async_graphql::ID::from(Uuid::new_v4().to_string()),
            key: "test_feature".to_string(),
            description: Some("Test description".to_string()),
            feature_type: crate::graphql::schema::FeatureType::Simple,
            enabled: true,
            kill_switch_enabled: true,
            kill_switch_activated_at: None,
            rollback_scheduled_at: Some(Utc::now()),
            lifecycle_stage: crate::graphql::schema::LifecycleStage::Active,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
            team_id: async_graphql::ID::from(Uuid::new_v4().to_string()),
            pending_approval_request_id: None,
        }
    }

    #[tokio::test]
    async fn test_create_environment_mutation() {
        use crate::logic::environment::MockEnvironmentLogic;

        let mut mock = MockEnvironmentLogic::new();
        let team_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();

        let input = crate::graphql::schema::CreateEnvironmentInput {
            name: "staging".to_string(),
            active: true,
            environment_type: Some("Development".to_string()),
        };

        let expected = crate::graphql::schema::Environment {
            id: ID::from(env_id),
            name: "staging".to_string(),
            team_id: ID::from(team_id),
            active: true,
            environment_type: "Development".to_string(),
        };

        let team_id_clone = team_id;
        mock.expect_create_environment()
            .times(1)
            .withf(move |tid, inp, _actor| {
                Uuid::try_from(tid.clone()).unwrap() == team_id_clone
                    && inp.name == "staging"
                    && inp.active == true
            })
            .returning(move |_, _, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::environment::EnvironmentLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($teamId: ID!, $name: String!, $active: Boolean!) {
                createEnvironment(teamId: $teamId, input: { name: $name, active: $active }) {
                    id
                    name
                    active
                    teamId
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "teamId": team_id.to_string(),
            "name": "staging",
            "active": true
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createEnvironment"]["name"], "staging");
        assert_eq!(data["createEnvironment"]["active"], true);
    }

    #[tokio::test]
    async fn test_update_environment_mutation() {
        use crate::logic::environment::MockEnvironmentLogic;

        let mut mock = MockEnvironmentLogic::new();
        let env_id = Uuid::new_v4();

        let input = crate::graphql::schema::UpdateEnvironmentInput {
            name: Some("production".to_string()),
            active: Some(false),
            environment_type: Some("Production".to_string()),
        };

        let expected = crate::graphql::schema::Environment {
            id: ID::from(env_id),
            name: "production".to_string(),
            team_id: ID::from(Uuid::new_v4()),
            active: false,
            environment_type: "Production".to_string(),
        };

        let env_id_clone = env_id;
        mock.expect_update_environment()
            .times(1)
            .withf(move |id, inp, _actor| {
                Uuid::try_from(id.clone()).unwrap() == env_id_clone
                    && inp.name.as_ref() == Some(&"production".to_string())
                    && inp.active == Some(false)
            })
            .returning(move |_, _, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::environment::EnvironmentLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($id: ID!, $name: String, $active: Boolean) {
                updateEnvironment(id: $id, input: { name: $name, active: $active }) {
                    id
                    name
                    active
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": env_id.to_string(),
            "name": "production",
            "active": false
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["updateEnvironment"]["name"], "production");
        assert_eq!(data["updateEnvironment"]["active"], false);
    }

    #[tokio::test]
    async fn test_delete_environment_mutation() {
        use crate::logic::environment::MockEnvironmentLogic;

        let mut mock = MockEnvironmentLogic::new();
        let env_id = Uuid::new_v4();

        let env_id_clone = env_id;
        mock.expect_delete_environment()
            .times(1)
            .withf(move |id, _actor| Uuid::try_from(id.clone()).unwrap() == env_id_clone)
            .returning(move |_, _| Ok(()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::environment::EnvironmentLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($id: ID!) {
                deleteEnvironment(id: $id)
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": env_id.to_string()
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["deleteEnvironment"], true);
    }

    #[tokio::test]
    async fn test_create_team_mutation() {
        use crate::logic::team::MockTeamLogic;

        let mut mock = MockTeamLogic::new();
        let team_id = Uuid::new_v4();

        let input = crate::graphql::schema::CreateTeamInput {
            name: "Development Team".to_string(),
            description: "Team responsible for development".to_string(),
        };

        let expected = crate::graphql::schema::Team {
            id: ID::from(team_id),
            name: "Development Team".to_string(),
            description: "Team responsible for development".to_string(),
        };

        mock.expect_create_team()
            .times(1)
            .withf(move |inp, _actor| {
                inp.name == "Development Team"
                    && inp.description == "Team responsible for development"
            })
            .returning(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::team::TeamLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($name: String!, $description: String!) {
                createTeam(input: { name: $name, description: $description }) {
                    id
                    name
                    description
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "name": "Development Team",
            "description": "Team responsible for development"
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createTeam"]["name"], "Development Team");
        assert_eq!(
            data["createTeam"]["description"],
            "Team responsible for development"
        );
    }

    #[tokio::test]
    async fn test_update_team_mutation() {
        use crate::logic::team::MockTeamLogic;

        let mut mock = MockTeamLogic::new();
        let team_id = Uuid::new_v4();

        let input = crate::graphql::schema::UpdateTeamInput {
            name: Some("Updated Team".to_string()),
            description: Some("Updated description".to_string()),
        };

        let expected = crate::graphql::schema::Team {
            id: ID::from(team_id),
            name: "Updated Team".to_string(),
            description: "Updated description".to_string(),
        };

        let team_id_clone = team_id;
        mock.expect_update_team()
            .times(1)
            .withf(move |id, inp, _actor| {
                Uuid::try_from(id.clone()).unwrap() == team_id_clone
                    && inp.name.as_ref() == Some(&"Updated Team".to_string())
                    && inp.description.as_ref() == Some(&"Updated description".to_string())
            })
            .returning(move |_, _, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::team::TeamLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($id: ID!, $name: String, $description: String) {
                updateTeam(id: $id, input: { name: $name, description: $description }) {
                    id
                    name
                    description
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": team_id.to_string(),
            "name": "Updated Team",
            "description": "Updated description"
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["updateTeam"]["name"], "Updated Team");
        assert_eq!(data["updateTeam"]["description"], "Updated description");
    }

    #[tokio::test]
    async fn test_create_pipeline_mutation() {
        use crate::logic::pipeline::MockPipelineLogic;

        let mut mock = MockPipelineLogic::new();
        let team_id = Uuid::new_v4();
        let pipeline_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();

        let stage_input = crate::graphql::schema::CreateStageInput {
            environment_id: ID::from(env_id),
            order_index: 0,
            position: "dev".to_string(),
        };

        let _input = crate::graphql::schema::CreatePipelineInput {
            name: "CI/CD Pipeline".to_string(),
            stages: vec![stage_input],
            relationships: vec![],
        };

        // Mock the validator call to get_pipelines (should return empty for new pipeline)
        let team_id_clone = team_id;
        mock.expect_get_pipelines()
            .times(1)
            .withf(move |tid, name, active, _fields| {
                Uuid::try_from(tid.clone()).unwrap() == team_id_clone
                    && name.as_ref() == Some(&"CI/CD Pipeline".to_string())
                    && active == &Some(true)
            })
            .returning(move |_, _, _, _| Ok(vec![])); // Empty vec means no duplicate

        // Mock the actual create_pipeline call
        let team_id_clone2 = team_id;
        mock.expect_create_pipeline()
            .times(1)
            .withf(move |tid, inp, _actor| {
                Uuid::try_from(tid.clone()).unwrap() == team_id_clone2
                    && inp.name == "CI/CD Pipeline"
                    && inp.stages.len() == 1
            })
            .returning(move |_, _, _| Ok(ID::from(pipeline_id)));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::pipeline::PipelineLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($teamId: ID!, $name: String!, $environmentId: ID!) {
                createPipeline(teamId: $teamId, input: { 
                    name: $name, 
                    stages: [{ environmentId: $environmentId, orderIndex: 0, position: "dev" }],
                    relationships: []
                })
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "teamId": team_id.to_string(),
            "name": "CI/CD Pipeline",
            "environmentId": env_id.to_string()
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createPipeline"], pipeline_id.to_string());
    }

    #[tokio::test]
    async fn test_update_pipeline_mutation() {
        use crate::logic::pipeline::MockPipelineLogic;

        let mut mock = MockPipelineLogic::new();
        let pipeline_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();

        let stage_input = crate::graphql::schema::CreateStageInput {
            environment_id: ID::from(env_id),
            order_index: 0,
            position: "prod".to_string(),
        };

        let _input = crate::graphql::schema::UpdatePipelineInput {
            name: Some("Updated Pipeline".to_string()),
            active: Some(false),
            stages: vec![stage_input],
            relationships: vec![],
        };

        let existing_pipeline = crate::graphql::schema::Pipeline {
            id: ID::from(pipeline_id),
            name: "Old Pipeline".to_string(),
            active: true,
            team_id: ID::from(team_id),
            stages: vec![],
            relationships: vec![],
        };

        let updated_pipeline = crate::graphql::schema::Pipeline {
            id: ID::from(pipeline_id),
            name: "Updated Pipeline".to_string(),
            active: false,
            team_id: ID::from(team_id),
            stages: vec![],
            relationships: vec![],
        };

        // Mock validator call to get_pipeline_by_id
        let pipeline_id_clone = pipeline_id;
        mock.expect_get_pipeline_by_id()
            .times(1)
            .withf(move |id| Uuid::try_from(id.clone()).unwrap() == pipeline_id_clone)
            .returning(move |_| Ok(existing_pipeline.clone()));

        // Mock validator call to get_pipelines (should return empty to allow update)
        let team_id_clone = team_id;
        mock.expect_get_pipelines()
            .times(1)
            .withf(move |tid, name, active, _fields| {
                Uuid::try_from(tid.clone()).unwrap() == team_id_clone
                    && name.as_ref() == Some(&"Updated Pipeline".to_string())
                    && active == &Some(false)
            })
            .returning(move |_, _, _, _| Ok(vec![])); // Empty vec means no duplicate

        // Mock the actual update_pipeline call
        let pipeline_id_clone2 = pipeline_id;
        mock.expect_update_pipeline()
            .times(1)
            .withf(move |id, inp, _actor| {
                Uuid::try_from(id.clone()).unwrap() == pipeline_id_clone2
                    && inp.name.as_ref() == Some(&"Updated Pipeline".to_string())
                    && inp.active == Some(false)
            })
            .returning(move |_, _, _| Ok(updated_pipeline.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::pipeline::PipelineLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($id: ID!, $name: String, $active: Boolean, $environmentId: ID!) {
                updatePipeline(id: $id, input: { 
                    name: $name, 
                    active: $active,
                    stages: [{ environmentId: $environmentId, orderIndex: 0, position: "prod" }],
                    relationships: []
                }) {
                    id
                    name
                    active
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": pipeline_id.to_string(),
            "name": "Updated Pipeline",
            "active": false,
            "environmentId": env_id.to_string()
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["updatePipeline"]["name"], "Updated Pipeline");
        assert_eq!(data["updatePipeline"]["active"], false);
    }

    #[tokio::test]
    async fn test_delete_pipeline_mutation() {
        use crate::logic::pipeline::MockPipelineLogic;

        let mut mock = MockPipelineLogic::new();
        let pipeline_id = Uuid::new_v4();

        let pipeline_id_clone = pipeline_id;
        mock.expect_delete_pipeline()
            .times(1)
            .withf(move |id, _actor| Uuid::try_from(id.clone()).unwrap() == pipeline_id_clone)
            .returning(move |_, _| Ok(()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::pipeline::PipelineLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($id: ID!) {
                deletePipeline(id: $id)
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "id": pipeline_id.to_string()
        })));

        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "Expected no errors, but got: {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["deletePipeline"], true);
    }
}
