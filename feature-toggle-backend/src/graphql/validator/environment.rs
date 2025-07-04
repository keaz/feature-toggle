use crate::graphql::schema::{CreateEnvironmentInput, CreateRelationshipInput, CreateStageInput, UpdateEnvironmentInput};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::environment::EnvironmentLogic;
use async_graphql::{Context, Error, Result, ID};
use std::collections::HashMap;

fn validate_relationships_and_stages(
    stages: &[CreateStageInput],
    relationships: &[CreateRelationshipInput],
) -> Result<(), Error> {
    if stages.is_empty() {
        return Err(Error::new("Pipeline must have at least one stage"));
    }

    if relationships.len() != stages.len() - 1 {
        return Err(Error::new(format!(
            "Pipeline must have at least {} relationships",
            stages.len() - 1
        )));
    }

    Ok(())
}

fn validate_duplicate_environment_and_index(stages: &[CreateStageInput]) -> Result<(), Error> {
    let mut env_map: HashMap<&ID, usize> = HashMap::new();
    let mut order_map: HashMap<i32, usize> = HashMap::new();

    for (idx, stage) in stages.iter().enumerate() {
        if env_map.contains_key(&stage.environment_id) {
            return Err(Error::new(format!(
                "Stages should not have the same environment_id: '{:?}'",
                stage.environment_id
            )));
        }

        if order_map.contains_key(&stage.order_index) {
            return Err(Error::new(format!(
                "Stages should not have the same order_index: '{}'",
                stage.order_index
            )));
        }

        env_map.insert(&stage.environment_id, idx);
        order_map.insert(stage.order_index, idx);
    }
    Ok(())
}

impl CreateInputValidator for CreateEnvironmentInput {
    async fn validate(
        &self,
        team_id: Option<ID>,
        ctx: &Context<'_>,
    ) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        let environments = logic
            .get_environments(team_id.unwrap(), Some(self.name.clone()), None)
            .await?;

        if !environments.is_empty() {
            return Err(Error::new(format!(
                "Environment with name '{:?}' already exists",
                self.name
            )));
        }

        Ok(())
    }
}

impl UpdateInputValidator for UpdateEnvironmentInput {
    async fn validate(
        &self,
        id: Option<ID>,
        ctx: &Context<'_>,
    ) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        let environment = logic.get_environment_by_id(id.clone().unwrap()).await?;
        let environments = logic
            .get_environments(environment.team_id, self.name.clone(), None)
            .await?;

        if !environments.is_empty() {
            return Err(Error::new(format!(
                "Pipeline with name '{:?}' already exists",
                self.name
            )));
        }

        Ok(())
    }
}
