use crate::database::entity::Stage;
use crate::database::{Error, handle_error};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

pub struct CreateStage {
    pub pipeline_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i8,
    pub parent_stage_id: Option<Uuid>,
}

#[automock]
#[async_trait::async_trait]
pub trait StageRepository: Send + Sync {
    async fn get_stage_by_id(&self, id: Uuid) -> Result<Stage, Error>;
    async fn get_stage(
        &self,
        pipeline_id: Option<Uuid>,
        parent_stage_id: Option<Uuid>,
    ) -> Result<Vec<Stage>, Error>;
    async fn create_stage(&self, stage: CreateStage) -> Result<Stage, Error>;
    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error>;
    fn clone_box(&self) -> Box<dyn StageRepository>;
}

impl Clone for Box<dyn StageRepository> {
    fn clone(&self) -> Box<dyn StageRepository> {
        self.clone_box()
    }
}

pub fn stage_repository(pool: PgPool) -> Box<dyn StageRepository> {
    Box::new(StageRepositoryImpl { pool })
}

#[derive(Clone)]
struct StageRepositoryImpl {
    pool: PgPool,
}

#[async_trait::async_trait]
impl StageRepository for StageRepositoryImpl {
    async fn get_stage_by_id(&self, id: Uuid) -> Result<Stage, Error> {
        let result = sqlx::query_as::<_, Stage>(
            r#"SELECT id, pipeline_id , environment_id, order_index, parent_stage_id FROM pipeline_stages WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_stage(
        &self,
        pipeline_id: Option<Uuid>,
        parent_stage_id: Option<Uuid>,
    ) -> Result<Vec<Stage>, Error> {
        let mut query_builder = sqlx::QueryBuilder::new(
            r#"SELECT id, pipeline_id, environment_id, order_index, parent_stage_id FROM pipeline_stages"#,
        );

        let mut has_where = false;
        if pipeline_id.is_some() || parent_stage_id.is_some() {
            query_builder.push(" WHERE ");
        }

        if let Some(pipeline_id) = pipeline_id {
            query_builder.push(" pipeline_id ILIKE ");
            query_builder.push_bind(format!("%{}%", pipeline_id));
            has_where = true;
        }
        if let Some(parent_stage_id) = parent_stage_id {
            if has_where {
                query_builder.push(" AND ");
            }
            query_builder
                .push("parent_stage_id = ")
                .push_bind(parent_stage_id);
        }
        query_builder.push(" ORDER BY name");

        let result = query_builder
            .build_query_as::<Stage>()
            .fetch_all(&self.pool)
            .await;

        handle_error(None, result)
    }

    async fn create_stage(&self, stage: CreateStage) -> Result<Stage, Error> {
        let existing_stage = self
            .get_stage(Some(stage.pipeline_id), Some(stage.environment_id))
            .await?;

        if !existing_stage.is_empty() {
            return Err(Error::RecordAlreadyExists(format!(
                "Stage already exists for pipeline {} and \
            environment {}",
                stage.pipeline_id, stage.environment_id
            )));
        }

        let id = Uuid::new_v4();
        let result = sqlx::query_as::<_, Stage>(
            r#"INSERT INTO pipeline_stages (id, pipeline_id, environment_id, order_index, parent_stage_id)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id, pipeline_id, environment_id, order_index, parent_stage_id"#,
        )
        .bind(id)
        .bind(stage.pipeline_id)
        .bind(stage.environment_id)
        .bind(stage.order_index)
        .bind(stage.parent_stage_id)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error> {
        let result = sqlx::query!(r#"DELETE FROM pipeline_stages WHERE pipeline_id = $1"#, id)
            .execute(&self.pool)
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::DatabaseError(e)),
        }
    }

    fn clone_box(&self) -> Box<dyn StageRepository> {
        Box::new(self.clone())
    }
}
