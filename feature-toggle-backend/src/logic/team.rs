use crate::database::team::TeamRepository;
use crate::Error;
use async_graphql::ID;
use feature_toggle_shared::graphql::{CreateTeamInput, Team, UpdateTeamInput};
use uuid::Uuid;

#[async_trait::async_trait]
pub trait TeamLogic: Send + Sync {
    async fn get_team_by_id(&self, env_id: Uuid) -> Result<Team, Error>;

    async fn get_teams(
        &self,
        name: Option<String>,
    ) -> Result<Vec<Team>, Error>;

    async fn create_team(&self, input: CreateTeamInput)
                         -> Result<Team, Error>;
    async fn update_team(&self, id: ID, input: UpdateTeamInput)
                         -> Result<Team, Error>;
    async fn delete_team(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn TeamLogic>;
}

impl Clone for Box<dyn TeamLogic> {
    fn clone(&self) -> Box<dyn TeamLogic> {
        self.clone_box()
    }
}

pub fn team_logic(repository: Box<dyn TeamRepository>) -> Box<dyn TeamLogic> {
    Box::new(TeamLogicImpl { repository })
}

#[derive(Clone)]
struct TeamLogicImpl {
    repository: Box<dyn TeamRepository>,
}

#[async_trait::async_trait]
impl TeamLogic for TeamLogicImpl {
    async fn get_team_by_id(&self, env_id: Uuid) -> Result<Team, Error> {
        let team = self.repository.get_team_by_id(env_id).await?;
        let id = ID::from(team.id);
        Ok(Team {
            id,
            name: team.name,
            description: team.description,
        })
    }

    async fn get_teams(
        &self,
        name: Option<String>,
    ) -> Result<Vec<Team>, Error> {
        let teams = self.repository.get_teams(name).await?;
        Ok(teams
            .into_iter()
            .map(|env| Team {
                id: ID::from(env.id),
                name: env.name,
                description: env.description,
            })
            .collect())
    }

    async fn create_team(
        &self,
        input: CreateTeamInput,
    ) -> Result<Team, Error> {
        let input = crate::database::team::CreateTeam { name: input.name, description: input.description };

        if input.name.is_empty() {
            return Err(Error::InvalidInput("Team name cannot be empty".to_string()));
        }

        let team = self.repository.create_team(input).await?;
        let id = ID::from(team.id);
        Ok(Team {
            id,
            name: team.name,
            description: team.description,
        })
    }

    async fn update_team(
        &self,
        id: ID,
        input: UpdateTeamInput,
    ) -> Result<Team, Error> {
        let input = crate::database::team::UpdateTeam {
            id: Uuid::try_from(id).unwrap(),
            name: input.name,
            description: input.description,
        };

        let team = self.repository.update_team(input).await?;
        let id = ID::from(team.id);
        Ok(Team {
            id,
            name: team.name,
            description: team.description,
        })
    }

    async fn delete_team(&self, id: Uuid) -> Result<(), Error> {
        self.repository.delete_team(id).await
    }

    fn clone_box(&self) -> Box<dyn TeamLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::team::MockTeamRepository;

    #[tokio::test]
    async fn test_ok_get_team_by_id() {
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let mut mock_repository = MockTeamRepository::new();
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_get_team_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Team {
                    id,
                    name: "Mock Team".to_string(),
                    description: "Mock Description".to_string(),
                })
            });

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.get_team_by_id(id).await;

        assert!(result.is_ok());
        let team = result.unwrap();
        assert_eq!(team.id, ID::from(id));
        assert_eq!(team.name, "Mock Team");
    }

    #[tokio::test]
    async fn test_error_get_team_by_id() {
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let mut mock_repository = MockTeamRepository::new();
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_get_team_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(id)));

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.get_team_by_id(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        match error {
            Error::NotFound(eid) => assert_eq!(eid, id),
            _ => panic!("Expected NotFound error variant"),
        }
    }

    #[tokio::test]
    async fn test_create_team() {
        let mut mock_repository = MockTeamRepository::new();
        let input = CreateTeamInput {
            name: "New Team".to_string(),
            description: "Description of the new team".to_string(),
        };
        let expected_id = Uuid::new_v4();
        mock_repository
            .expect_create_team()
            .withf(|input| input.name == "New Team")
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Team {
                    id: expected_id,
                    name: "New Team".to_string(),
                    description: "Description of the new team".to_string(),
                })
            });

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.create_team(input).await;

        assert!(result.is_ok());
        let team = result.unwrap();
        assert_eq!(team.id, ID::from(expected_id));
        assert_eq!(team.name, "New Team");
    }

    #[tokio::test]
    async fn test_update_team() {
        let mut mock_repository = MockTeamRepository::new();
        let input = UpdateTeamInput {
            name: Some("Updated Team".to_string()),
            description: Some("Updated description".to_string()),
        };
        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let expected_id = Uuid::parse_str(ID).unwrap();
        mock_repository
            .expect_update_team()
            .withf(|input| {
                input.id == input.id && input.name == Some("Updated Team".to_string())
            })
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Team {
                    id: expected_id,
                    name: "Updated Team".to_string(),
                    description: "Updated description".to_string(),
                })
            });

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.update_team(ID::from(ID), input).await;

        assert!(result.is_ok());
        let team = result.unwrap();
        assert_eq!(team.id, ID::from(expected_id));
        assert_eq!(team.name, "Updated Team");
    }

    #[tokio::test]
    async fn test_not_exists_update_team() {
        let mut mock_repository = MockTeamRepository::new();
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let input = UpdateTeamInput {
            name: Some("Updated Team".to_string()),
            description: Some("Updated description".to_string()),
        };
        let expected_id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_update_team()
            .withf(|input| {
                input.id == input.id && input.name == Some("Updated Team".to_string())
            })
            .times(1)
            .returning(move |_| Err(Error::NotFound(expected_id)));

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.update_team(ID::from(ENV_ID), input).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        match error {
            Error::NotFound(eid) => assert_eq!(eid, Uuid::parse_str(ENV_ID).unwrap()),
            _ => panic!("Expected NotFound error variant"),
        }
    }

    #[tokio::test]
    async fn test_delete_team() {
        let mut mock_repository = MockTeamRepository::new();
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_delete_team()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.delete_team(id).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_error_delete_team() {
        let mut mock_repository = MockTeamRepository::new();
        const ENV_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
        let id = Uuid::parse_str(ENV_ID).unwrap();
        mock_repository
            .expect_delete_team()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ENV_ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(id)));

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.delete_team(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        match error {
            Error::NotFound(eid) => assert_eq!(eid, id),
            _ => panic!("Expected NotFound error variant"),
        }
    }

    #[tokio::test]
    async fn test_get_teams() {
        let mut mock_repository = MockTeamRepository::new();
        let expected_id = Uuid::new_v4();
        mock_repository
            .expect_get_teams()
            .withf(|name| name.is_none())
            .times(1)
            .returning(move |_| {
                Ok(vec![
                    crate::database::entity::Team {
                        id: expected_id,
                        name: "Test Team 1".to_string(),
                        description: "Test Description".to_string(),
                    },
                    crate::database::entity::Team {
                        id: expected_id,
                        name: "Test Team 2".to_string(),
                        description: "Test Description 2".to_string(),
                    },
                ])
            });

        let logic = team_logic(Box::new(mock_repository));
        let result = logic.get_teams(None).await;

        assert!(result.is_ok());
        let teams = result.unwrap();
        assert_eq!(teams.len(), 2);
        assert_eq!(teams[0].id, ID::from(expected_id));
        assert_eq!(teams[0].name, "Test Team 1");
    }
}
