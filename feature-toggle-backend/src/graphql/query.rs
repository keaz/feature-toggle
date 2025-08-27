use crate::graphql::create_user;
use crate::graphql::schema::{
    Client, ClientType, Environment, Feature, FeatureType, Pipeline, Team, User, UsersPage,
};
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::team::TeamLogic;
use crate::logic::user::UserLogic;
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
        Ok(logic
            .get_clients(team_id, name, enabled, client_type)
            .await?)
    }

    async fn context(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the context")] id: ID,
    ) -> GqlResult<crate::graphql::schema::Context> {
        debug!("Fetching context with id: {id:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        Ok(logic.get_context_by_id(id).await?)
    }

    async fn contexts(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the team")] team_id: ID,
        #[graphql(desc = "Filter by key (ILIKE)")] key: Option<String>,
    ) -> GqlResult<Vec<crate::graphql::schema::Context>> {
        debug!("Fetching contexts for team with id: {team_id:?} key={key:?}");
        let logic = ctx.data::<Box<dyn ContextLogic>>().unwrap();
        Ok(logic.get_contexts(team_id, key).await?)
    }

    async fn stage_contexts(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
    ) -> GqlResult<Vec<crate::graphql::schema::Context>> {
        debug!("Fetching contexts for stage id: {stage_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.get_stage_contexts(stage_id).await?)
    }

    async fn get_stage_criteria(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
    ) -> GqlResult<Vec<crate::graphql::schema::StageCriterion>> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.get_stage_criteria(stage_id).await?)
    }

    // Users
    async fn user(&self, ctx: &Context<'_>, id: ID) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let u = logic.get_user_by_id(id).await?;
        create_user(u)
    }

    async fn user_by_username(&self, ctx: &Context<'_>, username: String) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let u = logic.get_user_by_username(username).await?;
        create_user(u)
    }

    async fn users(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Filter by team id")] team_id: Option<ID>,
        #[graphql(desc = "Search by first/last/username (ILIKE)")] name: Option<String>,
        #[graphql(desc = "Page number (1-based)")] page_number: i32,
        #[graphql(desc = "Page size")] page_size: i32,
    ) -> GqlResult<UsersPage> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let (items, total) = logic
            .search_users(team_id, name, page_number, page_size)
            .await?;
        let items: Vec<User> = items
            .into_iter()
            .map(create_user)
            .collect::<Result<_, _>>()?;
        Ok(UsersPage { items, page_number, page_size, total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::context::MockContextLogic;
    use async_graphql::{EmptySubscription, Request, Schema};

    #[tokio::test]
    async fn test_contexts_query() {
        let mut mock = MockContextLogic::new();
        let team_id = ID::from("11111111-1111-1111-1111-111111111111");
        let expected = vec![crate::graphql::schema::Context {
            id: ID::from("22222222-2222-2222-2222-222222222222"),
            team_id: team_id.clone(),
            key: "country".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from("33333333-3333-3333-3333-333333333333"),
                value: "US".into(),
            }],
        }];
        let team_id_clone = team_id.clone();
        mock.expect_get_contexts()
            .times(1)
            .withf(move |tid, key| tid == &team_id_clone && key.is_none())
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::context::ContextLogic>>(Box::new(mock))
        .finish();

        let gql = r#"
            query($team: ID!) {
                contexts(teamId: $team) { key entries { value } }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "team": team_id.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["contexts"].as_array().unwrap().len(), 1);
        assert_eq!(data["contexts"][0]["key"], "country");
    }

    #[tokio::test]
    async fn test_stage_contexts_query() {
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from("11111111-1111-1111-1111-111111111111");
        let expected = vec![crate::graphql::schema::Context {
            id: ID::from("22222222-2222-2222-2222-222222222222"),
            team_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
            key: "alpha".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from("33333333-3333-3333-3333-333333333333"),
                value: "X".into(),
            }],
        }];
        let stage_id_clone = stage_id.clone();
        mock.expect_get_stage_contexts()
            .times(1)
            .withf(move |sid| sid == &stage_id_clone)
            .return_once(move |_| Ok(expected.clone()));

        let schema = Schema::build(
            super::Query,
            crate::graphql::mutation::MutationRoot,
            EmptySubscription,
        )
        .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
        .finish();

        let gql = r#"
            query($sid: ID!) {
                stageContexts(stageId: $sid) { key entries { value } }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["stageContexts"].as_array().unwrap().len(), 1);
        assert_eq!(data["stageContexts"][0]["key"], "alpha");
    }
}
