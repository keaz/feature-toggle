use crate::database::entity::DBStage;
use crate::graphql::schema::{Environment, Relationship, Stage};
use crate::logic::environment::EnvironmentLogic;
use crate::Error;
use async_graphql::ID;
use std::collections::HashMap;
use uuid::Uuid;

pub mod environment;
pub mod feature;
pub mod pipeline;
pub mod team;

fn create_relationships<R: Relationship + 'static>(
    has_stage: bool,
    stages: Vec<Box<dyn DBStage>>,
    relationship_factory: impl Fn(i32, i32) -> R,
) -> Vec<R> {
    let mut relationships: Vec<R> = vec![];
    if !has_stage {
        return relationships;
    }
    stages
        .iter()
        .filter(|stage| stage.parent_stage_id().is_some())
        .for_each(|stage| {
            stages
                .iter()
                .filter(|stage_inner| stage.parent_stage_id().unwrap() == stage_inner.get_id())
                .for_each(|stage_inner| {
                    relationships.push(relationship_factory(
                        stage_inner.order_index(),
                        stage.order_index(),
                    ));
                });
        });

    relationships
}

fn map_stages<R: Stage + 'static>(
    has_stage: bool,
    environment_map: &HashMap<Uuid, Environment>,
    stages: &Vec<Box<dyn DBStage>>,
    stage_factory: impl Fn(ID, Environment, i32, String, bool) -> R,
) -> Vec<R> {
    let mut mapped_stages: Vec<R> = vec![];
    if has_stage {
        stages.iter().for_each(|stage| {
            let feature_stage = stage_factory(
                stage.get_id().into(),
                environment_map
                    .get(&stage.environment_id())
                    .unwrap()
                    .to_owned(),
                stage.order_index(),
                stage.position(),
                stage.enabled()
            );

            mapped_stages.push(feature_stage);
        });
        mapped_stages
    } else {
        mapped_stages
    }
}

pub async fn get_environment_map(
    environment_logic: &dyn EnvironmentLogic,
    stages: &Vec<Box<dyn DBStage>>,
    has_stage: bool,
) -> Result<HashMap<Uuid, Environment>, Error> {
    let mut environment_map = HashMap::new();
    for stage in stages {
        if has_stage && !environment_map.contains_key(&stage.environment_id()) {
            let environment = environment_logic
                .get_environment_by_id(stage.environment_id().into())
                .await?;
            environment_map.insert(stage.environment_id(), environment);
        }
    }
    Ok(environment_map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::environment::MockEnvironmentLogic;

    #[derive(Debug, Clone)]
    struct MockStage {
        id: Uuid,
        environment_id: Uuid,
        order_index: i32,
        position: String,
        parent_stage_id: Option<Uuid>,
    }

    impl DBStage for MockStage {
        fn get_id(&self) -> Uuid {
            self.id
        }

        fn order_index(&self) -> i32 {
            self.order_index
        }

        fn parent_stage_id(&self) -> Option<Uuid> {
            self.parent_stage_id
        }

        fn environment_id(&self) -> Uuid {
            self.environment_id
        }

        fn position(&self) -> String {
            self.position.clone()
        }

        fn enabled(&self) -> bool {
            true // Mock stages are always enabled
        }
    }

    impl Stage for MockStage {}

    #[derive(Debug, Clone)]
    struct MockRelationship {
        source_id: i32,
        target_id: i32,
    }

    impl Relationship for MockRelationship {}

    #[test]
    fn test_create_relationships_no_relationships() {
        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::new_v4(),
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::new_v4(),
                order_index: 2,
                position: "second".to_string(),
                parent_stage_id: Some(Uuid::new_v4()),
            }),
        ];

        let relationships =
            create_relationships(true, stages, |target_id, source_id| MockRelationship {
                target_id,
                source_id,
            });

        assert_eq!(relationships.len(), 0);
    }

    #[test]
    fn test_create_relationships_with_relationships() {
        let parent_stage_id = Uuid::new_v4();
        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: parent_stage_id,
                environment_id: Uuid::new_v4(),
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::new_v4(),
                order_index: 2,
                position: "second".to_string(),
                parent_stage_id: Some(parent_stage_id),
            }),
        ];

        let relationships =
            create_relationships(true, stages, |target_id, source_id| MockRelationship {
                target_id,
                source_id,
            });

        assert_eq!(relationships.len(), 1);
        assert_eq!(relationships[0].source_id, 2);
        assert_eq!(relationships[0].target_id, 1);
    }

    #[test]
    fn test_map_stages_no_stages() {
        let environment_map: HashMap<Uuid, Environment> = HashMap::new();
        let stages: Vec<Box<dyn DBStage>> = vec![];

        let mapped_stages: Vec<MockStage> = map_stages(
            false,
            &environment_map,
            &stages,
            |id, environment, order_index, position, _| MockStage {
                id: id.parse().unwrap(),
                environment_id: environment.id.parse().unwrap(),
                order_index,
                position,
                parent_stage_id: None,
            },
        );

        assert!(mapped_stages.is_empty());
    }

    #[test]
    fn test_map_stages_with_stages() {
        let mut environment_map: HashMap<Uuid, Environment> = HashMap::new();
        let env_first_id = "52e4cfe1-b20b-4ad1-9ede-24c771cdef9d";
        let env_second_id = "56f618e6-0303-47c4-bb53-d13e0e24e4cb";
        environment_map.insert(
            Uuid::parse_str(env_first_id).unwrap(),
            Environment {
                id: ID::from(env_first_id),
                name: "Test Environment".to_string(),
                team_id: Default::default(),
                active: true,
            },
        );

        environment_map.insert(
            Uuid::parse_str(env_second_id).unwrap(),
            Environment {
                id: ID::from(env_second_id),
                name: "Another Environment".to_string(),
                team_id: Default::default(),
                active: true,
            },
        );

        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::parse_str(env_first_id).unwrap(),
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::parse_str(env_second_id).unwrap(),
                order_index: 2,
                position: "second".to_string(),
                parent_stage_id: Some(Uuid::new_v4()),
            }),
        ];

        let mapped_stages: Vec<MockStage> = map_stages(
            true,
            &environment_map,
            &stages,
            |id, environment, order_index, position, enabled| MockStage {
                id: id.parse().unwrap(),
                environment_id: environment.id.parse().unwrap(),
                order_index,
                position,
                parent_stage_id: None,
            },
        );

        assert_eq!(mapped_stages.len(), 2);
        assert_eq!(mapped_stages[0].order_index, 1);
        assert_eq!(mapped_stages[1].order_index, 2);
        assert_eq!(mapped_stages[0].position, "first");
        assert_eq!(mapped_stages[1].position, "second");
    }

    #[tokio::test]
    async fn test_get_environment_map_has_stage_false() {
        let mock_env_logic = MockEnvironmentLogic::new();
        // No expectations set because the function shouldn't call get_environment_by_id
        
        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: Uuid::new_v4(),
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
        ];

        let result = get_environment_map(&mock_env_logic, &stages, false).await;
        
        assert!(result.is_ok());
        let env_map = result.unwrap();
        assert_eq!(env_map.len(), 0);
    }

    #[tokio::test]
    async fn test_get_environment_map_same_environment_id() {
        let mut mock_env_logic = MockEnvironmentLogic::new();
        let env_id = Uuid::new_v4();
        let env_id_str = env_id.to_string();
        
        // Set expectation for get_environment_by_id to be called once
        mock_env_logic
            .expect_get_environment_by_id()
            .withf(move |id| id.to_string() == env_id_str)
            .times(1)
            .returning(move |id| {
                Ok(Environment {
                    id,
                    name: "Test Environment".to_string(),
                    team_id: Default::default(),
                    active: true,
                })
            });
        
        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: env_id,
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: env_id, // Same environment ID
                order_index: 2,
                position: "second".to_string(),
                parent_stage_id: None,
            }),
        ];

        let result = get_environment_map(&mock_env_logic, &stages, true).await;
        
        assert!(result.is_ok());
        let env_map = result.unwrap();
        assert_eq!(env_map.len(), 1);
        assert!(env_map.contains_key(&env_id));
    }

    #[tokio::test]
    async fn test_get_environment_map_different_environment_ids() {
        let mut mock_env_logic = MockEnvironmentLogic::new();
        let env_id1 = Uuid::new_v4();
        let env_id1_str = env_id1.to_string();
        let env_id2 = Uuid::new_v4();
        let env_id2_str = env_id2.to_string();
        
        // Set expectations for get_environment_by_id to be called for each environment ID
        mock_env_logic
            .expect_get_environment_by_id()
            .withf(move |id| id.to_string() == env_id1_str)
            .times(1)
            .returning(move |id| {
                Ok(Environment {
                    id,
                    name: "Environment 1".to_string(),
                    team_id: Default::default(),
                    active: true,
                })
            });
            
        mock_env_logic
            .expect_get_environment_by_id()
            .withf(move |id| id.to_string() == env_id2_str)
            .times(1)
            .returning(move |id| {
                Ok(Environment {
                    id,
                    name: "Environment 2".to_string(),
                    team_id: Default::default(),
                    active: true,
                })
            });
        
        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: env_id1,
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: env_id2, // Different environment ID
                order_index: 2,
                position: "second".to_string(),
                parent_stage_id: None,
            }),
        ];

        let result = get_environment_map(&mock_env_logic, &stages, true).await;
        
        assert!(result.is_ok());
        let env_map = result.unwrap();
        assert_eq!(env_map.len(), 2);
        assert!(env_map.contains_key(&env_id1));
        assert!(env_map.contains_key(&env_id2));
        assert_eq!(env_map.get(&env_id1).unwrap().name, "Environment 1");
        assert_eq!(env_map.get(&env_id2).unwrap().name, "Environment 2");
    }

    #[tokio::test]
    async fn test_get_environment_map_error_handling() {
        let mut mock_env_logic = MockEnvironmentLogic::new();
        let env_id = Uuid::new_v4();
        let env_id_str = env_id.to_string();
        
        // Set expectation for get_environment_by_id to return an error
        mock_env_logic
            .expect_get_environment_by_id()
            .withf(move |id| id.to_string() == env_id_str)
            .times(1)
            .returning(move |_| Err(Error::NotFound(env_id)));
        
        let stages: Vec<Box<dyn DBStage>> = vec![
            Box::new(MockStage {
                id: Uuid::new_v4(),
                environment_id: env_id,
                order_index: 1,
                position: "first".to_string(),
                parent_stage_id: None,
            }),
        ];

        let result = get_environment_map(&mock_env_logic, &stages, true).await;
        
        assert!(result.is_err());
        match result {
            Err(Error::NotFound(uuid)) => assert_eq!(uuid, env_id),
            _ => panic!("Expected NotFound error"),
        }
    }
}
