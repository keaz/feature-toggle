use crate::graphql::schema::{Environment, Feature, FeatureType, Pipeline, Team};
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::team::TeamLogic;
use async_graphql::{Context, Object, Result as GqlResult, ID};
use log::debug;
use uuid::Uuid;

pub struct Query;

#[Object]
impl Query {
    async fn environment(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of object")] id: Uuid,
    ) -> GqlResult<Environment> {
        debug!("Fetching environment with id: {}", id);
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(repository.get_environment_by_id(id).await?)
    }

    async fn environments(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")]  team_id: ID,
        #[graphql(desc = "Name of the environment")] name: Option<String>,
        #[graphql(desc = "Active status of the environment")] active: Option<bool>,
    ) -> GqlResult<Vec<Environment>> {
        debug!("Fetching environments with name: {:?} and active: {:?}", name, active);
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(repository.get_environments(team_id, name, active).await?)
    }

    async fn teams(
        &self,
        ctx: &Context<'_>,
    ) -> GqlResult<Vec<Team>> {
        debug!("Fetching teams");
        let repository = ctx.data::<Box<dyn TeamLogic>>().unwrap();
        Ok(repository.get_teams(None).await?)
    }

    async fn pipelines(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the environment")] name: Option<String>,
        #[graphql(desc = "Active status of the environment")] active: Option<bool>,
    ) -> GqlResult<Vec<Pipeline>> {
        debug!("Fetching pipelines for team with id: {:?}", team_id);
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        Ok(logic.get_pipelines(team_id, name, active).await?)
    }

    async fn feature(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature")] id: Uuid,
    ) -> GqlResult<Feature> {
        debug!("Fetching feature with id: {}", id);
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.get_feature_by_id(id).await?)
    }

    async fn features(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the feature")] name: Option<String>,
        #[graphql(desc = "Type of the feature")] feature_type: Option<FeatureType>,
    ) -> GqlResult<Vec<Feature>> {
        debug!("Fetching features for team with id: {:?}", team_id);
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.get_features(team_id, name, feature_type).await?)
    }
}
