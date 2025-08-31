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
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>()?;
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
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        Ok(repository.get_environments(team_id, name, active).await?)
    }

    async fn teams(&self, ctx: &Context<'_>) -> GqlResult<Vec<Team>> {
        debug!("Fetching teams");
        let repository = ctx.data::<Box<dyn TeamLogic>>()?;
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

        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        Ok(logic.get_pipelines(team_id, name, active, fields).await?)
    }

    async fn pipeline(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the Pipeline")] id: ID,
    ) -> GqlResult<Pipeline> {
        debug!("Fetching pipeline with id: {id:?}");
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        Ok(logic.get_pipeline_by_id(id).await?)
    }

    async fn feature(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature")] id: ID,
    ) -> GqlResult<Feature> {
        debug!("Fetching feature with id: {id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
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
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        Ok(logic.get_features(team_id, name, feature_type).await?)
    }

    async fn client(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the client")] id: ID,
    ) -> GqlResult<Client> {
        debug!("Fetching client with id: {id:?}");
        let logic = ctx.data::<Box<dyn ClientLogic>>()?;
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
        let logic = ctx.data::<Box<dyn ClientLogic>>()?;
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
        let logic = ctx.data::<Box<dyn ContextLogic>>()?;
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
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
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


#[cfg(test)]
mod more_query_tests {
    use super::*;
    use async_graphql::{EmptySubscription, Request, Schema};

    // Use stub for PipelineLogic (no automock) and mock for EnvironmentLogic
    use crate::logic::environment::MockEnvironmentLogic;
    use std::sync::{Arc, Mutex};

    struct StubPipelineLogic {
        pub captured_fields: Arc<Mutex<Option<Vec<String>>>>,
    }
    #[async_trait::async_trait]
    impl crate::logic::pipeline::PipelineLogic for StubPipelineLogic {
        async fn get_pipelines(&self, _team_id: ID, _name: Option<String>, _active: Option<bool>, fields: Vec<String>) -> Result<Vec<Pipeline>, crate::Error> {
            *self.captured_fields.lock().unwrap() = Some(fields);
            Ok(Vec::new())
        }
        async fn get_pipeline_by_id(&self, _id: ID) -> Result<Pipeline, crate::Error> { unreachable!() }
        async fn create_pipeline(&self, _team_id: ID, _input: crate::graphql::schema::CreatePipelineInput) -> Result<ID, crate::Error> { unreachable!() }
        async fn update_pipeline(&self, _id: ID, _input: crate::graphql::schema::UpdatePipelineInput) -> Result<Pipeline, crate::Error> { unreachable!() }
        async fn delete_pipeline(&self, _id: ID) -> Result<(), crate::Error> { unreachable!() }
        fn clone_box(&self) -> Box<dyn crate::logic::pipeline::PipelineLogic> { Box::new(Self { captured_fields: self.captured_fields.clone() }) }
    }

    struct StubUserLogic {
        items: Vec<crate::logic::user::GqlUser>,
        total: i64,
    }
    #[async_trait::async_trait]
    impl crate::logic::user::UserLogic for StubUserLogic {
        async fn get_user_by_id(&self, _id: ID) -> Result<crate::logic::user::GqlUser, crate::Error> { unreachable!() }
        async fn get_user_by_username(&self, _username: String) -> Result<crate::logic::user::GqlUser, crate::Error> { unreachable!() }
        async fn register_user(&self, _input: crate::logic::user::RegisterUserInput) -> Result<crate::logic::user::GqlUser, crate::Error> { unreachable!() }
        async fn authenticate_user(&self, _username: String, _password: String) -> Result<crate::logic::user::GqlUser, crate::Error> { unreachable!() }
        async fn update_user(&self, _id: ID, _input: crate::logic::user::UpdateGqlUserInput) -> Result<crate::logic::user::GqlUser, crate::Error> { unreachable!() }
        async fn assign_user_teams(&self, _id: ID, _team_ids: Vec<ID>) -> Result<bool, crate::Error> { unreachable!() }
        async fn search_users(&self, _team_id: Option<ID>, _name: Option<String>, _page_number: i32, _page_size: i32) -> Result<(Vec<crate::logic::user::GqlUser>, i64), crate::Error> {
            Ok((self.items.clone(), self.total))
        }
        fn clone_box(&self) -> Box<dyn crate::logic::user::UserLogic> { Box::new(Self { items: self.items.clone(), total: self.total }) }
    }

    #[tokio::test]
    async fn test_pipelines_lookahead_includes_stages_field() {
        let team_id = ID::from("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
        let captured = Arc::new(Mutex::new(None));
        let stub = StubPipelineLogic { captured_fields: captured.clone() };

        let schema = Schema::build(super::Query, crate::graphql::mutation::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::pipeline::PipelineLogic>>(Box::new(stub))
            .finish();

        let q = r#"query($tid: ID!){ pipelines(teamId: $tid){ id stages { id } } }"#;
        let mut req = Request::new(q);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({"tid": team_id.to_string()})));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "{}", serde_json::to_string(&resp.errors).unwrap());
        let fields = captured.lock().unwrap().clone().unwrap();
        assert!(fields.contains(&"stages".to_string()));
    }

    #[tokio::test]
    async fn test_users_pagination_maps_items_and_total() {
        use chrono::Utc;
        let u1 = crate::logic::user::GqlUser {
            id: ID::from("11111111-1111-1111-1111-111111111111"),
            username: "u1".into(), first_name: "F1".into(), last_name: "L1".into(),
            email: "u1@example.com".into(), is_admin: false, created_at: Utc::now(), updated_at: Utc::now(), last_login: None };
        let u2 = crate::logic::user::GqlUser {
            id: ID::from("22222222-2222-2222-2222-222222222222"),
            username: "u2".into(), first_name: "F2".into(), last_name: "L2".into(),
            email: "u2@example.com".into(), is_admin: true, created_at: Utc::now(), updated_at: Utc::now(), last_login: None };
        let stub = StubUserLogic { items: vec![u1, u2], total: 42 };

        let schema = Schema::build(super::Query, crate::graphql::mutation::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::user::UserLogic>>(Box::new(stub))
            .finish();

        let q = r#"query{ users(pageNumber: 2, pageSize: 10){ pageNumber pageSize total items { username isAdmin } } }"#;
        let resp = schema.execute(Request::new(q)).await;
        assert!(resp.errors.is_empty(), "{}", serde_json::to_string(&resp.errors).unwrap());
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["users"]["pageNumber"], 2);
        assert_eq!(data["users"]["pageSize"], 10);
        assert_eq!(data["users"]["total"], 42);
        let items = data["users"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["username"], "u1");
        assert_eq!(items[1]["isAdmin"], true);
    }

    #[tokio::test]
    async fn test_environment_query_calls_logic() {
        let mut mock = MockEnvironmentLogic::new();
        let env_id = ID::from("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
        let expected = Environment { id: env_id.clone(), name: "prod".into(), active: true, team_id: ID::from("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb") };
        let env_id_for_check = env_id.clone();
        mock.expect_get_environment_by_id()
            .times(1)
            .withf(move |id| id.to_string() == env_id_for_check.to_string())
            .return_once(move |_| Ok(expected));

        let schema = Schema::build(super::Query, crate::graphql::mutation::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::environment::EnvironmentLogic>>(Box::new(mock))
            .finish();

        let q = r#"query($id: ID!){ environment(id: $id){ id name active } }"#;
        let mut req = Request::new(q);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({"id": env_id.to_string()})));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty());
    }
}
