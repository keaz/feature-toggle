use crate::database::environment::EnvironmentRepository;
use crate::database::Error;
use async_graphql::ID;
use feature_toggle_shared::graphql::{CreateEnvironmentInput, Environment, UpdateEnvironmentInput};
use uuid::Uuid;

#[async_trait::async_trait]
pub trait EnvironmentLogic: Send + Sync {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error>;
    async fn create_environment(&self, input: CreateEnvironmentInput)
    -> Result<Environment, Error>;
    async fn update_environment(&self, input: UpdateEnvironmentInput) -> Result<(), Error>;
    async fn delete_environment(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn EnvironmentLogic>;
}

impl Clone for Box<dyn EnvironmentLogic> {
    fn clone(&self) -> Box<dyn EnvironmentLogic> {
        self.clone_box()
    }
}

pub fn environment_logic(repository: Box<dyn EnvironmentRepository>) -> Box<dyn EnvironmentLogic> {
    Box::new(EnvironmentLogicImpl { repository })
}

#[derive(Clone)]
struct EnvironmentLogicImpl {
    repository: Box<dyn EnvironmentRepository>,
}

#[async_trait::async_trait]
impl EnvironmentLogic for EnvironmentLogicImpl {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error> {
        let environment = self.repository.get_environment_by_id(env_id).await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
        })
    }

    async fn create_environment(
        &self,
        input: CreateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let environment = self.repository.create_environment(input).await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
        })
    }

    async fn update_environment(&self, input: UpdateEnvironmentInput) -> Result<(), Error> {
        self.repository.update_environment(input).await
    }

    async fn delete_environment(&self, id: Uuid) -> Result<(), Error> {
        self.repository.delete_environment(id).await
    }

    fn clone_box(&self) -> Box<dyn EnvironmentLogic> {
        Box::new(self.clone())
    }
}

mod tests {
    use super::*;
    use crate::database::environment::MockEnvironmentRepository;

    #[tokio::test]
    async fn test_ok_get_environment_by_id() {
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let mut mock_repository = MockEnvironmentRepository::new();
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_get_environment_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Environment {
                    id: id.clone(),
                    name: "Mock Environment".to_string(),
                    active: true,
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.get_environment_by_id(id).await;

        assert!(result.is_ok());
        let environment = result.unwrap();
        assert_eq!(environment.id, ID::from(id));
        assert_eq!(environment.name, "Mock Environment");
    }

    #[tokio::test]
    async fn test_error_get_environment_by_id() {
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let mut mock_repository = MockEnvironmentRepository::new();
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_get_environment_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(id.clone())));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.get_environment_by_id(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        match error {
            Error::NotFound(eid) => assert_eq!(eid, id),
            _ => panic!("Expected NotFound error variant"),
        }
    }
}
