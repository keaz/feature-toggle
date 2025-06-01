use crate::database::entity::Stage;
use crate::database::Error;
use feature_toggle_shared::graphql::CreateStageInput;
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait StageRepository: Send + Sync {
    async fn get_stage_by_id(&self, env_id: Uuid) -> Result<Stage, Error>;
    async fn get_stage(&self, name: Option<String>) -> Result<Vec<Stage>, Error>;
    async fn create_stage(&self, input: CreateStageInput) -> Result<Stage, Error>;
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
    async fn get_stage_by_id(&self, env_id: Uuid) -> Result<Stage, Error> {
        todo!()
    }

    async fn get_stage(&self, name: Option<String>) -> Result<Vec<Stage>, Error> {
        todo!()
    }

    async fn create_stage(&self, input: CreateStageInput) -> Result<Stage, Error> {
        todo!()
    }

    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error> {
        todo!()
    }

    fn clone_box(&self) -> Box<dyn StageRepository> {
        todo!()
    }
}
