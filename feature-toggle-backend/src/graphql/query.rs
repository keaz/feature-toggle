use crate::graphql::schema::{Client, ClientType, Environment, Feature, FeatureType, Pipeline, Team};
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::team::TeamLogic;
use crate::logic::client::ClientLogic;
use async_graphql::{Context, Object, Result as GqlResult, ID};
use log::debug;

pub struct Query;

#[Object]
impl Query {
    async fn environment(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of object")] id: ID,
    ) -> GqlResult<Environment> {
        debug!("Fetching environment with id: {id:?}");
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(repository.get_environment_by_id(id).await?)
    }

    async fn environments(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the environment")] name: Option<String>,
        #[graphql(desc = "Active status of the environment")] active: Option<bool>,
    ) -> GqlResult<Vec<Environment>> {
        debug!("Fetching environments with name: {name:?} and active: {active:?}");
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(repository.get_environments(team_id, name, active).await?)
    }

    async fn teams(&self, ctx: &Context<'_>) -> GqlResult<Vec<Team>> {
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
        debug!("Fetching pipelines for team with id: {team_id:?}");

        let mut fields = vec![];
        if ctx.look_ahead().field("stages").exists() {
            fields.push("stages".to_string());
        }

        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        Ok(logic.get_pipelines(team_id, name, active, fields).await?)
    }

    async fn pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the Pipeline")] id: ID,
    ) -> GqlResult<Pipeline> {
        debug!("Fetching pipeline with id: {id:?}");
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        Ok(logic.get_pipeline_by_id(id).await?)
    }

    async fn feature(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature")] id: ID,
    ) -> GqlResult<Feature> {
        debug!("Fetching feature with id: {id:?}");
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
        debug!("Fetching features for team with id: {team_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.get_features(team_id, name, feature_type).await?)
    }

    async fn client(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the client")] id: ID,
    ) -> GqlResult<Client> {
        debug!("Fetching client with id: {id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.get_client_by_id(id).await?)
    }

    async fn clients(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Name of the client")] name: Option<String>,
        #[graphql(desc = "Enabled status of the client")] enabled: Option<bool>,
        #[graphql(desc = "Type of the client")] client_type: Option<ClientType>,
    ) -> GqlResult<Vec<Client>> {
        debug!("Fetching clients for team with id: {team_id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.get_clients(team_id, name, enabled, client_type).await?)
    }
}
