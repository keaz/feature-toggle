use crate::Error;
use crate::database::environment::EnvironmentRepository;
use crate::graphql::schema::{CreateEnvironmentInput, Environment, UpdateEnvironmentInput};
use async_graphql::ID;
use mockall::automock;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait EnvironmentLogic: Send + Sync {
    async fn get_environment_by_id(&self, env_id: ID) -> Result<Environment, Error>;

    async fn get_environments(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error>;

    async fn get_environments_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Environment>, i64), Error>;

    async fn create_environment(
        &self,
        team_id: ID,
        input: CreateEnvironmentInput,
    ) -> Result<Environment, Error>;
    async fn update_environment(
        &self,
        id: ID,
        input: UpdateEnvironmentInput,
    ) -> Result<Environment, Error>;
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
    async fn get_environment_by_id(&self, env_id: ID) -> Result<Environment, Error> {
        let environment = self
            .repository
            .get_environment_by_id(Uuid::try_from(env_id).unwrap())
            .await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
            active: environment.active,
            team_id: ID::from(environment.team_id),
        })
    }

    async fn get_environments(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let environments = self
            .repository
            .get_environments(team_id, name, active)
            .await?;
        Ok(environments
            .into_iter()
            .map(|env| Environment {
                id: ID::from(env.id),
                name: env.name,
                active: env.active,
                team_id: ID::from(env.team_id),
            })
            .collect())
    }

    async fn get_environments_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Environment>, i64), Error> {
        let team_id = Uuid::try_from(team_id)
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let (environments, total) = self
            .repository
            .get_environments_paginated(team_id, name, active, page_number, page_size)
            .await?;
        let mapped_environments = environments
            .into_iter()
            .map(|env| Environment {
                id: ID::from(env.id),
                name: env.name,
                active: env.active,
                team_id: ID::from(env.team_id),
            })
            .collect();
        Ok((mapped_environments, total))
    }

    async fn create_environment(
        &self,
        team_id: ID,
        input: CreateEnvironmentInput,
    ) -> Result<Environment, Error> {
        let input = crate::database::environment::CreateEnvironment {
            name: input.name,
            active: input.active,
        };

        if input.name.is_empty() {
            return Err(Error::InvalidInput(
                "Environment name cannot be empty".to_string(),
            ));
        }

        let team_id = Uuid::try_from(team_id).unwrap();
        let environment = self.repository.create_environment(team_id, input).await?;
        let id = ID::from(environment.id);
        Ok(Environment {
            id,
            name: environment.name,
            active: environment.active,
            team_id: ID::from(environment.team_id),
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
            team_id: ID::from(environment.team_id),
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
                    team_id: Uuid::new_v4(), // Mock team ID
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic
            .get_environment_by_id(ID::try_from(ENV_ID).unwrap())
            .await;

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
        let result = logic
            .get_environment_by_id(ID::try_from(ENV_ID).unwrap())
            .await;

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
                    team_id: Uuid::new_v4(),
                })
            });

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.create_environment(ID::from(ID), input).await;

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
                    team_id: Uuid::new_v4(),
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
        mock_repository
            .expect_delete_environment()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = environment_logic(Box::new(mock_repository));
        let result = logic.delete_environment(ID::from(ENV_ID)).await;

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
        let result = logic.delete_environment(ID::from(ENV_ID)).await;

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
        let team_id = ID::from(expected_id);
        mock_repository
            .expect_get_environments()
            .withf(|_, name, active| name.is_none() && active.is_none())
            .times(1)
            .returning(move |_, _, _| {
                Ok(vec![
                    crate::database::entity::Environment {
                        id: expected_id,
                        name: "Test Environment".to_string(),
                        active: true,
                        team_id: Uuid::new_v4(),
                    },
                    crate::database::entity::Environment {
                        id: expected_id,
                        name: "Test Environment".to_string(),
                        active: true,
                        team_id: Uuid::new_v4(),
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

    #[tokio::test]
    async fn test_get_environments_paginated_success() {
        let mut mock_repository = MockEnvironmentRepository::new();
        let team_id = Uuid::new_v4();
        let env1_id = Uuid::new_v4();
        let env2_id = Uuid::new_v4();
        
        let expected_environments = vec![
            crate::database::entity::Environment {
                id: env1_id,
                name: "Production".to_string(),
                active: true,
                team_id,
            },
            crate::database::entity::Environment {
                id: env2_id,
                name: "Development".to_string(),
                active: false,
                team_id,
            },
        ];

        mock_repository
            .expect_get_environments_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(move |_, _, _, _, _| Ok((expected_environments.clone(), 20)));

        let logic = environment_logic(Box::new(mock_repository));
        let (environments, total) = logic
            .get_environments_paginated(
                ID::from(team_id),
                None,
                None,
                1,
                10,
            )
            .await
            .unwrap();

        assert_eq!(environments.len(), 2);
        assert_eq!(total, 20);
        assert_eq!(environments[0].name, "Production");
        assert_eq!(environments[0].active, true);
        assert_eq!(environments[1].name, "Development");
        assert_eq!(environments[1].active, false);
    }

    #[tokio::test]
    async fn test_get_environments_paginated_with_filters() {
        let mut mock_repository = MockEnvironmentRepository::new();
        let team_id = Uuid::new_v4();

        mock_repository
            .expect_get_environments_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(Some("prod".to_string())),
                mockall::predicate::eq(Some(true)),
                mockall::predicate::eq(2),
                mockall::predicate::eq(5),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 0)));

        let logic = environment_logic(Box::new(mock_repository));
        let (environments, total) = logic
            .get_environments_paginated(
                ID::from(team_id),
                Some("prod".to_string()),
                Some(true),
                2,
                5,
            )
            .await
            .unwrap();

        assert_eq!(environments.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_environments_paginated_invalid_team_id() {
        let mock_repository = MockEnvironmentRepository::new();
        let logic = environment_logic(Box::new(mock_repository));
        
        let result = logic
            .get_environments_paginated(
                ID::from("invalid-uuid"),
                None,
                None,
                1,
                10,
            )
            .await;

        assert!(matches!(result, Err(Error::InvalidInput(_))));
    }
}
