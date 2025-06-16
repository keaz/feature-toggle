use crate::graphql::schema::{CreateEnvironmentInput, CreatePipelineInput, CreateTeamInput, Environment, Pipeline, Team, UpdateEnvironmentInput};
use crate::logic::environment::EnvironmentLogic;
use async_graphql::{Context, Object, Result as GqlResult, ID};
use log::{debug, info};
use uuid::Uuid;

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_environment(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateEnvironmentInput,
    ) -> GqlResult<Environment> {
        info!("Creating environment with input: {:?}", input);
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(logic.create_environment(team_id, input).await?)
    }

    async fn update_environment(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateEnvironmentInput,
    ) -> GqlResult<Environment> {
        info!("Updating environment with id: {:?} and input: {:?}", id, input);
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(logic.update_environment(id, input).await?)
    }

    async fn delete_environment(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting environment with id: {:?}", id);
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        logic.delete_environment(id).await?;
        Ok(true)
    }

    async fn create_team(&self, input: CreateTeamInput) -> GqlResult<Team> {
        let id = ID::from(Uuid::new_v4().to_string());
        Ok(Team {
            id,
            name: input.name,
            description: input.description,
        })
    }

    async fn create_pipeline(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreatePipelineInput,
    ) -> GqlResult<Pipeline> {
        info!("Creating pipeline with input: {:?}", input);

        panic!("Implement")
    }
}
