use crate::Error;
use crate::database::feature::FeatureRepository;
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

pub async fn validate_dependency_graph_update<R>(
    repo: &R,
    team_id: Uuid,
    target_feature_id: Uuid,
    target_feature_key: &str,
    new_dependencies: &[Uuid],
) -> Result<(), Error>
where
    R: FeatureRepository + ?Sized,
{
    let team_features = repo.get_features(team_id, None, None).await?;

    let mut feature_keys: HashMap<Uuid, String> = team_features
        .iter()
        .map(|feature| (feature.id, feature.key.clone()))
        .collect();
    feature_keys
        .entry(target_feature_id)
        .or_insert_with(|| target_feature_key.to_string());

    let mut deduped_dependencies = Vec::with_capacity(new_dependencies.len());
    let mut seen_dependencies = HashSet::new();
    for dependency_id in new_dependencies {
        if *dependency_id == target_feature_id {
            return Err(Error::InvalidInput(format!(
                "Feature '{target_feature_key}' cannot depend on itself"
            )));
        }

        if !feature_keys.contains_key(dependency_id) {
            return Err(Error::InvalidInput(format!(
                "Dependency '{dependency_id}' is not part of team '{team_id}' for feature '{target_feature_key}'"
            )));
        }

        if seen_dependencies.insert(*dependency_id) {
            deduped_dependencies.push(*dependency_id);
        }
    }

    let mut adjacency: HashMap<Uuid, Vec<Uuid>> = team_features
        .iter()
        .map(|feature| {
            (
                feature.id,
                feature
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.depends_on_id)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    adjacency.insert(target_feature_id, deduped_dependencies);

    if let Some(cycle) = detect_cycle(&adjacency) {
        let cycle_path = cycle
            .iter()
            .map(|id| {
                feature_keys
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| id.to_string())
            })
            .collect::<Vec<_>>()
            .join(" -> ");

        return Err(Error::InvalidInput(format!(
            "Dependency cycle detected: {cycle_path}"
        )));
    }

    Ok(())
}

pub async fn ensure_rollout_dependencies_safe<R>(
    repo: &R,
    feature_id: Uuid,
    environment_id: Uuid,
) -> Result<(), Error>
where
    R: FeatureRepository + ?Sized,
{
    let root_feature = repo.get_feature_by_id(feature_id).await?;
    let team_features = repo.get_features(root_feature.team_id, None, None).await?;

    let mut feature_map: HashMap<Uuid, crate::database::entity::Feature> = team_features
        .into_iter()
        .map(|feature| (feature.id, feature))
        .collect();
    feature_map
        .entry(root_feature.id)
        .or_insert_with(|| root_feature.clone());

    let adjacency: HashMap<Uuid, Vec<Uuid>> = feature_map
        .iter()
        .map(|(id, feature)| {
            (
                *id,
                feature
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.depends_on_id)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    if let Some(cycle) = detect_cycle(&adjacency) {
        let cycle_path = cycle
            .iter()
            .map(|id| {
                feature_map
                    .get(id)
                    .map(|feature| feature.key.clone())
                    .unwrap_or_else(|| id.to_string())
            })
            .collect::<Vec<_>>()
            .join(" -> ");

        return Err(Error::InvalidInput(format!(
            "Rollout blocked due to dependency cycle: {cycle_path}"
        )));
    }

    let root = feature_map
        .get(&feature_id)
        .ok_or_else(|| Error::NotFound(feature_id))?;

    let mut visited_dependencies = HashSet::new();
    let mut queue: VecDeque<(Uuid, Uuid)> = root
        .dependencies
        .iter()
        .map(|dependency| (root.id, dependency.depends_on_id))
        .collect();

    while let Some((parent_id, dependency_id)) = queue.pop_front() {
        if !visited_dependencies.insert(dependency_id) {
            continue;
        }

        let parent_key = feature_map
            .get(&parent_id)
            .map(|feature| feature.key.as_str())
            .unwrap_or("unknown-feature");

        let dependency_feature = feature_map.get(&dependency_id).ok_or_else(|| {
            Error::InvalidInput(format!(
                "Rollout blocked: '{parent_key}' depends on missing feature '{dependency_id}'"
            ))
        })?;

        if !dependency_feature.active || !dependency_feature.kill_switch_enabled {
            return Err(Error::InvalidInput(format!(
                "Rollout blocked: '{parent_key}' depends on '{}' which is disabled",
                dependency_feature.key
            )));
        }

        let dependency_stage = repo
            .get_feature_stages(dependency_id)
            .await?
            .into_iter()
            .find(|stage| stage.environment_id == environment_id);

        match dependency_stage {
            None => {
                return Err(Error::InvalidInput(format!(
                    "Rollout blocked: '{parent_key}' depends on '{}' which has no stage in environment '{environment_id}'",
                    dependency_feature.key
                )));
            }
            Some(stage) if !stage.enabled => {
                return Err(Error::InvalidInput(format!(
                    "Rollout blocked: '{parent_key}' depends on '{}' which is not deployed in environment '{environment_id}' (status '{}')",
                    dependency_feature.key, stage.status
                )));
            }
            Some(_) => {}
        }

        for nested_dependency in &dependency_feature.dependencies {
            queue.push_back((dependency_id, nested_dependency.depends_on_id));
        }
    }

    Ok(())
}

fn detect_cycle(adjacency: &HashMap<Uuid, Vec<Uuid>>) -> Option<Vec<Uuid>> {
    let mut nodes: HashSet<Uuid> = adjacency.keys().copied().collect();
    for dependencies in adjacency.values() {
        nodes.extend(dependencies.iter().copied());
    }

    let mut states: HashMap<Uuid, u8> = HashMap::new();
    let mut stack: Vec<Uuid> = Vec::new();

    for node in nodes {
        if states.get(&node).copied().unwrap_or(0) != 0 {
            continue;
        }

        if let Some(cycle) = detect_cycle_from(node, adjacency, &mut states, &mut stack) {
            return Some(cycle);
        }
    }

    None
}

fn detect_cycle_from(
    node: Uuid,
    adjacency: &HashMap<Uuid, Vec<Uuid>>,
    states: &mut HashMap<Uuid, u8>,
    stack: &mut Vec<Uuid>,
) -> Option<Vec<Uuid>> {
    states.insert(node, 1);
    stack.push(node);

    if let Some(dependencies) = adjacency.get(&node) {
        for dependency in dependencies {
            match states.get(dependency).copied().unwrap_or(0) {
                0 => {
                    if let Some(cycle) = detect_cycle_from(*dependency, adjacency, states, stack) {
                        return Some(cycle);
                    }
                }
                1 => {
                    if let Some(start_index) = stack.iter().position(|id| id == dependency) {
                        let mut cycle = stack[start_index..].to_vec();
                        cycle.push(*dependency);
                        return Some(cycle);
                    }
                }
                _ => {}
            }
        }
    }

    stack.pop();
    states.insert(node, 2);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::entity::{
        Feature as EntityFeature, FeatureDependency, FeaturePipelineStage, FeatureType,
    };
    use crate::database::feature::MockFeatureRepository;
    use chrono::Utc;

    fn mk_feature(
        id: Uuid,
        key: &str,
        team_id: Uuid,
        active: bool,
        kill_switch_enabled: bool,
        dependencies: Vec<Uuid>,
    ) -> EntityFeature {
        EntityFeature {
            id,
            key: key.to_string(),
            description: None,
            feature_type: FeatureType::Simple,
            team_id,
            active,
            created_at: Utc::now(),
            kill_switch_enabled,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: dependencies
                .into_iter()
                .map(|depends_on_id| FeatureDependency {
                    feature_id: id,
                    depends_on_id,
                })
                .collect(),
        }
    }

    fn mk_stage(feature_id: Uuid, environment_id: Uuid, enabled: bool) -> FeaturePipelineStage {
        FeaturePipelineStage {
            id: Uuid::new_v4(),
            feature_id,
            environment_id,
            order_index: 0,
            parent_stage_id: None,
            position: "{\"x\":0,\"y\":0}".to_string(),
            enabled,
            status: if enabled {
                "DEPLOYED".to_string()
            } else {
                "NOT_DEPLOYED".to_string()
            },
        }
    }

    #[tokio::test]
    async fn validate_dependency_graph_update_rejects_cycle() {
        let mut repo = MockFeatureRepository::new();
        let team_id = Uuid::new_v4();
        let feature_a_id = Uuid::new_v4();
        let feature_b_id = Uuid::new_v4();

        let feature_a = mk_feature(
            feature_a_id,
            "feature-a",
            team_id,
            true,
            true,
            vec![feature_b_id],
        );
        let feature_b = mk_feature(feature_b_id, "feature-b", team_id, true, true, vec![]);

        repo.expect_get_features()
            .times(1)
            .returning(move |_, _, _| Ok(vec![feature_a.clone(), feature_b.clone()]));

        let result = validate_dependency_graph_update(
            &repo,
            team_id,
            feature_b_id,
            "feature-b",
            &[feature_a_id],
        )
        .await;

        assert!(result.is_err());
        let error = result.err().expect("expected cycle validation error");
        match error {
            Error::InvalidInput(message) => {
                assert!(message.contains("Dependency cycle detected"));
                assert!(message.contains("feature-a"));
                assert!(message.contains("feature-b"));
            }
            other => panic!("expected invalid input error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ensure_rollout_dependencies_safe_blocks_disabled_dependency() {
        let mut repo = MockFeatureRepository::new();
        let team_id = Uuid::new_v4();
        let root_id = Uuid::new_v4();
        let dependency_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();

        let root = mk_feature(
            root_id,
            "feature-root",
            team_id,
            true,
            true,
            vec![dependency_id],
        );
        let dependency = mk_feature(
            dependency_id,
            "feature-dependency",
            team_id,
            false,
            false,
            vec![],
        );

        let root_for_get = root.clone();
        repo.expect_get_feature_by_id()
            .times(1)
            .returning(move |_| Ok(root_for_get.clone()));

        repo.expect_get_features()
            .times(1)
            .returning(move |_, _, _| Ok(vec![root.clone(), dependency.clone()]));

        repo.expect_get_feature_stages().never();

        let result = ensure_rollout_dependencies_safe(&repo, root_id, environment_id).await;

        assert!(result.is_err());
        let error = result.err().expect("expected rollout to be blocked");
        match error {
            Error::InvalidInput(message) => {
                assert!(message.contains("Rollout blocked"));
                assert!(message.contains("feature-dependency"));
                assert!(message.contains("disabled"));
            }
            other => panic!("expected invalid input error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ensure_rollout_dependencies_safe_allows_enabled_dependency_chain() {
        let mut repo = MockFeatureRepository::new();
        let team_id = Uuid::new_v4();
        let root_id = Uuid::new_v4();
        let dep_a_id = Uuid::new_v4();
        let dep_b_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();

        let root = mk_feature(root_id, "feature-root", team_id, true, true, vec![dep_a_id]);
        let dependency_a = mk_feature(dep_a_id, "feature-a", team_id, true, true, vec![dep_b_id]);
        let dependency_b = mk_feature(dep_b_id, "feature-b", team_id, true, true, vec![]);

        let root_for_get = root.clone();
        repo.expect_get_feature_by_id()
            .times(1)
            .returning(move |_| Ok(root_for_get.clone()));

        repo.expect_get_features()
            .times(1)
            .returning(move |_, _, _| {
                Ok(vec![
                    root.clone(),
                    dependency_a.clone(),
                    dependency_b.clone(),
                ])
            });

        repo.expect_get_feature_stages()
            .times(2)
            .returning(move |feature_id| Ok(vec![mk_stage(feature_id, environment_id, true)]));

        let result = ensure_rollout_dependencies_safe(&repo, root_id, environment_id).await;
        assert!(result.is_ok(), "dependency chain should be rollout-safe");
    }
}
