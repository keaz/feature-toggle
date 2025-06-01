use crate::database::entity::{Environment, Pipeline};
use crate::database::{handle_error, Error};
use feature_toggle_shared::graphql::{CreatePipelineInput, UpdatePipelineInput};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait PipelineRepository: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error>;
    async fn get_pipelines(&self, name: Option<String>) -> Result<Vec<Pipeline>, Error>;
    async fn create_pipeline(&self, input: CreatePipelineInput) -> Result<Pipeline, Error>;
    async fn update_pipeline(&self, input: UpdatePipelineInput) -> Result<Pipeline, Error>;
    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn PipelineRepository>;
}

impl Clone for Box<dyn PipelineRepository> {
    fn clone(&self) -> Box<dyn PipelineRepository> {
        self.clone_box()
    }
}

pub fn pipeline_repository(pool: PgPool) -> Box<dyn PipelineRepository> {
    Box::new(PipelineRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct PipelineRepositoryImpl {
    pool: PgPool,
}

impl PipelineRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl PipelineRepository for PipelineRepositoryImpl {
    async fn get_pipeline_by_id(&self, id: Uuid) -> Result<Pipeline, Error> {
        let result = sqlx::query_as::<_, Pipeline>(
            r#"SELECT id, name, active FROM pipelines WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_pipelines(&self, name: Option<String>) -> Result<Vec<Pipeline>, Error> {
        let mut query_builder =
            sqlx::QueryBuilder::new(r#"SELECT id, name, active FROM pipelines"#);

        if let Some(name) = name {
            query_builder.push(" WHERE name ILIKE ");
            query_builder.push_bind(format!("%{}%", name));
        }
        query_builder.push(" ORDER BY name");

        let result = query_builder
            .build_query_as::<Pipeline>()
            .fetch_all(&self.pool)
            .await;

        handle_error(None, result)
    }

    async fn create_pipeline(&self, input: CreatePipelineInput) -> Result<Pipeline, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
        r#"INSERT INTO pipelines (id, name, active) VALUES ($1, $2, true) RETURNING id,name,active"#,
        id,
        input.name
    )
            .fetch_one(&self.pool)
            .await;

        let handled_error = handle_error(None, result)?;
        Ok(Pipeline {
            id: handled_error.id,
            name: handled_error.name,
            active: handled_error.active,
        })
    }

    async fn update_pipeline(&self, input: UpdatePipelineInput) -> Result<Pipeline, Error> {
        let id = Uuid::try_from(input.id).unwrap();
        let existing_env = self.get_pipeline_by_id(id).await?;
        let result = sqlx::query!(
            r#"UPDATE pipelines SET name = $1, active = $2 WHERE id = $3 RETURNING id, name, active"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            id
        )
        .fetch_one(&self.pool)
        .await;

        let pipeline = handle_error(Some(id), result)?;
        Ok(Pipeline {
            id: pipeline.id,
            name: pipeline.name,
            active: pipeline.active,
        })
    }

    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error> {
        self.get_pipeline_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM pipelines WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn PipelineRepository> {
        Box::new(self.clone())
    }
}
