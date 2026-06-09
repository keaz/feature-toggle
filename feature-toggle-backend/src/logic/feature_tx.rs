use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::compound_rules::{
    CompoundRulesRepositoryTx, CreateRuleConditionInput, CreateRuleGroupInput,
};
use crate::database::feature::{
    CreateFeature, CreateFeatureStage, CreateStageCriterion, FeatureRepositoryTx, UpdateFeature,
    diff_entries_to_json, diff_feature_snapshots, snapshot_dependencies, snapshot_feature_key,
};
use crate::database::variant_allocations::{
    CreateVariantAllocationInput, VariantAllocationsRepositoryTx,
};
use crate::logic::ActorContext;
use crate::logic::stage_builder::build_stage_relationships;
use crate::model::ID;
use crate::model::{
    Context as ModelContext, ContextEntry as ModelContextEntry, CreateFeatureInput,
    CreateStageCriterionInput, Feature as ModelFeature, FeatureType as ModelFeatureType,
    LifecycleStage as ModelLifecycleStage, RuleOperator, StageCriterion as ModelStageCriterion,
    UpdateFeatureInput, VariantValueType as ModelVariantValueType,
};
use chrono::{Duration, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

// --- Helpers ---

fn id_to_uuid(id: ID) -> Result<Uuid, Error> {
    Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))
}

fn map_api_to_entity_feature_type(ft: ModelFeatureType) -> crate::database::entity::FeatureType {
    match ft {
        ModelFeatureType::Simple => crate::database::entity::FeatureType::Simple,
        ModelFeatureType::Contextual => crate::database::entity::FeatureType::Contextual,
    }
}

fn map_api_to_db_lifecycle_stage(stage: ModelLifecycleStage) -> &'static str {
    match stage {
        ModelLifecycleStage::Draft => "draft",
        ModelLifecycleStage::Active => "active",
        ModelLifecycleStage::Deprecated => "deprecated",
        ModelLifecycleStage::Archived => "archived",
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized = tags
        .into_iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn stale_reasons(feature: &crate::database::entity::Feature) -> Vec<String> {
    let now = Utc::now();
    let mut reasons = Vec::new();

    if feature
        .expires_at
        .is_some_and(|expires_at| expires_at < now)
    {
        reasons.push("Expired".to_string());
    }

    let lifecycle = feature.lifecycle_stage.to_lowercase();
    let lifecycle_can_stale = lifecycle == "active" || lifecycle == "deprecated";
    if lifecycle_can_stale {
        let older_than_30_days = feature.created_at < now - Duration::days(30);
        let no_recent_evaluations = feature
            .last_evaluated_at
            .map(|last| last < now - Duration::days(30))
            .unwrap_or(true);
        if older_than_30_days && no_recent_evaluations {
            reasons.push("No recent evaluations".to_string());
        }

        if feature.created_at < now - Duration::days(90) && feature.evaluation_count_90d == 0 {
            reasons.push("No evaluations in 90 days".to_string());
        }

        if feature.created_at < now - Duration::days(90) && !feature.active {
            reasons.push("Disabled for 90+ days".to_string());
        }
    }

    reasons
}

fn map_entity_to_api_feature(feature_entity: crate::database::entity::Feature) -> ModelFeature {
    let stale_reasons = stale_reasons(&feature_entity);
    ModelFeature {
        id: feature_entity.id.into(),
        key: feature_entity.key,
        description: feature_entity.description,
        feature_type: match feature_entity.feature_type {
            crate::database::entity::FeatureType::Simple => ModelFeatureType::Simple,
            crate::database::entity::FeatureType::Contextual => ModelFeatureType::Contextual,
        },
        enabled: feature_entity.active,
        created_at: feature_entity.created_at,
        kill_switch_enabled: feature_entity.kill_switch_enabled,
        kill_switch_activated_at: feature_entity.kill_switch_activated_at,
        rollback_scheduled_at: feature_entity.rollback_scheduled_at,
        emergency_override_reason: feature_entity.emergency_override_reason,
        emergency_override_expires_at: feature_entity.emergency_override_expires_at,
        emergency_override_actor_id: feature_entity.emergency_override_actor_id.map(ID::from),
        emergency_override_applied_at: feature_entity.emergency_override_applied_at,
        lifecycle_stage: match feature_entity.lifecycle_stage.to_lowercase().as_str() {
            "draft" => crate::model::LifecycleStage::Draft,
            "deprecated" => crate::model::LifecycleStage::Deprecated,
            "archived" => crate::model::LifecycleStage::Archived,
            _ => crate::model::LifecycleStage::Active,
        },
        owner: feature_entity.owner,
        purpose: feature_entity.purpose,
        reference_url: feature_entity.reference_url,
        expires_at: feature_entity.expires_at,
        cleanup_reason: feature_entity.cleanup_reason,
        tags: feature_entity.tags,
        archived_at: feature_entity.archived_at,
        deprecated_at: feature_entity.deprecated_at,
        deprecation_notice: feature_entity.deprecation_notice,
        last_evaluated_at: feature_entity.last_evaluated_at,
        evaluation_count_7d: feature_entity.evaluation_count_7d,
        evaluation_count_30d: feature_entity.evaluation_count_30d,
        evaluation_count_90d: feature_entity.evaluation_count_90d,
        is_stale: !stale_reasons.is_empty(),
        stale_reasons,
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
) -> Result<Vec<ModelContext>, Error>
where
    R: FeatureRepositoryTx + ?Sized,
{
    let stage_uuid = id_to_uuid(stage_id)?;
    let ctx_uuids: Vec<Uuid> = context_ids
        .into_iter()
        .map(id_to_uuid)
        .collect::<Result<Vec<Uuid>, _>>()?;

    let result = repo
        .set_stage_contexts_tx(conn, stage_uuid, ctx_uuids)
        .await?;

    Ok(result
        .into_iter()
        .map(|c| ModelContext {
            id: ID::from(c.id),
            team_id: ID::from(c.team_id),
            key: c.key,
            entries: c
                .entries
                .into_iter()
                .map(|e| ModelContextEntry {
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
) -> Result<Vec<ModelStageCriterion>, Error>
where
    F: FeatureRepositoryTx + ?Sized,
    V: VariantAllocationsRepositoryTx + ?Sized,
    C: CompoundRulesRepositoryTx + ?Sized,
{
    let stage_uuid = id_to_uuid(stage_id)?;

    // Map API input to DB input for criteria creation
    let db_criteria_input: Vec<CreateStageCriterion> = criteria
        .iter()
        .map(|c| {
            let mode = match c.variant_selection_mode {
                Some(crate::model::VariantSelectionMode::WeightedSplit) => {
                    crate::database::entity::VariantSelectionMode::WeightedSplit
                }
                Some(crate::model::VariantSelectionMode::SpecificVariant) => {
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
                            value: cond.value.clone(),
                            order_index: cond.order_index,
                        })
                        .collect();

                    let logic = match g.logic_operator {
                        crate::model::LogicOperator::And => {
                            crate::database::entity::LogicOperator::And
                        }
                        crate::model::LogicOperator::Or => {
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
                    crate::model::VariantSelectionMode::WeightedSplit
                }
                crate::database::entity::VariantSelectionMode::SpecificVariant => {
                    crate::model::VariantSelectionMode::SpecificVariant
                }
            };

            let allocations = c
                .variant_allocations
                .into_iter()
                .map(|a| crate::model::VariantAllocation {
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
                        .map(|cond| crate::model::CompoundRuleCondition {
                            id: ID::from(cond.id),
                            context_key: cond.context_key,
                            operator: parse_rule_operator(&cond.operator),
                            value: cond.value,
                            order_index: cond.order_index,
                        })
                        .collect();

                    let logic = match g.logic_operator {
                        crate::database::entity::LogicOperator::And => {
                            crate::model::LogicOperator::And
                        }
                        crate::database::entity::LogicOperator::Or => {
                            crate::model::LogicOperator::Or
                        }
                    };

                    crate::model::CompoundRuleGroup {
                        id: ID::from(g.id),
                        logic_operator: logic,
                        conditions,
                    }
                })
                .collect();

            ModelStageCriterion {
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
                    ModelVariantValueType::String => {
                        crate::database::entity::VariantValueType::String
                    }
                    ModelVariantValueType::Number => {
                        crate::database::entity::VariantValueType::Number
                    }
                    ModelVariantValueType::Boolean => {
                        crate::database::entity::VariantValueType::Boolean
                    }
                    ModelVariantValueType::Json => crate::database::entity::VariantValueType::Json,
                };
                (
                    variant.control,
                    variant.value,
                    value_type,
                    variant.description,
                )
            })
            .collect::<Vec<_>>()
    });

    let dependencies = input
        .dependencies
        .into_iter()
        .map(id_to_uuid)
        .collect::<Result<Vec<_>, _>>()?;

    crate::logic::dependency_graph::validate_dependency_graph_update(
        feature_repo,
        team_uuid,
        Uuid::new_v4(),
        input.key.as_str(),
        &dependencies,
    )
    .await?;

    let db_input = CreateFeature {
        team_id: team_uuid,
        key: input.key,
        description: input.description,
        feature_type: map_api_to_entity_feature_type(input.feature_type),
        lifecycle_stage: input
            .lifecycle_stage
            .map(map_api_to_db_lifecycle_stage)
            .unwrap_or("active")
            .to_string(),
        owner: normalize_optional_text(input.owner),
        purpose: normalize_optional_text(input.purpose),
        reference_url: normalize_optional_text(input.reference_url),
        expires_at: input.expires_at,
        cleanup_reason: normalize_optional_text(input.cleanup_reason),
        tags: normalize_tags(input.tags.unwrap_or_default()),
        stages,
        dependencies,
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
) -> Result<ModelFeature, Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let feature_uuid = id_to_uuid(id)?;
    let before_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;

    // Map update input
    let feature_type = Some(map_api_to_entity_feature_type(input.feature_type));

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

    let existing_feature = feature_repo.get_feature_by_id(feature_uuid).await?;
    crate::logic::dependency_graph::validate_dependency_graph_update(
        feature_repo,
        existing_feature.team_id,
        feature_uuid,
        input.key.as_str(),
        &dependencies,
    )
    .await?;

    let variants = input.variants.map(|v| {
        v.into_iter()
            .map(|variant| {
                let value_type = match variant.value_type {
                    ModelVariantValueType::String => {
                        crate::database::entity::VariantValueType::String
                    }
                    ModelVariantValueType::Number => {
                        crate::database::entity::VariantValueType::Number
                    }
                    ModelVariantValueType::Boolean => {
                        crate::database::entity::VariantValueType::Boolean
                    }
                    ModelVariantValueType::Json => crate::database::entity::VariantValueType::Json,
                };
                (
                    variant.control,
                    variant.value,
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
        lifecycle_stage: input
            .lifecycle_stage
            .map(map_api_to_db_lifecycle_stage)
            .map(str::to_string),
        owner: input
            .owner
            .map(|owner| owner.and_then(|value| normalize_optional_text(Some(value)))),
        purpose: input
            .purpose
            .map(|purpose| purpose.and_then(|value| normalize_optional_text(Some(value)))),
        reference_url: input
            .reference_url
            .map(|reference_url| reference_url.and_then(|value| normalize_optional_text(Some(value)))),
        expires_at: input.expires_at,
        cleanup_reason: input
            .cleanup_reason
            .map(|reason| reason.and_then(|value| normalize_optional_text(Some(value)))),
        tags: input.tags.map(normalize_tags),
        archive_confirmation: input.archive_confirmation,
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
        activity_type: if existing_feature.lifecycle_stage != updated_feature.lifecycle_stage {
            crate::utils::activity_logger::activity_types::FEATURE_LIFECYCLE_UPDATED.to_string()
        } else {
            crate::utils::activity_logger::activity_types::FEATURE_UPDATED.to_string()
        },
        entity_type: "feature".to_string(),
        entity_id: feature_uuid.to_string(),
        actor_id,
        actor_name: actor_name.clone(),
        description: format!("Updated feature '{}'", updated_feature.key),
        metadata: Some(serde_json::json!({
             "feature_id": feature_uuid.to_string(),
             "feature_key": updated_feature.key,
             "old_lifecycle_stage": existing_feature.lifecycle_stage,
             "new_lifecycle_stage": updated_feature.lifecycle_stage,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    let after_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let change_summary =
        diff_entries_to_json(&diff_feature_snapshots(&before_snapshot, &after_snapshot));
    feature_repo
        .create_feature_version_tx(
            conn,
            feature_uuid,
            after_snapshot,
            change_summary,
            actor_id,
            actor_name,
            "update",
        )
        .await?;

    Ok(map_entity_to_api_feature(updated_feature))
}

pub async fn rollback_feature_to_version_in_tx<R, A>(
    conn: &mut PgConnection,
    feature_repo: &R,
    activity_repo: &A,
    id: ID,
    version_id: ID,
    archive_confirmation: bool,
    actor: Option<ActorContext>,
) -> Result<ModelFeature, Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let feature_uuid = id_to_uuid(id)?;
    let version_uuid = id_to_uuid(version_id)?;
    let before_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let target_version = feature_repo
        .get_feature_version_tx(conn, feature_uuid, version_uuid)
        .await?;

    let existing_feature = feature_repo.get_feature_by_id(feature_uuid).await?;
    let target_key = snapshot_feature_key(&target_version.snapshot)?;
    let dependencies = snapshot_dependencies(&target_version.snapshot)?;
    crate::logic::dependency_graph::validate_dependency_graph_update(
        feature_repo,
        existing_feature.team_id,
        feature_uuid,
        target_key.as_str(),
        &dependencies,
    )
    .await?;

    let restored_feature = feature_repo
        .restore_feature_snapshot_tx(
            conn,
            feature_uuid,
            target_version.snapshot.clone(),
            archive_confirmation,
        )
        .await?;

    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    let activity = CreateActivityLog {
        activity_type: crate::utils::activity_logger::activity_types::FEATURE_UPDATED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature_uuid.to_string(),
        actor_id,
        actor_name: actor_name.clone(),
        description: format!(
            "Rolled back feature '{}' to version {}",
            restored_feature.key, target_version.version_number
        ),
        metadata: Some(serde_json::json!({
            "feature_id": feature_uuid.to_string(),
            "feature_key": restored_feature.key,
            "target_version_id": version_uuid.to_string(),
            "target_version_number": target_version.version_number,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    let after_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let change_summary =
        diff_entries_to_json(&diff_feature_snapshots(&before_snapshot, &after_snapshot));
    feature_repo
        .create_feature_version_tx(
            conn,
            feature_uuid,
            after_snapshot,
            change_summary,
            actor_id,
            actor_name,
            "rollback",
        )
        .await?;

    Ok(map_entity_to_api_feature(restored_feature))
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

pub async fn emergency_disable_feature_in_tx<R, A>(
    conn: &mut PgConnection,
    feature_repo: &R,
    activity_repo: &A,
    id: ID,
    rollback_in_minutes: Option<i32>,
    reason: String,
    expires_at: Option<chrono::DateTime<Utc>>,
    actor: Option<ActorContext>,
) -> Result<ModelFeature, Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let feature_uuid = id_to_uuid(id)?;
    let before_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));
    let feature = feature_repo
        .emergency_disable_feature_tx(
            conn,
            feature_uuid,
            rollback_in_minutes,
            reason.clone(),
            expires_at,
            actor_id,
        )
        .await?;

    let log_message = match rollback_in_minutes {
        Some(minutes) if minutes > 0 => {
            let scheduled = feature
                .rollback_scheduled_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| "unknown".to_string());
            format!(
                "Kill switch scheduled for feature '{}' at {} (in {} minutes)",
                feature.key, scheduled, minutes
            )
        }
        _ => format!("Kill switch activated for feature '{}'", feature.key),
    };

    let activity = CreateActivityLog {
        activity_type: crate::utils::activity_logger::activity_types::KILL_SWITCH_ACTIVATED
            .to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature.id.to_string(),
        actor_id,
        actor_name: actor_name.clone(),
        description: log_message,
        metadata: Some(serde_json::json!({
            "feature_id": feature.id.to_string(),
            "feature_key": feature.key.clone(),
            "rollback_in_minutes": rollback_in_minutes,
            "reason": reason,
            "old_state": {
                "kill_switch_enabled": before_snapshot
                    .get("feature")
                    .and_then(|feature| feature.get("kill_switch_enabled"))
                    .cloned(),
                "active": before_snapshot
                    .get("feature")
                    .and_then(|feature| feature.get("active"))
                    .cloned(),
            },
            "new_state": {
                "kill_switch_enabled": feature.kill_switch_enabled,
                "active": feature.active,
            },
            "expires_at": expires_at.map(|value| value.to_rfc3339()),
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    let after_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let change_summary =
        diff_entries_to_json(&diff_feature_snapshots(&before_snapshot, &after_snapshot));
    feature_repo
        .create_feature_version_tx(
            conn,
            feature_uuid,
            after_snapshot,
            change_summary,
            actor_id,
            actor_name,
            "kill_switch",
        )
        .await?;

    Ok(map_entity_to_api_feature(feature))
}

pub async fn emergency_enable_feature_in_tx<R, A>(
    conn: &mut PgConnection,
    feature_repo: &R,
    activity_repo: &A,
    id: ID,
    reason: String,
    actor: Option<ActorContext>,
) -> Result<ModelFeature, Error>
where
    R: FeatureRepositoryTx + ?Sized,
    A: ActivityLogRepository + ?Sized,
{
    let feature_uuid = id_to_uuid(id)?;
    let before_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let feature = feature_repo
        .emergency_enable_feature_tx(conn, feature_uuid)
        .await?;

    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    let activity = CreateActivityLog {
        activity_type: crate::utils::activity_logger::activity_types::KILL_SWITCH_DEACTIVATED
            .to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature.id.to_string(),
        actor_id,
        actor_name: actor_name.clone(),
        description: format!(
            "Feature is enabled and kill switch deactivated for '{}'",
            feature.key
        ),
        metadata: Some(serde_json::json!({
            "feature_id": feature.id.to_string(),
            "feature_key": feature.key.clone(),
            "reason": reason,
            "old_state": {
                "kill_switch_enabled": before_snapshot
                    .get("feature")
                    .and_then(|feature| feature.get("kill_switch_enabled"))
                    .cloned(),
                "active": before_snapshot
                    .get("feature")
                    .and_then(|feature| feature.get("active"))
                    .cloned(),
            },
            "new_state": {
                "kill_switch_enabled": feature.kill_switch_enabled,
                "active": feature.active,
            },
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    let after_snapshot = feature_repo
        .build_feature_snapshot_tx(conn, feature_uuid)
        .await?;
    let change_summary =
        diff_entries_to_json(&diff_feature_snapshots(&before_snapshot, &after_snapshot));
    feature_repo
        .create_feature_version_tx(
            conn,
            feature_uuid,
            after_snapshot,
            change_summary,
            actor_id,
            actor_name,
            "kill_switch",
        )
        .await?;

    Ok(map_entity_to_api_feature(feature))
}
