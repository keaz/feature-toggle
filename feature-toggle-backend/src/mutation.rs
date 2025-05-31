use crate::database::environment::EnvironmentRepository;
use crate::logic::environment::EnvironmentLogic;
use async_graphql::{Context, Object, Result as GqlResult};
use feature_toggle_shared::graphql::{
    CreateEnvironmentInput, DeleteEnvironmentInput, Environment, UpdateEnvironmentInput,
};

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_environment(
        &self,
        ctx: &Context<'_>,
        input: CreateEnvironmentInput,
    ) -> GqlResult<Environment> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        let result = logic.create_environment(input).await?; //FIXME Handle error appropriately in production code
        Ok(result)
    }

    async fn update_environment(&self, input: UpdateEnvironmentInput) -> GqlResult<Environment> {
        Ok(Environment {
            id: input.id,
            name: input.name,
        })
    }

    async fn delete_environment(&self, input: DeleteEnvironmentInput) -> GqlResult<bool> {
        Ok(true)
    }
    async fn get_environment_by_id(
        &self,
        ctx: &Context<'_>,
        id: uuid::Uuid,
    ) -> GqlResult<Environment> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>().unwrap();
        let result = logic.get_environment_by_id(id).await?;
        Ok(result)
    }
}
