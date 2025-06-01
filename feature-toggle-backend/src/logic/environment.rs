use crate::database::environment::EnvironmentRepository;
use crate::database::Error;
use async_graphql::ID;
use feature_toggle_shared::graphql::{CreateEnvironmentInput, Environment, UpdateEnvironmentInput};
use uuid::Uuid;

#[async_trait::async_trait]
pub trait EnvironmentLogic: Send + Sync {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error>;

    async fn get_environments(
        &self,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error>;

    async fn create_environment(&self, input: CreateEnvironmentInput)
    -> Result<Environment, Error>;
    async fn update_environment(&self, input: UpdateEnvironmentInput)
    -> Result<Environment, Error>;
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
            active: environment.active,
        })
    }

    async fn get_environments(
        &self,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error> {
        let environments = self.repository.get_environments(name, active).await?;
        Ok(environments
            .into_iter()
            .map(|env| Environment {
                id: ID::from(env.id),
                name: env.name,
                active: env.active,
            })
            .collect())
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
            active: environment.active,
        })
    }

    async fn update_environment(
        &self,
        input: UpdateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let environment = self.repository.update_environment(input).await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
            active: environment.active,
        })
    }

    async fn delete_environment(&self, id: Uuid) -> Result<(), Error> {
        self.repository.delete_environment(id).await
    }

    fn clone_box(&self) -> Box<dyn EnvironmentLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
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

    #[tokio::test]
    async fn test_create_environment() {
        let mut mock_repository = MockEnvironmentRepository::new();
        let input = CreateEnvironmentInput {
            name: "New Environment".to_string(),
        };
        let expected_id = Uuid::new_v4();
        mock_repository
            .expect_create_environment()
            .withf(|input| input.name == "New Environment")
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Environment {
                    id: expected_id,
                    name: "New Environment".to_string(),
                    active: true,
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.create_environment(input).await;

        assert!(result.is_ok());
        let environment = result.unwrap();
        assert_eq!(environment.id, ID::from(expected_id));
        assert_eq!(environment.name, "New Environment");
    }

    #[tokio::test]
    async fn test_update_environment() {
        let mut mock_repository = MockEnvironmentRepository::new();
        let input = UpdateEnvironmentInput {
            id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
            name: Some("Updated Environment".to_string()),
            active: Some(true),
        };
        let expected_id = Uuid::parse_str(&input.id).unwrap();
        mock_repository
            .expect_update_environment()
            .withf(|input| {
                input.id == input.id && input.name == Some("Updated Environment".to_string())
            })
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Environment {
                    id: expected_id,
                    name: "Updated Environment".to_string(),
                    active: true,
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.update_environment(input).await;

        assert!(result.is_ok());
        let environment = result.unwrap();
        assert_eq!(environment.id, ID::from(expected_id));
        assert_eq!(environment.name, "Updated Environment");
    }

    #[tokio::test]
    async fn test_not_exists_update_environment() {
        let mut mock_repository = MockEnvironmentRepository::new();
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let input = UpdateEnvironmentInput {
            id: ID::from(ENV_ID),
            name: Some("Updated Environment".to_string()),
            active: Some(true),
        };
        let expected_id = Uuid::parse_str(&input.id).unwrap();
        mock_repository
            .expect_update_environment()
            .withf(|input| {
                input.id == input.id && input.name == Some("Updated Environment".to_string())
            })
            .times(1)
            .returning(move |_| Err(Error::NotFound(expected_id.clone())));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.update_environment(input).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        match error {
            Error::NotFound(eid) => assert_eq!(eid, Uuid::parse_str(ENV_ID).unwrap()),
            _ => panic!("Expected NotFound error variant"),
        }
    }

    #[tokio::test]
    async fn test_delete_environment() {
        let mut mock_repository = MockEnvironmentRepository::new();
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_delete_environment()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.delete_environment(id).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_error_delete_environment() {
        let mut mock_repository = MockEnvironmentRepository::new();
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_delete_environment()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(id.clone())));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.delete_environment(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        match error {
            Error::NotFound(eid) => assert_eq!(eid, id),
            _ => panic!("Expected NotFound error variant"),
        }
    }

    #[tokio::test]
    async fn test_get_environments() {
        let mut mock_repository = MockEnvironmentRepository::new();
        let expected_id = Uuid::new_v4();
        mock_repository
            .expect_get_environments()
            .withf(|name, active| name.is_none() && active.is_none())
            .times(1)
            .returning(move |_, _| {
                Ok(vec![
                    crate::database::entity::Environment {
                        id: expected_id,
                        name: "Test Environment".to_string(),
                        active: true,
                    },
                    crate::database::entity::Environment {
                        id: expected_id,
                        name: "Test Environment".to_string(),
                        active: true,
                    },
                ])
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.get_environments(None, None).await;

        assert!(result.is_ok());
        let environments = result.unwrap();
        assert_eq!(environments.len(), 2);
        assert_eq!(environments[0].id, ID::from(expected_id));
        assert_eq!(environments[0].name, "Test Environment");
    }
}
