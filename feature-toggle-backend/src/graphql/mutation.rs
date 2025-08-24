use crate::graphql::create_user;
use crate::graphql::schema::{CreateClientInput, CreateEnvironmentInput, CreateFeatureInput, CreatePipelineInput, CreateTeamInput, Environment, Feature, LoginInput as GqlLoginInput, Pipeline, RegisterUserInput as GqlRegisterUserInput, Team, UpdateClientInput, UpdateEnvironmentInput, UpdateFeatureInput, UpdatePipelineInput, UpdateTeamInput, UpdateUserInput as GqlUpdateUserInput, User};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::client::ClientLogic;
use crate::logic::context::ContextLogic;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::team::TeamLogic;
use crate::logic::user::{GqlUser, RegisterUserInput, UpdateGqlUserInput, UserLogic};
use crate::middleware::admin_guard::AdminState;
use async_graphql::{Context, Error, Object, Result as GqlResult, ID};
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

    async fn create_team(&self, ctx: &Context<'_>, input: CreateTeamInput) -> GqlResult<Team> {
        let logic = ctx.data::<Box<dyn TeamLogic>>().unwrap();
        let team = logic.create_team(input).await?;
        Ok(team)
    }

    async fn update_team(&self, ctx: &Context<'_>, #[graphql(
        desc = "Id of the Team"
    )] id: ID, input: UpdateTeamInput) -> GqlResult<Team> {
        let logic = ctx.data::<Box<dyn TeamLogic>>().unwrap();
        let team = logic.update_team(id, input).await?;
        Ok(team)
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
        let feature = logic.update_feature(id.clone(), input).await?;

        // After successful update, publish to gRPC streaming subscribers
        if let (Ok(pool), Ok(updates_tx)) = (
            ctx.data::<sqlx::PgPool>(),
            ctx.data::<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>(),
        ) {
            // Try to load the updated feature from DB and broadcast an UPSERT
            let repo = crate::database::feature::feature_repository(pool.clone());
            if let Ok(db_feature) = repo
                .get_feature_by_id(uuid::Uuid::try_from(id.clone()).unwrap())
                .await
            {
                // Map db_feature -> pb::FeatureFull
                if let Ok(full) =
                    map_db_feature_to_full_for_broadcast(pool.clone(), db_feature).await
                {
                    let _ = updates_tx.send(crate::grpc::pb::FeatureUpdate {
                        message_id: uuid::Uuid::new_v4().to_string(),
                        action: crate::grpc::pb::feature_update::Action::Upsert as i32,
                        feature: Some(full),
                        feature_key: String::new(),
                        error: String::new(),
                    });
                }
            }
        }

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
        #[graphql(desc = "Criteria to assign")] criteria: Vec<
            crate::graphql::schema::CreateStageCriterionInput,
        >,
    ) -> GqlResult<Vec<crate::graphql::schema::StageCriterion>> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>().unwrap();
        Ok(logic.set_stage_criteria(stage_id, criteria).await?)
    }

    // User mutations
    async fn register_user(&self, ctx: &Context<'_>, input: GqlRegisterUserInput) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let created = logic.register_user(RegisterUserInput {
            username: input.username,
            password: input.password,
            first_name: input.first_name,
            last_name: input.last_name,
            email: input.email,
            is_admin: input.is_admin.unwrap_or(false),
        }).await?;

        // If an admin was created, flip the admin-exists cache so middleware stops redirecting.
        if created.is_admin && let Ok(state) = ctx.data::<AdminState>() {
            state.set_exists(true);
        }
        create_user(created)
    }

    async fn create_admin(&self, ctx: &Context<'_>, mut input: GqlRegisterUserInput) -> GqlResult<User> {
        input.is_admin = Some(true);
        self.register_user(ctx, input).await?
    }

    async fn login(&self, ctx: &Context<'_>, input: GqlLoginInput) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let u = logic.authenticate_user(input.username, input.password).await?;
        create_user(u)
    }

    async fn update_user(&self, ctx: &Context<'_>, id: ID, input: GqlUpdateUserInput) -> GqlResult<User> {
        let logic = ctx.data::<Box<dyn UserLogic>>().unwrap();
        let u = logic.update_user(id, UpdateGqlUserInput {
            first_name: input.first_name,
            last_name: input.last_name,
            email: input.email,
            is_admin: input.is_admin,
        }).await?;
        create_user(u)
    }
}


async fn map_db_feature_to_full_for_broadcast(
    pool: sqlx::PgPool,
    f: crate::database::entity::Feature,
) -> Result<crate::grpc::pb::FeatureFull, crate::Error> {
    use crate::grpc::pb;
    let repo = crate::database::feature::feature_repository(pool.clone());

    // stages with criterias
    let mut stage_msgs: Vec<pb::FeatureStageFull> = Vec::with_capacity(f.stages.len());
    for s in f.stages.iter() {
        let crits = repo.get_stage_criteria(s.id).await?;
        let criterias = crits
            .into_iter()
            .map(|c| pb::StageCriterionFull {
                id: c.id.to_string(),
                context_key: c.context_key,
                context: Some(pb::CriterionContext {
                    key: c.context.key,
                    entries: c.context.entries.into_iter().map(|e| e.value).collect(),
                }),
                rollout_percentage: c.rollout_percentage,
            })
            .collect::<Vec<_>>();

        stage_msgs.push(pb::FeatureStageFull {
            id: s.id.to_string(),
            environment_id: s.environment_id.to_string(),
            order_index: s.order_index,
            position: s.position.clone(),
            enabled: s.enabled,
            bucketing_key: s.bucketing_key.clone().unwrap_or_default(),
            criterias,
        });
    }

    let deps = f
        .dependencies
        .iter()
        .map(|d| pb::FeatureDependencyFull {
            feature_id: d.feature_id.to_string(),
            depends_on_id: d.depends_on_id.to_string(),
        })
        .collect::<Vec<_>>();

    let feature = pb::FeatureFull {
        id: f.id.to_string(),
        key: f.key,
        description: f.description.unwrap_or_default(),
        feature_type: format!("{:?}", f.feature_type),
        team_id: f.team_id.to_string(),
        created_at: f.created_at.to_rfc3339(),
        stages: stage_msgs,
        dependencies: deps,
    };
    Ok(feature)
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
        let input = crate::graphql::schema::CreateContextInput {
            key: "country".into(),
            entries: vec!["US".into()],
        };
        let expected = crate::graphql::schema::Context {
            id: ID::from(Uuid::new_v4()),
            team_id: team_id.clone(),
            key: "country".into(),
            entries: vec![crate::graphql::schema::ContextEntry {
                id: ID::from(Uuid::new_v4()),
                value: "US".into(),
            }],
        };

        let team_id_clone = team_id.clone();
        mock.expect_create_context()
            .times(1)
            .withf(move |tid, i| {
                tid == &team_id_clone && i.key == "country" && i.entries.len() == 1
            })
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
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["createContext"]["key"], "country");
        assert_eq!(
            data["createContext"]["entries"].as_array().unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn test_set_stage_contexts_mutation() {
        use crate::graphql::query::Query as GqlQuery;
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        let ctx1 = ID::from(Uuid::new_v4());
        let ctx2 = ID::from(Uuid::new_v4());
        let expected = vec![
            crate::graphql::schema::Context {
                id: ctx1.clone(),
                team_id: ID::from(Uuid::new_v4()),
                key: "k1".into(),
                entries: vec![],
            },
            crate::graphql::schema::Context {
                id: ctx2.clone(),
                team_id: ID::from(Uuid::new_v4()),
                key: "k2".into(),
                entries: vec![],
            },
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
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["setStageContexts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_set_stage_criteria_mutation_and_validation() {
        use crate::logic::feature::MockFeatureLogic;
        let mut mock = MockFeatureLogic::new();
        let stage_id = ID::from(Uuid::new_v4());
        // success path
        let expected = vec![crate::graphql::schema::StageCriterion {
            id: ID::from(Uuid::new_v4()),
            stage_id: stage_id.clone(),
            context_key: "filter".into(),
            context: crate::graphql::schema::Context {
                id: ID::from(Uuid::new_v4()),
                team_id: ID::from(Uuid::new_v4()),
                key: "filter-alpha".into(),
                entries: vec![],
            },
            rollout_percentage: 75,
        }];
        let stage_id_clone = stage_id.clone();
        mock.expect_set_stage_criteria()
            .times(1)
            .withf(move |sid, crit| {
                sid == &stage_id_clone
                    && crit.len() == 1
                    && crit[0].context_key == "filter"
                    && crit[0].rollout_percentage == 75
            })
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
        assert!(
            resp.errors.is_empty(),
            "{}",
            serde_json::to_string(&resp.errors).unwrap()
        );

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
