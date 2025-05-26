use async_graphql::{Object, ID, Result as GqlResult, Context};
use feature_toggle_shared::graphql::{CreateEnvironmentInput, DeleteEnvironmentInput, Environment, UpdateEnvironmentInput};
use uuid::Uuid;

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_environment(&self,ctx: &Context<'_>, input: CreateEnvironmentInput) -> GqlResult<Environment> {
        let id = ID::from(Uuid::new_v4().to_string());
        Ok(Environment {
            id,
            name: input.name,
        })
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
}