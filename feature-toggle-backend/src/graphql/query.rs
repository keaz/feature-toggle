use crate::logic::environment::EnvironmentLogic;
use async_graphql::{Context, Object, Result as GqlResult};
use feature_toggle_shared::graphql::Environment;
use uuid::Uuid;

pub struct Query;

#[Object]
impl Query {
    async fn environment(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Id of object")] id: Uuid,
    ) -> GqlResult<Environment> {
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(repository.get_environment_by_id(id).await?)
    }

    async fn environments(
        &self,
        ctx: &Context<'_>,
        #[graphql(desc = "Name of the environment")] name: Option<String>,
        #[graphql(desc = "Active status of the environment")] active: Option<bool>,
    ) -> GqlResult<Vec<Environment>> {
        let repository = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(repository.get_environments(name, active).await?)
    }
}
