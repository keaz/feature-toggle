use crate::graphql::schema::{
    CreatePipelineInput, CreateRelationshipInput, CreateStageInput, UpdatePipelineInput,
};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::pipeline::PipelineLogic;
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

impl CreateInputValidator for CreatePipelineInput {
    async fn validate(
        &self,
        team_id: Option<ID>,
        ctx: &Context<'_>,
    ) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipelines = logic
            .get_pipelines(
                team_id.unwrap(),
                Some(self.name.clone()),
                Some(true),
                vec![],
            )
            .await?;
        if !pipelines.is_empty() {
            return Err(Error::new(format!(
                "Pipeline with name '{}' already exists",
                self.name
            )));
        }

        validate_relationships_and_stages(&self.stages, &self.relationships)?;
        validate_duplicate_environment_and_index(&self.stages)?;

        Ok(())
    }
}

impl UpdateInputValidator for UpdatePipelineInput {
    async fn validate(
        &self,
        id: Option<ID>,
        ctx: &Context<'_>,
    ) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipeline = logic.get_pipeline_by_id(id.clone().unwrap()).await?;
        let pipelines = logic
            .get_pipelines(pipeline.team_id, self.name.clone(), self.active, vec![])
            .await?;
        
        if !pipelines.is_empty() && pipelines.iter().any(|p| p.id != id.clone().unwrap()) {
            return Err(Error::new(format!(
                "Pipeline with name '{:?}' already exists",
                self.name
            )));
        }

        validate_relationships_and_stages(&self.stages, &self.relationships)?;
        validate_duplicate_environment_and_index(&self.stages)?;

        Ok(())
    }
}
