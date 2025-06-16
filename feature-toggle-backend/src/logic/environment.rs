use crate::database::environment::EnvironmentRepository;
use crate::graphql::schema::{CreateEnvironmentInput, Environment, UpdateEnvironmentInput};
use crate::Error;
use async_graphql::ID;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait EnvironmentLogic: Send + Sync {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error>;

    async fn get_environments(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error>;

    async fn create_environment(&self, team_id: ID, input: CreateEnvironmentInput)
    -> Result<Environment, Error>;
    async fn update_environment(&self, id: ID, input: UpdateEnvironmentInput)
    -> Result<Environment, Error>;
    async fn delete_environment(&self, id: ID) -> Result<(), Error>;

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
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let environments = self.repository.get_environments(team_id, name, active).await?;
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
        team_id: ID,
        input: CreateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let input = crate::database::environment::CreateEnvironment { name: input.name, active: input.active };

        if input.name.is_empty() {
            return Err(Error::InvalidInput("Environment name cannot be empty".to_string()));
        }

        let team_id = Uuid::try_from(team_id).unwrap();
        let environment = self.repository.create_environment(team_id, input).await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
            active: environment.active,
        })
    }

    async fn update_environment(
        &self,
        id: ID,
        input: UpdateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let input = crate::database::environment::UpdateEnvironment {
            name: input.name,
            active: input.active,
        };

        let id = Uuid::try_from(id).unwrap();
        let environment = self.repository.update_environment(id, input).await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
            active: environment.active,
        })
    }

    async fn delete_environment(&self, id: ID) -> Result<(), Error> {
        let id = Uuid::try_from(id).unwrap();
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
                    id,
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
            .returning(move |_| Err(Error::NotFound(id)));

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
            active: true,
        };
        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let expected_id = Uuid::parse_str(ID).unwrap();
        mock_repository
            .expect_create_environment()
            .withf(move |id, input| id == &expected_id.clone() && input.name == "New Environment")
            .times(1)
            .returning(move |_, _| {
                Ok(crate::database::entity::Environment {
                    id: expected_id,
                    name: "New Environment".to_string(),
                    active: true,
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.create_environment(ID::try_from(ID).unwrap(), input).await;

        assert!(result.is_ok());
        let environment = result.unwrap();
        assert_eq!(environment.id, ID::from(expected_id));
        assert_eq!(environment.name, "New Environment");
    }

    #[tokio::test]
    async fn test_update_environment() {
        let mut mock_repository = MockEnvironmentRepository::new();
        let input = UpdateEnvironmentInput {
            name: Some("Updated Environment".to_string()),
            active: Some(true),
        };
        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let expected_id = Uuid::parse_str(ID).unwrap();
        mock_repository
            .expect_update_environment()
            .withf(move |id, input| {
                id == &expected_id.clone() && input.name == Some("Updated Environment".to_string())
            })
            .times(1)
            .returning(move |_, _| {
                Ok(crate::database::entity::Environment {
                    id: expected_id,
                    name: "Updated Environment".to_string(),
                    active: true,
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.update_environment(ID::from(ID), input).await;

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
            name: Some("Updated Environment".to_string()),
            active: Some(true),
        };
        let expected_id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_update_environment()
            .withf(move |id, input| {
                id == &expected_id.clone() && input.name == Some("Updated Environment".to_string())
            })
            .times(1)
            .returning(move |_, _| Err(Error::NotFound(expected_id)));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.update_environment(ID::from(ENV_ID), input).await;

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
        let result = logic.delete_environment(ID::try_from(ENV_ID).unwrap()).await;

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
            .returning(move |_| Err(Error::NotFound(id)));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.delete_environment(ID::try_from(ENV_ID).unwrap()).await;

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
        let team_id = ID::try_from(expected_id).unwrap();
        mock_repository
            .expect_get_environments()
            .withf(|team, name, active| name.is_none() && active.is_none())
            .times(1)
            .returning(move |_, _, _| {
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
        let result = logic.get_environments(team_id, None, None).await;

        assert!(result.is_ok());
        let environments = result.unwrap();
        assert_eq!(environments.len(), 2);
        assert_eq!(environments[0].id, ID::from(expected_id));
        assert_eq!(environments[0].name, "Test Environment");
    }
}
