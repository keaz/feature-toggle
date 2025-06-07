use crate::logic::environment::EnvironmentLogic;
use async_graphql::{Context, Object, Result as GqlResult, ID};
use feature_toggle_shared::graphql::{CreateEnvironmentInput, Environment, UpdateEnvironmentInput};

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_environment(
        &self,
        ctx: &Context<'_>,
        input: CreateEnvironmentInput,
    ) -> GqlResult<Environment> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(logic.create_environment(input).await?)
    }

    async fn update_environment(
        &self,
        ctx: &Context<'_>,
        id: ID,
        input: UpdateEnvironmentInput,
    ) -> GqlResult<Environment> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(logic.update_environment(id, input).await?)
    }

    async fn delete_environment(&self, ctx: &Context<'_>, id: uuid::Uuid) -> GqlResult<bool> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        logic.delete_environment(id).await?;
        Ok(true)
    }

    async fn get_environment_by_id(
        &self,
        ctx: &Context<'_>,
        id: uuid::Uuid,
    ) -> GqlResult<Environment> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        Ok(logic.get_environment_by_id(id).await?)
    }
}
