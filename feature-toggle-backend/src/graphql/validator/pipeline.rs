use crate::graphql::schema::{CreatePipelineInput, UpdatePipelineInput};
use crate::graphql::validator::{
    CreateInputValidator, UpdateInputValidator, validate_duplicate_environment_and_index,
    validate_relationships_and_stages,
};
use crate::logic::pipeline::PipelineLogic;
use async_graphql::{Context, Error, ID, Result};

impl CreateInputValidator for CreatePipelineInput {
    async fn validate(&self, team_id: Option<ID>, ctx: &Context<'_>) -> Result<(), Error> {
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
    async fn validate(&self, id: Option<ID>, ctx: &Context<'_>) -> Result<(), Error> {
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
