use crate::graphql::schema::{CreateFeatureInput, FeatureType, UpdateFeatureInput};
use crate::graphql::validator::{
    validate_duplicate_environment_and_index, validate_relationships_and_stages, CreateInputValidator,
    UpdateInputValidator,
};
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use async_graphql::{Context, Error, Result, ID};

impl CreateInputValidator for CreateFeatureInput {
    async fn validate(&self, team_id: Option<ID>, ctx: &Context<'_>) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipelines = logic
            .get_pipelines(team_id.unwrap(), Some(self.key.clone()), Some(true), vec![])
            .await?;
        if !pipelines.is_empty() {
            return Err(Error::new(format!(
                "Feature with name '{}' already exists",
                self.key
            )));
        }

        validate_relationships_and_stages(&self.stages, &self.relationships)?;
        validate_duplicate_environment_and_index(&self.stages)?;

        if self.feature_type == FeatureType::Contextual {
            //# TODO: validate stage context
        }

        Ok(())
    }
}

impl UpdateInputValidator for UpdateFeatureInput {
    async fn validate(&self, id: Option<ID>, ctx: &Context<'_>) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature = logic.get_feature_by_id(id.clone().unwrap()).await?;
        let pipelines = logic
            .get_features(feature.team_id, Some(self.key.clone()), None)
            .await?;

        if !pipelines.is_empty() && pipelines.iter().any(|p| p.id != id.clone().unwrap()) {
            return Err(Error::new(format!(
                "Feature with name '{:?}' already exists",
                self.key
            )));
        }

        validate_relationships_and_stages(&self.stages, &self.relationships)?;
        validate_duplicate_environment_and_index(&self.stages)?;

        Ok(())
    }
}

// State transition validation for feature stage deployment workflow
// Allowed transitions:
// NOT_DEPLOYED -> DEPLOYMENT_REQUESTED
// DEPLOYMENT_REQUESTED -> DEPLOYMENT_REJECTED | DEPLOYED
// DEPLOYMENT_REJECTED -> DEPLOYMENT_REQUESTED
// DEPLOYED -> ROLLBACK_REQUESTED
// ROLLBACK_REQUESTED -> ROLLBACK_REJECTED | ROLLBACKED
// ROLLBACK_REJECTED -> ROLLBACK_REQUESTED
// ROLLBACKED -> DEPLOYMENT_REQUESTED
pub fn validate_stage_transition(current: &str, next: &str) -> Result<(), Error> {
    use std::collections::HashSet;
    // Normalize to uppercase to be safe
    let cur = current.to_uppercase();
    let nxt = next.to_uppercase();

    // Define allowed next states for each state
    let mut allowed: std::collections::HashMap<&str, HashSet<&str>> = std::collections::HashMap::new();
    allowed.insert("NOT_DEPLOYED", HashSet::from(["DEPLOYMENT_REQUESTED"]));
    allowed.insert("DEPLOYMENT_REQUESTED", HashSet::from(["DEPLOYMENT_REJECTED", "DEPLOYED"]));
    allowed.insert("DEPLOYMENT_REJECTED", HashSet::from(["DEPLOYMENT_REQUESTED"]));
    allowed.insert("DEPLOYED", HashSet::from(["ROLLBACK_REQUESTED"]));
    allowed.insert("ROLLBACK_REQUESTED", HashSet::from(["ROLLBACK_REJECTED", "ROLLBACKED"]));
    allowed.insert("ROLLBACK_REJECTED", HashSet::from(["ROLLBACK_REQUESTED"]));
    allowed.insert("ROLLBACKED", HashSet::from(["DEPLOYMENT_REQUESTED"]));

    if let Some(nexts) = allowed.get(cur.as_str()) {
        if nexts.contains(nxt.as_str()) {
            return Ok(());
        }
        return Err(Error::new(format!(
            "Invalid transition: {} -> {}",
            cur, nxt
        )));
    }
    Err(Error::new(format!(
        "Unknown current status: {}",
        cur
    )))
}

#[cfg(test)]
mod tests {
    use super::validate_stage_transition;

    #[test]
    fn test_allowed_transitions() {
        assert!(validate_stage_transition("NOT_DEPLOYED", "DEPLOYMENT_REQUESTED").is_ok());
        assert!(validate_stage_transition("DEPLOYMENT_REQUESTED", "DEPLOYMENT_REJECTED").is_ok());
        assert!(validate_stage_transition("DEPLOYMENT_REQUESTED", "DEPLOYED").is_ok());
        assert!(validate_stage_transition("DEPLOYMENT_REJECTED", "DEPLOYMENT_REQUESTED").is_ok());
        assert!(validate_stage_transition("DEPLOYED", "ROLLBACK_REQUESTED").is_ok());
        assert!(validate_stage_transition("ROLLBACK_REQUESTED", "ROLLBACK_REJECTED").is_ok());
        assert!(validate_stage_transition("ROLLBACK_REQUESTED", "ROLLBACKED").is_ok());
        assert!(validate_stage_transition("ROLLBACK_REJECTED", "ROLLBACK_REQUESTED").is_ok());
        assert!(validate_stage_transition("ROLLBACKED", "DEPLOYMENT_REQUESTED").is_ok());
    }

    #[test]
    fn test_invalid_transitions() {
        assert!(validate_stage_transition("NOT_DEPLOYED", "DEPLOYED").is_err());
        assert!(validate_stage_transition("DEPLOYED", "DEPLOYMENT_REQUESTED").is_err());
        assert!(validate_stage_transition("DEPLOYMENT_REJECTED", "DEPLOYED").is_err());
        assert!(validate_stage_transition("ROLLBACKED", "DEPLOYED").is_err());
        assert!(validate_stage_transition("ROLLBACK_REJECTED", "DEPLOYED").is_err());
    }
}
