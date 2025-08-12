use crate::graphql::schema::{
    CreateClientInput, CreateEnvironmentInput, CreateFeatureInput, CreatePipelineInput, CreateTeamInput, Environment,
    Feature, Pipeline, Team, UpdateClientInput, UpdateEnvironmentInput, UpdateFeatureInput, UpdatePipelineInput,
};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
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

    // Feature stage context bindings
    async fn set_stage_contexts(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
        #[graphql(desc = "List of context IDs to assign")] context_ids: Vec<ID>,
    ) -> GqlResult<Vec<crate::graphql::schema::Context>> {
        info!("Setting contexts for stage {stage_id:?}");
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.set_stage_contexts(stage_id, context_ids).await?)
    }
    

    async fn set_stage_criteria(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of the feature stage")] stage_id: ID,
        #[graphql(desc = "Criteria to assign")] criteria: Vec<crate::graphql::schema::CreateStageCriterionInput>,
    ) -> GqlResult<Vec<crate::graphql::schema::StageCriterion>> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.set_stage_criteria(stage_id, criteria).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::query::Query as GqlQuery;
    use crate::logic::context::MockContextLogic;
    use async_graphql::{EmptySubscription, Request, Schema};

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

    #[tokio::test]
    async fn test_set_stage_contexts_mutation() {
        use crate::logic::feature::MockFeatureLogic;
        use crate::graphql::query::Query as GqlQuery;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        let ctx1 = ID::from(Uuid::new_v4());
        let ctx2 = ID::from(Uuid::new_v4());
        let expected = vec![
            crate::graphql::schema::Context { id: ctx1.clone(), team_id: ID::from(Uuid::new_v4()), key: "k1".into(), entries: vec![] },
            crate::graphql::schema::Context { id: ctx2.clone(), team_id: ID::from(Uuid::new_v4()), key: "k2".into(), entries: vec![] },
        ];
        let stage_id_clone = stage_id.clone();
        let ids_for_match = vec![ctx1.clone(), ctx2.clone()];
        mock.expect_set_stage_contexts()
            .times(1)
            .withf(move |sid, ids| sid == &stage_id_clone && ids == &ids_for_match)
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($sid: ID!, $ids: [ID!]!) {
                setStageContexts(stageId: $sid, contextIds: $ids) { key }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string(),
            "ids": [ctx1.to_string(), ctx2.to_string()]
        })));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "{}", serde_json::to_string(&resp.errors).unwrap());
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["setStageContexts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_get_stage_criteria_mutation() {
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        let expected = vec![
            crate::graphql::schema::StageCriterion {
                id: ID::from(Uuid::new_v4()),
                stage_id: stage_id.clone(),
                context_key: "filter".into(),
                context: crate::graphql::schema::Context { id: ID::from(Uuid::new_v4()), team_id: ID::from(Uuid::new_v4()), key: "filter-alpha".into(), entries: vec![] },
                rollout_percentage: 50,
            }
        ];
        let stage_id_clone = stage_id.clone();
        mock.expect_get_stage_criteria()
            .times(1)
            .withf(move |sid| sid == &stage_id_clone)
            .return_once(move |_| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .finish();

        let gql = r#"
            mutation($sid: ID!) {
                getStageCriteria(stageId: $sid) { contextKey rolloutPercentage }
            }
        "#;
        let mut req = Request::new(gql);
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "{}", serde_json::to_string(&resp.errors).unwrap());
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["getStageCriteria"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_set_stage_criteria_mutation_and_validation() {
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        // success path
        let expected = vec![
            crate::graphql::schema::StageCriterion {
                id: ID::from(Uuid::new_v4()),
                stage_id: stage_id.clone(),
                context_key: "filter".into(),
                context: crate::graphql::schema::Context { id: ID::from(Uuid::new_v4()), team_id: ID::from(Uuid::new_v4()), key: "filter-alpha".into(), entries: vec![] },
                rollout_percentage: 75,
            }
        ];
        let stage_id_clone = stage_id.clone();
        mock.expect_set_stage_criteria()
            .times(1)
            .withf(move |sid, crit| sid == &stage_id_clone && crit.len() == 1 && crit[0].context_key == "filter" && crit[0].rollout_percentage == 75)
            .return_once(move |_, _| Ok(expected.clone()));

        let schema = Schema::build(GqlQuery, super::MutationRoot, EmptySubscription)
            .data::<Box<dyn crate::logic::feature::FeatureLogic>>(Box::new(mock))
            .finish();

        // success
        let gql = r#"
            mutation($sid: ID!, $cid: ID!) {
                setStageCriteria(stageId: $sid, criteria: [{ contextKey: "filter", contextId: $cid, rolloutPercentage: 75 }]) { rolloutPercentage }
            }
        "#;
        let mut req = Request::new(gql);
        let cid = ID::from(Uuid::new_v4());
        req = req.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string(),
            "cid": cid.to_string()
        })));
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "{}", serde_json::to_string(&resp.errors).unwrap());

        // validation failure for rollout_percentage
        let gql_bad = r#"
            mutation($sid: ID!, $cid: ID!) {
                setStageCriteria(stageId: $sid, criteria: [{ contextKey: "filter", contextId: $cid, rolloutPercentage: -5 }]) { rolloutPercentage }
            }
        "#;
        let mut req_bad = Request::new(gql_bad);
        req_bad = req_bad.variables(async_graphql::Variables::from_json(serde_json::json!({
            "sid": stage_id.to_string(),
            "cid": cid.to_string()
        })));
        let resp_bad = schema.execute(req_bad).await;
        assert!(!resp_bad.errors.is_empty());
    }
}
