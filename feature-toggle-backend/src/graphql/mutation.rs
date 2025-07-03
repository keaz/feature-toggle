use crate::graphql::schema::{CreateEnvironmentInput, CreateFeatureInput, CreatePipelineInput, CreateTeamInput, Environment, Feature, Pipeline, Team, UpdateEnvironmentInput, UpdateFeatureInput, UpdatePipelineInput};
use crate::graphql::validator::InputValidator;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use async_graphql::{Context, Object, Result as GqlResult, ID};
use log::info;
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
        info!("Updating environment with id: {id:?} and input: {input:?}");
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(logic.update_environment(id, input).await?)
    }

    async fn delete_environment(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting environment with id: {id:?}");
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
        #[graphql(desc = "Team id")]
        team_id: ID,
        input: CreatePipelineInput,
    ) -> GqlResult<ID> {
        info!("Creating pipeline with input: {input:?}");
        input.validate(None, Some(team_id.clone()), ctx).await?;
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        let pipeline_id = logic.create_pipeline(team_id, input).await?;
        Ok(pipeline_id)
    }

    async fn update_pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Team id")] team_id: ID,
        #[graphql(desc = "Id of the current pipeline")] id: ID,
        input: UpdatePipelineInput,
    ) -> GqlResult<Pipeline> {
        info!("Updating pipeline with input: {input:?}");
        input.validate(Some(id.clone()), Some(team_id), ctx).await?;
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        let pipeline = logic.update_pipeline(id, input).await?;
        Ok(pipeline)
    }

    async fn delete_pipeline(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> GqlResult<bool> {
        info!("Deleting pipeline with id: {:?}", id);
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        logic.delete_pipeline(id).await?;
        Ok(true)
    }

    async fn create_feature(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateFeatureInput,
    ) -> GqlResult<ID> {
        info!("Creating feature with input: {:?}", input);
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        let feature_id = logic.create_feature(team_id, input).await?;
        Ok(feature_id)
    }

    async fn update_feature(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateFeatureInput,
    ) -> GqlResult<Feature> {
        info!("Updating feature with input: {:?}", input);
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        let feature = logic.update_feature(id, input).await?;
        Ok(feature)
    }

    async fn delete_feature(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> GqlResult<bool> {
        info!("Deleting feature with id: {:?}", id);
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        logic.delete_feature(id).await?;
        Ok(true)
    }
}
