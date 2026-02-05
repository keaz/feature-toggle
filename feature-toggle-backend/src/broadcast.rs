use crate::database::feature::FeatureRepository;

pub async fn map_db_feature_to_full_for_broadcast(
    repo: &dyn FeatureRepository,
    f: crate::database::entity::Feature,
) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
    use crate::grpc::pb;

    let stages = repo.get_feature_stages(f.id).await?;
    let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(stages.len());
    for s in stages.iter() {
        let crits = repo.get_stage_criteria(s.id).await?;
        let criterias = crits
            .into_iter()
            .map(|c| {
                let rule_groups = c
                    .rule_groups
                    .into_iter()
                    .map(|group| pb::RuleGroup {
                        id: group.id.to_string(),
                        logic_operator: match group.logic_operator {
                            crate::database::entity::LogicOperator::And => "AND".to_string(),
                            crate::database::entity::LogicOperator::Or => "OR".to_string(),
                        },
                        conditions: group
                            .conditions
                            .into_iter()
                            .map(|cond| pb::RuleCondition {
                                id: cond.id.to_string(),
                                context_key: cond.context_key,
                                operator: cond.operator,
                                value: cond.value.to_string(),
                                order_index: cond.order_index,
                            })
                            .collect(),
                    })
                    .collect();

                let variant_allocations = c
                    .variant_allocations
                    .into_iter()
                    .map(|alloc| pb::VariantAllocation {
                        variant_control: alloc.variant_control,
                        weight: alloc.weight,
                    })
                    .collect();

                let selection_mode = match c.variant_selection_mode {
                    crate::database::entity::VariantSelectionMode::WeightedSplit => {
                        "WEIGHTED_SPLIT"
                    }
                    crate::database::entity::VariantSelectionMode::SpecificVariant => {
                        "SPECIFIC_VARIANT"
                    }
                };

                pb::StageCriterionFull {
                    id: c.id.to_string(),
                    stage_id: c.stage_id.to_string(),
                    priority: c.priority,
                    rule_groups,
                    variant_allocations,
                    variant_selection_mode: selection_mode.to_string(),
                    selected_variant_control: c.selected_variant_control.unwrap_or_default(),
                }
            })
            .collect();

        stage_msgs.push(pb::FeatureStageFull {
            id: s.id.to_string(),
            environment_id: s.environment_id.to_string(),
            order_index: s.order_index,
            position: s.position.clone(),
            enabled: s.enabled,
            criterias,
        });
    }

    let variants = repo.get_feature_variants(f.id).await?;
    let mut variant_msgs: Vec<pb::FeatureVariant> = Vec::with_capacity(variants.len());
    for v in variants.iter() {
        variant_msgs.push(pb::FeatureVariant {
            control: v.control.clone(),
            value: v.value.to_string(),
        });
    }

    let feature_type = match f.feature_type {
        crate::database::entity::FeatureType::Simple => "Simple",
        crate::database::entity::FeatureType::Contextual => "Contextual",
    };

    Ok(pb::FeatureFull {
        id: f.id.to_string(),
        key: f.key,
        description: f.description.unwrap_or_default(),
        feature_type: feature_type.to_string(),
        team_id: f.team_id.to_string(),
        created_at: f.created_at.to_rfc3339(),
        kill_switch_enabled: f.kill_switch_enabled,
        kill_switch_activated_at: f
            .kill_switch_activated_at
            .map(|v| v.to_rfc3339())
            .unwrap_or_default(),
        rollback_scheduled_at: f
            .rollback_scheduled_at
            .map(|v| v.to_rfc3339())
            .unwrap_or_default(),
        stages: stage_msgs,
        variants: variant_msgs,
        dependencies: f
            .dependencies
            .into_iter()
            .map(|d| pb::FeatureDependencyFull {
                feature_id: d.feature_id.to_string(),
                depends_on_id: d.depends_on_id.to_string(),
            })
            .collect(),
        active: f.active,
    })
}
