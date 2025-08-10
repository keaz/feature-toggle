use crate::graphql::schema::{
    CreateClientInput, CreateEnvironmentInput, CreateFeatureInput, CreatePipelineInput, CreateTeamInput, Environment,
    Feature, Pipeline, Team, UpdateClientInput, UpdateEnvironmentInput, UpdateFeatureInput, UpdatePipelineInput,
};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
use async_graphql::{Context, ID, Object, Result as GqlResult};
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
        info!("Creating environment with input: {input:?}");
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
        #[graphql(desc = "Team id")] team_id: ID,
        input: CreatePipelineInput,
    ) -> GqlResult<ID> {
        info!("Creating pipeline with input: {input:?}");
        input.validate(Some(team_id.clone()), ctx).await?;
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        let pipeline_id = logic.create_pipeline(team_id, input).await?;
        Ok(pipeline_id)
    }

    async fn update_pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the current pipeline")] id: ID,
        input: UpdatePipelineInput,
    ) -> GqlResult<Pipeline> {
        info!("Updating pipeline with input: {input:?}");
        input.validate(Some(id.clone()), ctx).await?;
        let logic = ctx.data::<Box<dyn PipelineLogic>>().unwrap();
        let pipeline = logic.update_pipeline(id, input).await?;
        Ok(pipeline)
    }

    async fn delete_pipeline(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting pipeline with id: {id:?}");
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
        info!("Creating feature with input: {input:?}");
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
        info!("Updating feature with input: {input:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        let feature = logic.update_feature(id, input).await?;
        Ok(feature)
    }

    async fn delete_feature(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting feature with id: {id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        logic.delete_feature(id).await?;
        Ok(true)
    }

    async fn create_client(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: CreateClientInput,
    ) -> GqlResult<crate::graphql::schema::Client> {
        info!("Creating client with input: {input:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.create_client(team_id, input).await?)
    }

    async fn update_client(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateClientInput,
    ) -> GqlResult<crate::graphql::schema::Client> {
        info!("Updating client with id: {id:?} and input: {input:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        Ok(logic.update_client(id, input).await?)
    }

    async fn delete_client(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting client with id: {id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>().unwrap();
        logic.delete_client(id).await?;
        Ok(true)
    }

    // Context mutations
    async fn create_context(
        &self,
        ctx: &Context<'_>,
        team_id: ID,
        input: crate::graphql::schema::CreateContextInput,
    ) -> GqlResult<crate::graphql::schema::Context> {
        info!("Creating context with key: {}", input.key);
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        Ok(logic.create_context(team_id, input).await?)
    }

    async fn update_context(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: crate::graphql::schema::UpdateContextInput,
    ) -> GqlResult<crate::graphql::schema::Context> {
        info!("Updating context with id: {id:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        Ok(logic.update_context(id, input).await?)
    }

    async fn delete_context(&self, ctx: &Context<'_>, id: ID) -> GqlResult<bool> {
        info!("Deleting context with id: {id:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        logic.delete_context(id).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::{Schema, EmptySubscription, Request};
    use crate::logic::context::MockContextLogic;
    use crate::graphql::query::Query as GqlQuery;

    #[tokio::test]
    async fn test_create_context_mutation() {
        let mut mock = MockContextLogic::new();
        let team_id = ID::from(Uuid::new_v4());
        let input = crate::graphql::schema::CreateContextInput { key: "country".into(), entries: vec!["US".into()] };
        let expected = crate::graphql::schema::Context {
            id: ID::from(Uuid::new_v4()),
            team_id: team_id.clone(),
            key: "country".into(),
            entries: vec![crate::graphql::schema::ContextEntry { id: ID::from(Uuid::new_v4()), value: "US".into() }],
        };

        let team_id_clone = team_id.clone();
        mock.expect_create_context()
            .times(1)
            .withf(move |tid, i| tid == &team_id_clone && i.key == "country" && i.entries.len() == 1)
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::context::ContextLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($team: ID!, $key: String!, $entries: [String!]!) {
                createContext(teamId: $team, input: { key: $key, entries: $entries }) {
                    key
                    entries { value }
                }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "team": team_id.to_string(),
            "key": "country",
            "entries": ["US"]
        })));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "{}", serde_json::to_string(&resp.errors).unwrap());
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createContext"]["key"], "country");
        assert_eq!(data["createContext"]["entries"].as_array().unwrap().len(), 1);
    }
}
