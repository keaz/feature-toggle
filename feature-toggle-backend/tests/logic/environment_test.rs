use async_trait::async_trait;
use feature_toggle_backend::database::entity::Environment;
use feature_toggle_backend::database::environment::EnvironmentRepository;
use feature_toggle_backend::database::Error;
use feature_toggle_shared::graphql::{CreateEnvironmentInput, UpdateEnvironmentInput};
use uuid::Uuid;

struct MockEnvironmentRepository;
#[async_trait]
impl EnvironmentRepository for MockEnvironmentRepository {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error> {
        Ok(Environment {
            id: env_id,
            name: "Mock Environment".to_string(),
            active: true,
        })
    }

    async fn create_environment(
        &self,
        input: CreateEnvironmentInput,
    ) -> Result<Environment, Error> {
        Ok(Environment {
            id: Uuid::new_v4(),
            name: input.name,
            active: true,
        })
    }

    async fn update_environment(&self, input: UpdateEnvironmentInput) -> Result<(), Error> {
        Ok(())
    }

    async fn delete_environment(&self, id: Uuid) -> Result<(), Error> {
        todo!()
    }

    fn clone_box(&self) -> Box<dyn EnvironmentRepository> {
        todo!()
    }
}

// #[tokio::test]
// async fn test_get_environment_by_id() {
//     let moc_repository =  MokEnvironmentRepository::new();
// }
