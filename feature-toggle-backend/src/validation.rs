use std::collections::{HashMap, HashSet};

use crate::model::{CreateRelationshipInput, StageInput};

pub fn validate_relationships_and_stages<T: StageInput + 'static>(
    stages: &[T],
    relationships: &[CreateRelationshipInput],
) -> Result<(), String> {
    if stages.is_empty() {
        return Err("Pipeline must have at least one stage".to_string());
    }

    if relationships.len() != stages.len().saturating_sub(1) {
        return Err(format!(
            "Pipeline must have at least {} relationships",
            stages.len().saturating_sub(1)
        ));
    }

    Ok(())
}

pub fn validate_duplicate_environment_and_index<T: StageInput + 'static>(
    stages: &[T],
) -> Result<(), String> {
    let mut env_map: HashMap<&crate::model::ID, usize> = HashMap::new();
    let mut order_map: HashMap<i32, usize> = HashMap::new();

    for (idx, stage) in stages.iter().enumerate() {
        if env_map.contains_key(stage.environment_id()) {
            return Err(format!(
                "Stages should not have the same environment_id: '{:?}'",
                stage.environment_id()
            ));
        }

        if order_map.contains_key(&stage.order_index()) {
            return Err(format!(
                "Stages should not have the same order_index: '{}'",
                stage.order_index()
            ));
        }

        env_map.insert(stage.environment_id(), idx);
        order_map.insert(stage.order_index(), idx);
    }

    Ok(())
}

pub fn validate_stage_transition(current: &str, next: &str) -> Result<(), String> {
    let cur = current.to_uppercase();
    let nxt = next.to_uppercase();

    let mut allowed: std::collections::HashMap<&str, HashSet<&str>> = HashMap::new();
    allowed.insert("NOT_DEPLOYED", HashSet::from(["DEPLOYMENT_REQUESTED"]));
    allowed.insert(
        "DEPLOYMENT_REQUESTED",
        HashSet::from(["DEPLOYMENT_REJECTED", "DEPLOYMENT_APPROVED"]),
    );
    allowed.insert("DEPLOYMENT_APPROVED", HashSet::from(["DEPLOYED"]));
    allowed.insert(
        "DEPLOYMENT_REJECTED",
        HashSet::from(["DEPLOYMENT_REQUESTED"]),
    );
    allowed.insert("DEPLOYED", HashSet::from(["ROLLBACK_REQUESTED"]));
    allowed.insert(
        "ROLLBACK_REQUESTED",
        HashSet::from(["ROLLBACK_REJECTED", "ROLLBACK_APPROVED"]),
    );
    allowed.insert("ROLLBACK_APPROVED", HashSet::from(["ROLLBACKED"]));
    allowed.insert("ROLLBACK_REJECTED", HashSet::from(["ROLLBACK_REQUESTED"]));
    allowed.insert("ROLLBACKED", HashSet::from(["DEPLOYMENT_REQUESTED"]));

    if let Some(nexts) = allowed.get(cur.as_str()) {
        if nexts.contains(nxt.as_str()) {
            return Ok(());
        }
        return Err(format!("Invalid transition: {} -> {}", cur, nxt));
    }

    Err(format!("Unknown current status: {}", cur))
}
