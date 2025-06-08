use crate::database::entity::Pipeline;
use crate::database::{handle_error, Error};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

pub struct CreatePipeline {
    pub team_id: Uuid,
    pub name: String,
}

pub struct UpdatePipeline {
    pub id: Uuid,
    pub name: Option<String>,
    pub active: Option<bool>,
}

#[automock]
#[async_trait::async_trait]
pub trait PipelineRepository: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error>;
    async fn get_pipelines(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error>;
    async fn create_pipeline(&self, input: CreatePipeline) -> Result<Pipeline, Error>;
    async fn update_pipeline(&self, input: UpdatePipeline) -> Result<Pipeline, Error>;
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
            r#"SELECT id, name, active, id, name, active, team_id FROM pipelines WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_pipelines(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error> {
        let mut query_builder =
            sqlx::QueryBuilder::new(r#"SELECT id, name, active, team_id FROM pipelines"#);
        query_builder.push(" WHERE team_id = ").push_bind(team_id);

        if let Some(name) = name {
            query_builder.push(" AND name ILIKE ");
            query_builder.push_bind(format!("%{}%", name));
        }
        if let Some(active_value) = active {
            query_builder.push(" AND active = ").push_bind(active_value);
        }
        query_builder.push(" ORDER BY name");

        let result = query_builder
            .build_query_as::<Pipeline>()
            .fetch_all(&self.pool)
            .await;

        handle_error(None, result)
    }

    async fn create_pipeline(&self, input: CreatePipeline) -> Result<Pipeline, Error> {
        let existing_pipeline = self.get_pipelines(input.team_id.clone(), Some(input.name.clone()), None)
            .await;

        if let Ok(existing_pipeline) = existing_pipeline {
            if !existing_pipeline.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Pipeline with name '{}' already exists",
                    input.name
                )));
            }
        }

        let id = Uuid::new_v4();
        let result = sqlx::query!(
        r#"INSERT INTO pipelines (id, name, active, team_id) VALUES ($1, $2, true, $3) RETURNING id, name, active, team_id"#,
        id,input.name, input.team_id ).fetch_one(&self.pool).await;

        let handled_error = handle_error(None, result)?;
        Ok(Pipeline {
            id: handled_error.id,
            name: handled_error.name,
            active: handled_error.active,
            team_id: handled_error.team_id,
        })
    }

    async fn update_pipeline(&self, input: UpdatePipeline) -> Result<Pipeline, Error> {
        let existing_env = self.get_pipeline_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE pipelines SET name = $1, active = $2 WHERE id = $3 RETURNING id, name, active, team_id"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.id
        ).fetch_one(&self.pool)
        .await;

        let pipeline = handle_error(Some(input.id), result)?;
        Ok(Pipeline {
            id: pipeline.id,
            name: pipeline.name,
            active: pipeline.active,
            team_id: pipeline.team_id,
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
