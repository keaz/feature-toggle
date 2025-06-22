use crate::database::environment::EnvironmentRepository;
use crate::database::pipeline::{CreatePipeline, CreateStage, PipelineRepository, UpdatePipeline};
use crate::graphql::schema::{CreatePipelineInput, CreateRelationshipInput, CreateStageInput, Pipeline, PipelineStage, UpdatePipelineInput};
use crate::logic::environment::EnvironmentLogic;
use crate::Error;
use async_graphql::ID;
use std::collections::HashMap;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait PipelineLogic: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error>;
    async fn get_pipelines(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
    ) -> Result<Vec<Pipeline>, Error>;

    async fn create_pipeline(&self, team_id: ID, input: CreatePipelineInput) -> Result<ID, Error>;
    async fn update_pipeline(&self, id: ID, input: UpdatePipelineInput) -> Result<Pipeline, Error>;
    async fn delete_pipeline(&self, id: ID) -> Result<(), Error>;
    fn clone_box(&self) -> Box<dyn PipelineLogic>;
}

impl Clone for Box<dyn PipelineLogic> {
    fn clone(&self) -> Box<dyn PipelineLogic> {
        self.clone_box()
    }
}

pub fn pipeline_logic(repository: Box<dyn PipelineRepository>, environment_logic: Box<dyn EnvironmentLogic>) -> Box<dyn PipelineLogic> {
    Box::new(PipelineLogicImpl { repository, environment_logic })
}

#[derive(Clone)]
struct PipelineLogicImpl {
    repository: Box<dyn PipelineRepository>,
    environment_logic: Box<dyn EnvironmentLogic>
}

impl PipelineLogicImpl {
    fn map_to_update_pipeline(id: ID, input: UpdatePipelineInput) -> UpdatePipeline {
        let id = Uuid::try_from(id).unwrap();
        let mut update = UpdatePipeline {
            id,
            name: input.name,
            active: input.active,
            stages: vec![],
        };

        update.stages = get_stages_to_create(input.stages, input.relationships);
        update
    }
}

#[async_trait::async_trait]
impl PipelineLogic for PipelineLogicImpl {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error> {
        let pipeline = self.repository.get_pipeline_by_id(env_id).await?;
        //
        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            stages: vec![], //#FIXME: Stages are not included in this mapping
            relationships: vec![], // #FIXME: Relationships are not included in this mapping
        })
    }

    async fn get_pipelines(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
    ) -> Result<Vec<Pipeline>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let pipelines = self.repository.get_pipelines(team_id, name, active).await?;
        let has_stage = fields.contains(&"stages".to_string());

        let mut environment_map = HashMap::new();
        for pipeline in &pipelines {
            for stage in &pipeline.stages {
                if has_stage && !environment_map.contains_key(&stage.environment_id) {
                    let environment = self.environment_logic.get_environment_by_id(stage.environment_id).await?;
                    environment_map.insert(stage.environment_id, environment);
                }
            }
        }

        Ok(pipelines
            .into_iter()
            .map(|pipeline| {
                let stages = if has_stage {
                    pipeline.stages.iter().map(|stage| {
                        PipelineStage {
                            id: stage.id.into(),
                            environment: environment_map.get(&stage.environment_id).unwrap().to_owned(),
                            order: stage.order_index,
                        }
                    }).collect()
                } else {
                    vec![]
                };
                Pipeline {
                    id: pipeline.id.into(),
                    name: pipeline.name,
                    active: pipeline.active,
                    stages,
                    relationships: vec![], // Relationships are not included in this mapping
                }
            })
            .collect())
    }

    async fn create_pipeline(&self, team_id: ID, input: CreatePipelineInput) -> Result<ID, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let input = map_to_create_pipeline(team_id, input);
        let pipeline = self.repository.create_pipeline(input).await?;
        Ok(ID::from(pipeline.to_string()))
    }

    async fn update_pipeline(&self, id: ID, input: UpdatePipelineInput) -> Result<Pipeline, Error> {
        let input = Self::map_to_update_pipeline(id, input);
        let pipeline = self.repository.update_pipeline(input).await?;
        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            stages: vec![], //#FIXME: Stages are not included in this mapping
            relationships: vec![], //#FIXME: Relationships are not included in this mapping
        })
    }

    async fn delete_pipeline(&self, id: ID) -> Result<(), Error> {
        self.repository
            .delete_pipeline(Uuid::try_from(id).unwrap())
            .await?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn PipelineLogic> {
        Box::new(self.clone())
    }
}

fn map_to_create_pipeline(team_id: Uuid, input: CreatePipelineInput) -> CreatePipeline {
    let mut pipeline = CreatePipeline {
        team_id,
        name: input.name.clone(),
        stages: vec![],
    };

    let stages = get_stages_to_create(input.stages, input.relationships);
    pipeline.stages = stages;
    pipeline
}

fn get_stages_to_create(
    stages: Vec<CreateStageInput>,
    relationships: Vec<CreateRelationshipInput>,
) -> Vec<CreateStage> {
    let mut stages = stages
        .into_iter()
        .map(|stage| {
            CreateStage::new(Uuid::new_v4(), Uuid::try_from(stage.environment_id.clone()).unwrap(), stage.order_index, None, stage.position)
        })
        .collect::<Vec<CreateStage>>();

    let cloned_stages = stages.clone();
    let relationships_map = relationships.iter().map(|relationship| {
        let stage = cloned_stages.iter().find(|stage| {
            stage.order_index == relationship.source_id
        });
        (relationship.source_id, relationship.target_id, stage.unwrap()) // Stage should always be present
    }).collect::<Vec<(i32, i32, &CreateStage)>>();

    for (_, target_id, stage) in relationships_map {
        if let Some(target_stage) = stages.iter_mut().find(|s| s.order_index == target_id) {
            target_stage.parent_stage = Some(Box::new(stage.clone()));
        }
    }

    stages
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::database::pipeline::MockPipelineRepository;
    use crate::graphql::schema::{CreateRelationshipInput, CreateStageInput};
    use crate::logic::environment::MockEnvironmentLogic;
    use async_graphql::ID;

    #[test]
    pub fn test_map_to_create_pipeline_with_single_stage() {
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let input = CreatePipelineInput {
            name: "Test Pipeline".to_string(),
            stages: vec![
                CreateStageInput {
                    environment_id: ID::from("3eef17bc-9e06-411d-b5f4-7a786e68bb96"),
                    order_index: 0,
                    position: "".to_string()
                },
            ],
            relationships: vec![],
        };
        let create_pipeline = map_to_create_pipeline(team_id, input);
        assert_eq!(create_pipeline.name, "Test Pipeline");
        assert_eq!(create_pipeline.team_id, team_id);
        assert_eq!(create_pipeline.stages.len(), 1);
        assert!(create_pipeline.stages[0].parent_stage.is_none());
    }

    #[test]
    pub fn test_map_to_create_pipeline_with_relationships() {
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let relationships = vec![
            CreateRelationshipInput {
                source_id: 0,
                target_id: 1,
            },
            CreateRelationshipInput {
                source_id: 1,
                target_id: 2,
            },
            CreateRelationshipInput {
                source_id: 0,
                target_id: 3,
            },
            CreateRelationshipInput {
                source_id: 3,
                target_id: 4,
            }
        ];
        let input = CreatePipelineInput {
            name: "Test Pipeline".to_string(),
            stages: vec![
                CreateStageInput {
                    environment_id: ID::from("e74a6c91-33b7-467f-b2ec-b01434a0bc96"),
                    order_index: 0,
                    position: "".to_string()
                },
                CreateStageInput {
                    environment_id: ID::from("81cf8b7d-4945-4a30-96a2-e27559e97fac"),
                    order_index: 1,
                    position: "".to_string()
                },
                CreateStageInput {
                    environment_id: ID::from("13728519-a82b-4987-b82a-3fb57652388f"),
                    order_index: 2,
                    position: "".to_string()
                },
                CreateStageInput {
                    environment_id: ID::from("cb1d22be-bc57-4626-abf2-7534de556586"),
                    order_index: 3,
                    position: "".to_string()
                },
                CreateStageInput {
                    environment_id: ID::from("06f28625-df1d-499f-a4ee-5629a8b6a169"),
                    order_index: 4,
                    position: "".to_string()
                }
            ],
            relationships,
        };
        let create_pipeline = map_to_create_pipeline(team_id, input);
        assert_eq!(create_pipeline.name, "Test Pipeline");
        assert_eq!(create_pipeline.team_id, team_id);
        let stages = create_pipeline.stages;
        assert_eq!(stages.len(), 5);
        assert!(stages[0].parent_stage.is_none());

        let parent_stage = &stages.get(1).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(parent_stage.environment_id, Uuid::parse_str("e74a6c91-33b7-467f-b2ec-b01434a0bc96").unwrap());

        let parent_stage = &stages.get(2).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(parent_stage.environment_id, Uuid::parse_str("81cf8b7d-4945-4a30-96a2-e27559e97fac").unwrap());

        let parent_stage = &stages.get(3).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(parent_stage.environment_id, Uuid::parse_str("e74a6c91-33b7-467f-b2ec-b01434a0bc96").unwrap());

        let parent_stage = &stages.get(4).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(parent_stage.environment_id, Uuid::parse_str("cb1d22be-bc57-4626-abf2-7534de556586").unwrap());
    }

    #[tokio::test]
    async fn test_get_pipeline_by_id() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_get_pipeline_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Pipeline {
                    id: Uuid::parse_str(ID).unwrap(),
                    name: "Test Pipeline".to_string(),
                    active: true,
                    team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                    stages: vec![],
                })
            });

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let id = Uuid::parse_str(ID).unwrap();
        let result = logic.get_pipeline_by_id(id).await;
        assert!(result.is_ok());
        let pipeline = result.unwrap();
        assert_eq!(pipeline.id.to_string(), ID);
    }

    #[tokio::test]
    async fn test_get_non_existing_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();
        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        repository
            .expect_get_pipeline_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let id = Uuid::parse_str(ID).unwrap();
        let result = logic.get_pipeline_by_id(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_create_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        let relationships = vec![];
        let input = CreatePipelineInput {
            name: "New Pipeline".to_string(),
            stages: vec![],
            relationships,
        };
        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        // let id = ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27");
        let id = Uuid::parse_str(ID).unwrap();
        repository
            .expect_create_pipeline()
            .withf(|input| input.name == "New Pipeline")
            .times(1)
            .returning(move |_| Ok(id));

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let result = logic
            .create_pipeline(ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"), input)
            .await;

        assert!(result.is_ok());
        let pipeline_id = result.unwrap();
        assert_eq!(pipeline_id, ID::from(ID));
    }

    #[tokio::test]
    async fn test_update_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        const NAME: &str = "Updated Pipeline";

        let stages = vec![CreateStageInput {
            environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
            order_index: 0,
            position: "".to_string()
        }];

        let input = UpdatePipelineInput {
            name: Some(NAME.to_string()),
            active: Some(true),
            stages,
            relationships: vec![]
        };

        repository
            .expect_update_pipeline()
            .withf(|input| {
                input.id == Uuid::parse_str(ID).unwrap() && input.name == Some(NAME.to_string())
            })
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Pipeline {
                    id: Uuid::parse_str(ID).unwrap(),
                    name: "Updated Pipeline".to_string(),
                    active: true,
                    team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                    stages: vec![crate::database::entity::Stage {
                        id: Uuid::parse_str(ID).unwrap(),
                        pipeline_id: Uuid::parse_str(ID).unwrap(),
                        environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27")
                            .unwrap(),
                        order_index: 0,
                        parent_stage_id: None,
                    }],
                })
            });

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let result = logic.update_pipeline(ID::from(ID), input).await;

        assert!(result.is_ok());
        let pipeline = result.unwrap();
        assert_eq!(pipeline.name, "Updated Pipeline");
        assert!(pipeline.active);
    }

    #[tokio::test]
    async fn test_not_existing_pipeline_update() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        let input = UpdatePipelineInput {
            name: Some("Non-existing Pipeline".to_string()),
            active: Some(false),
            stages: vec![],
            relationships: vec![]
        };

        repository
            .expect_update_pipeline()
            .withf(|input| input.id == input.id)
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let result = logic.update_pipeline(ID::from(ID), input).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_delete_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_delete_pipeline()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let result = logic.delete_pipeline(ID::from(ID)).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_non_existing_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        repository
            .expect_delete_pipeline()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let result = logic.delete_pipeline(ID::from(ID)).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_get_pipelines() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        repository
            .expect_get_pipelines()
            .withf(|team_id, name, active| name.is_none() && active.is_none())
            .times(1)
            .returning(move |_, _, _| {
                Ok(vec![
                    crate::database::entity::Pipeline {
                        id: Uuid::new_v4(),
                        name: "Test Pipeline".to_string(),
                        active: true,
                        team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                        stages: vec![],
                    },
                    crate::database::entity::Pipeline {
                        id: Uuid::new_v4(),
                        name: "Another Pipeline".to_string(),
                        active: false,
                        team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                        stages: vec![],
                    },
                ])
            });

        let logic = pipeline_logic(Box::new(repository), Box::new(environment_repo));
        let result = logic
            .get_pipelines(ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"), None, None, vec![])
            .await;
        assert!(result.is_ok());
    }
}
