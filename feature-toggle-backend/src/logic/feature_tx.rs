use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::compound_rules::{
    CompoundRulesRepositoryTx, CreateRuleConditionInput, CreateRuleGroupInput,
};
use crate::database::feature::{
    CreateFeature, CreateFeatureStage, CreateStageCriterion, FeatureRepositoryTx, UpdateFeature,
};
use crate::database::variant_allocations::{
    CreateVariantAllocationInput, VariantAllocationsRepositoryTx,
};
use crate::graphql::schema::{
    Context as GqlContext, ContextEntry as GqlContextEntry, CreateFeatureInput,
    CreateStageCriterionInput, Feature as GqlFeature, FeatureType as GqlFeatureType, RuleOperator,
    StageCriterion as GqlStageCriterion, UpdateFeatureInput,
    VariantValueType as GqlVariantValueType,
};
use crate::logic::ActorContext;
use crate::logic::stage_builder::build_stage_relationships;
use async_graphql::ID;
use sqlx::PgConnection;
use uuid::Uuid;

// --- Helpers ---

fn id_to_uuid(id: ID) -> Result<Uuid, Error> {
    Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))
}

fn map_graphql_to_entity_feature_type(ft: GqlFeatureType) -> crate::database::entity::FeatureType {
    match ft {
        GqlFeatureType::Simple => crate::database::entity::FeatureType::Simple,
        GqlFeatureType::Contextual => crate::database::entity::FeatureType::Contextual,
    }
}

fn map_entity_to_graphql_feature(feature_entity: crate::database::entity::Feature) -> GqlFeature {
    GqlFeature {
        id: feature_entity.id.into(),
        key: feature_entity.key,
        description: feature_entity.description,
        feature_type: match feature_entity.feature_type {
            crate::database::entity::FeatureType::Simple => GqlFeatureType::Simple,
            crate::database::entity::FeatureType::Contextual => GqlFeatureType::Contextual,
        },
        enabled: feature_entity.active,
        kill_switch_enabled: feature_entity.kill_switch_enabled,
        kill_switch_activated_at: feature_entity.kill_switch_activated_at,
        rollback_scheduled_at: feature_entity.rollback_scheduled_at,
        lifecycle_stage: match feature_entity.lifecycle_stage.to_lowercase().as_str() {
            "deprecated" => crate::graphql::schema::LifecycleStage::Deprecated,
            "archived" => crate::graphql::schema::LifecycleStage::Archived,
            "permanent" => crate::graphql::schema::LifecycleStage::Permanent,
            _ => crate::graphql::schema::LifecycleStage::Active,
        },
        deprecated_at: feature_entity.deprecated_at,
        deprecation_notice: feature_entity.deprecation_notice,
        last_evaluated_at: feature_entity.last_evaluated_at,
        evaluation_count_7d: feature_entity.evaluation_count_7d,
        evaluation_count_30d: feature_entity.evaluation_count_30d,
        evaluation_count_90d: feature_entity.evaluation_count_90d,
        team_id: feature_entity.team_id.into(),
        dependencies: feature_entity
            .dependencies
            .into_iter()
            .map(|d| d.depends_on_id.into())
            .collect(),
        pending_approval_request_id: None,
    }
}

fn parse_rule_operator(s: &str) -> RuleOperator {
    match s {
        "EQUALS" => RuleOperator::Equals,
        "NOT_EQUALS" => RuleOperator::NotEquals,
        "GREATER_THAN" => RuleOperator::GreaterThan,
        "LESS_THAN" => RuleOperator::LessThan,
        "GREATER_THAN_OR_EQUAL" => RuleOperator::GreaterThanOrEqual,
        "LESS_THAN_OR_EQUAL" => RuleOperator::LessThanOrEqual,
        "CONTAINS" => RuleOperator::Contains,
        "STARTS_WITH" => RuleOperator::StartsWith,
        "ENDS_WITH" => RuleOperator::EndsWith,
        "REGEX" => RuleOperator::Regex,
        "IN" => RuleOperator::In,
        "NOT_IN" => RuleOperator::NotIn,
        "SEMVER_GREATER_THAN" => RuleOperator::SemverGreaterThan,
        "SEMVER_LESS_THAN" => RuleOperator::SemverLessThan,
        _ => RuleOperator::In, // Default fallback
    }
}

// --- Transactions ---

pub async fn set_stage_contexts_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    stage_id: ID,
    context_ids: Vec<ID>,
) -> Result<Vec<GqlContext>, Error>
where
    R: FeatureRepositoryTx + ?Sized,
{
    let stage_uuid = id_to_uuid(stage_id)?;
    let ctx_uuids: Vec<Uuid> = context_ids
        .into_iter()
        .map(|id| id_to_uuid(id))
        .collect::<Result<Vec<Uuid>, _>>()?;

    let result = repo
        .set_stage_contexts_tx(conn, stage_uuid, ctx_uuids)
        .await?;

    Ok(result
        .into_iter()
        .map(|c| GqlContext {
            id: ID::from(c.id),
            team_id: ID::from(c.team_id),
            key: c.key,
            entries: c
                .entries
                .into_iter()
                .map(|e| GqlContextEntry {
                    id: ID::from(e.id),
                    value: e.value,
                })
                .collect(),
        })
        .collect())
}

pub async fn set_stage_criteria_in_tx<F, V, C>(
    conn: &mut PgConnection,
    feature_repo: &F,
    variant_repo: &V,
    compound_rules_repo: &C,
    stage_id: ID,
    criteria: Vec<CreateStageCriterionInput>,
) -> Result<Vec<GqlStageCriterion>, Error>
where
    F: FeatureRepositoryTx + ?Sized,
    V: VariantAllocationsRepositoryTx + ?Sized,
    C: CompoundRulesRepositoryTx + ?Sized,
{
    let stage_uuid = id_to_uuid(stage_id)?;

    // Map GraphQL input to DB input for criteria creation
    let db_criteria_input: Vec<CreateStageCriterion> = criteria
        .iter()
        .map(|c| {
            let mode = match c.variant_selection_mode {
                Some(crate::graphql::schema::VariantSelectionMode::WeightedSplit) => {
                    crate::database::entity::VariantSelectionMode::WeightedSplit
                }
                Some(crate::graphql::schema::VariantSelectionMode::SpecificVariant) => {
                    crate::database::entity::VariantSelectionMode::SpecificVariant
                }
                None => crate::database::entity::VariantSelectionMode::WeightedSplit, // Default
            };
            CreateStageCriterion {
                priority: c.priority,
                variant_selection_mode: mode,
                selected_variant_control: c.selected_variant_control.clone(),
            }
        })
        .collect();

    let created_criteria = feature_repo
        .set_stage_criteria_tx(conn, stage_uuid, db_criteria_input)
        .await?;

    let mut input_sorted = criteria.clone();
    input_sorted.sort_by_key(|c| c.priority);

    if created_criteria.len() != input_sorted.len() {
        return Err(Error::DatabaseError(sqlx::Error::RowNotFound));
    }

    for (created, input) in created_criteria.iter().zip(input_sorted.iter()) {
        let criterion_id = created.id;

        if let Some(allocs) = &input.variant_allocations {
            let db_allocs: Vec<CreateVariantAllocationInput> = allocs
                .iter()
                .map(|a| CreateVariantAllocationInput {
                    criteria_id: criterion_id,
                    variant_control: a.variant_control.clone(),
                    weight: a.weight,
                })
                .collect();

            variant_repo
                .set_allocations_tx(conn, criterion_id, db_allocs)
                .await?;
        }

        if let Some(groups) = &input.rule_groups {
            let db_groups: Vec<CreateRuleGroupInput> = groups
                .iter()
                .map(|g| {
                    let conditions = g
                        .conditions
                        .iter()
                        .map(|cond| CreateRuleConditionInput {
                            context_key: cond.context_key.clone(),
                            operator: cond.operator.to_db_string(),
                            value: cond.value.0.clone(),
                            order_index: cond.order_index,
                        })
                        .collect();

                    let logic = match g.logic_operator {
                        crate::graphql::schema::LogicOperator::And => {
                            crate::database::entity::LogicOperator::And
                        }
                        crate::graphql::schema::LogicOperator::Or => {
                            crate::database::entity::LogicOperator::Or
                        }
                    };

                    CreateRuleGroupInput {
                        criteria_id: criterion_id,
                        logic_operator: logic,
                        conditions,
                    }
                })
                .collect();

            compound_rules_repo
                .set_rule_groups_tx(conn, criterion_id, db_groups)
                .await?;
        }
    }

    let final_result = feature_repo.get_stage_criteria_tx(conn, stage_uuid).await?;

    Ok(final_result
        .into_iter()
        .map(|c| {
            let mode = match c.variant_selection_mode {
                crate::database::entity::VariantSelectionMode::WeightedSplit => {
                    crate::graphql::schema::VariantSelectionMode::WeightedSplit
                }
                crate::database::entity::VariantSelectionMode::SpecificVariant => {
                    crate::graphql::schema::VariantSelectionMode::SpecificVariant
                }
            };

            let allocations = c
                .variant_allocations
                .into_iter()
                .map(|a| crate::graphql::schema::VariantAllocation {
                    id: ID::from(Uuid::nil()), // Placeholder
                    criteria_id: ID::from(c.id),
                    variant_control: a.variant_control,
                    weight: a.weight,
                })
                .collect();

            let rule_groups = c
                .rule_groups
                .into_iter()
                .map(|g| {
                    let conditions = g
                        .conditions
                        .into_iter()
                        .map(|cond| crate::graphql::schema::CompoundRuleCondition {
                            id: ID::from(cond.id),
                            context_key: cond.context_key,
                            operator: parse_rule_operator(&cond.operator),
                            value: async_graphql::Json(cond.value),
                            order_index: cond.order_index,
                        })
                        .collect();

                    let logic = match g.logic_operator {
                        crate::database::entity::LogicOperator::And => {
                            crate::graphql::schema::LogicOperator::And
                        }
                        crate::database::entity::LogicOperator::Or => {
                            crate::graphql::schema::LogicOperator::Or
                        }
                    };

                    crate::graphql::schema::CompoundRuleGroup {
                        id: ID::from(g.id),
                        logic_operator: logic,
                        conditions,
                    }
                })
                .collect();

            GqlStageCriterion {
                id: ID::from(c.id),
                stage_id: ID::from(c.stage_id),
                priority: c.priority,
                variant_selection_mode: mode,
                selected_variant_control: c.selected_variant_control,
                variant_allocations: allocations,
                rule_groups,
            }
        })
        .collect())
}

pub async fn create_feature_in_tx<R, A>(
    conn: &mut PgConnection,
    feature_repo: &R,
    activity_repo: &A,
    team_id: ID,
    input: CreateFeatureInput,
    actor: Option<ActorContext>,
) -> Result<ID, Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let team_uuid = id_to_uuid(team_id)?;
    let feature_key = input.key.clone();

    // Map stages
    let raw_stages = input.stages; // Fixed: already vec
    let raw_relationships = input.relationships; // Fixed: already vec

    // Build stages logic
    let stages = raw_stages
        .into_iter()
        .map(|stage| -> Result<CreateFeatureStage, Error> {
            Ok(CreateFeatureStage {
                id: match stage.id {
                    Some(id) => id_to_uuid(id)?,
                    None => Uuid::new_v4(),
                },
                environment_id: id_to_uuid(stage.environment_id)?,
                order_index: stage.order_index,
                position: stage.position,
                enabled: false,
                parent_stage: None,
            })
        })
        .collect::<Result<Vec<CreateFeatureStage>, Error>>()?;

    let stages = build_stage_relationships(stages, raw_relationships);

    // Map variants
    let variants = input.variants.map(|v| {
        v.into_iter()
            .map(|variant| {
                let value_type = match variant.value_type {
                    GqlVariantValueType::String => {
                        crate::database::entity::VariantValueType::String
                    }
                    GqlVariantValueType::Number => {
                        crate::database::entity::VariantValueType::Number
                    }
                    GqlVariantValueType::Boolean => {
                        crate::database::entity::VariantValueType::Boolean
                    }
                    GqlVariantValueType::Json => crate::database::entity::VariantValueType::Json,
                };
                (
                    variant.control,
                    variant.value.0,
                    value_type,
                    variant.description,
                )
            })
            .collect::<Vec<_>>()
    });

    let db_input = CreateFeature {
        team_id: team_uuid,
        key: input.key,
        description: input.description,
        feature_type: map_graphql_to_entity_feature_type(input.feature_type),
        stages,
        dependencies: input
            .dependencies
            .into_iter()
            .map(id_to_uuid)
            .collect::<Result<Vec<_>, _>>()?,
        variants,
    };

    let feature_uuid = feature_repo.create_feature_tx(conn, db_input).await?;

    // Activity Log
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    let activity = CreateActivityLog {
        activity_type: crate::utils::activity_logger::activity_types::FEATURE_CREATED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Created feature '{}'", feature_key),
        metadata: Some(serde_json::json!({
            "feature_id": feature_uuid.to_string(),
            "feature_key": feature_key,
            "team_id": team_uuid.to_string(),
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(ID::from(feature_uuid))
}

pub async fn update_feature_in_tx<R, A>(
    conn: &mut PgConnection,
    feature_repo: &R,
    activity_repo: &A,
    id: ID,
    input: UpdateFeatureInput,
    actor: Option<ActorContext>,
) -> Result<GqlFeature, Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let feature_uuid = id_to_uuid(id)?;

    // Map update input
    let feature_type = Some(map_graphql_to_entity_feature_type(input.feature_type));

    let stages = {
        let mapped = input
            .stages
            .into_iter()
            .map(|stage| -> Result<CreateFeatureStage, Error> {
                Ok(CreateFeatureStage {
                    id: match stage.id {
                        Some(id) => id_to_uuid(id)?,
                        None => Uuid::new_v4(),
                    },
                    environment_id: id_to_uuid(stage.environment_id)?,
                    order_index: stage.order_index,
                    position: stage.position,
                    enabled: false,
                    parent_stage: None,
                })
            })
            .collect::<Result<Vec<CreateFeatureStage>, Error>>()?;
        build_stage_relationships(mapped, input.relationships)
    };

    let dependencies = input
        .dependencies
        .into_iter()
        .map(id_to_uuid)
        .collect::<Result<Vec<_>, _>>()?;

    let variants = input.variants.map(|v| {
        v.into_iter()
            .map(|variant| {
                let value_type = match variant.value_type {
                    GqlVariantValueType::String => {
                        crate::database::entity::VariantValueType::String
                    }
                    GqlVariantValueType::Number => {
                        crate::database::entity::VariantValueType::Number
                    }
                    GqlVariantValueType::Boolean => {
                        crate::database::entity::VariantValueType::Boolean
                    }
                    GqlVariantValueType::Json => crate::database::entity::VariantValueType::Json,
                };
                (
                    variant.control,
                    variant.value.0,
                    value_type,
                    variant.description,
                )
            })
            .collect::<Vec<_>>()
    });

    let db_input = UpdateFeature {
        id: feature_uuid,
        key: Some(input.key),
        description: input.description,
        feature_type,
        stages,
        dependencies,
        variants,
    };

    let updated_feature = feature_repo.update_feature_tx(conn, db_input).await?;

    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    let activity = CreateActivityLog {
        activity_type: crate::utils::activity_logger::activity_types::FEATURE_UPDATED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated feature '{}'", updated_feature.key),
        metadata: Some(serde_json::json!({
             "feature_id": feature_uuid.to_string(),
             "feature_key": updated_feature.key
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(map_entity_to_graphql_feature(updated_feature))
}

pub async fn delete_feature_in_tx<R, A>(
    conn: &mut PgConnection,
    feature_repo: &R,
    activity_repo: &A,
    id: ID,
    key: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let uuid = id_to_uuid(id)?;

    feature_repo.delete_feature_tx(conn, uuid).await?;

    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    let activity = CreateActivityLog {
        activity_type: crate::utils::activity_logger::activity_types::FEATURE_DELETED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted feature '{}'", key),
        metadata: Some(serde_json::json!({
             "feature_id": uuid.to_string(),
             "feature_key": key
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}
