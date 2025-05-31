use crate::database::entity::Environment;
use crate::database::{handle_error, Error};
use feature_toggle_shared::graphql::{CreateEnvironmentInput, UpdateEnvironmentInput};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait EnvironmentRepository: Send + Sync {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error>;
    async fn create_environment(&self, input: CreateEnvironmentInput)
    -> Result<Environment, Error>;
    async fn update_environment(&self, input: UpdateEnvironmentInput)
    -> Result<Environment, Error>;
    async fn delete_environment(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn EnvironmentRepository>;
}

impl Clone for Box<dyn EnvironmentRepository> {
    fn clone(&self) -> Box<dyn EnvironmentRepository> {
        self.clone_box()
    }
}

pub fn environment_repository(pool: PgPool) -> Box<dyn EnvironmentRepository> {
    Box::new(EnvironmentRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct EnvironmentRepositoryImpl {
    pool: PgPool,
}

impl EnvironmentRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl EnvironmentRepository for EnvironmentRepositoryImpl {
    async fn get_environment_by_id(&self, id: Uuid) -> Result<Environment, Error> {
        let result = sqlx::query_as::<_, Environment>(
            r#"SELECT id, name, active FROM environments WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn create_environment(
        &self,
        input: CreateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
        r#"INSERT INTO environments (id, name, active) VALUES ($1, $2, true) RETURNING id,name,active"#,
        id,
        input.name
    )
            .fetch_one(&self.pool)
            .await;

        let handled_error = handle_error(None, result)?;
        Ok(Environment {
            id: handled_error.id,
            name: handled_error.name,
            active: handled_error.active,
        })
    }

    async fn update_environment(
        &self,
        input: UpdateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let id = Uuid::try_from(input.id).unwrap();
        let existing_env = self.get_environment_by_id(id).await?;
        let result = sqlx::query!(
            r#"UPDATE environments SET name = $1, active = $2 WHERE id = $3 RETURNING id, name, active"#,
            input.name,
            input.active.unwrap_or(existing_env.active),
            id
        ).fetch_one(&self.pool)
            .await;

        let environment = handle_error(Some(id), result)?;

        Ok(Environment {
            id: environment.id,
            name: environment.name,
            active: environment.active,
        })
    }

    async fn delete_environment(&self, id: Uuid) -> Result<(), Error> {
        self.get_environment_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM environments WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn EnvironmentRepository> {
        Box::new(self.clone())
    }
}
