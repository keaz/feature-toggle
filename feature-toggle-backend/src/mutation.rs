use async_graphql::{Object, ID, Result as GqlResult, Context};
use sqlx::PgPool;
use feature_toggle_shared::graphql::{CreateEnvironmentInput, DeleteEnvironmentInput, Environment, UpdateEnvironmentInput};
use uuid::Uuid;
use crate::database;

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_environment(&self,ctx: &Context<'_>, input: CreateEnvironmentInput) -> GqlResult<Environment> {
        let pool = ctx.data::<PgPool>().unwrap();

        let id = ID::from(Uuid::new_v4().to_string());
        let result = database::repository::create_environment(pool,&input).await.unwrap();
        Ok(Environment {
            id: ID::from(result.id),
            name: result.name,
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