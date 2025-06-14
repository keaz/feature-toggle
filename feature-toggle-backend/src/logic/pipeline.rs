use crate::database::pipeline::{
    CreateStage, PipelineRepository, UpdateCreateStage, UpdatePipeline,
};
use crate::{Error, database::pipeline::CreatePipeline};
use async_graphql::ID;
use feature_toggle_shared::graphql::{CreatePipelineInput, Pipeline, UpdatePipelineInput};
use uuid::Uuid;
use uuid::timestamp::UUID_TICKS_BETWEEN_EPOCHS;

#[async_trait::async_trait]
pub trait PipelineLogic: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error>;
    async fn get_pipelines(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
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

pub fn pipeline_logic(repository: Box<dyn PipelineRepository>) -> Box<dyn PipelineLogic> {
    Box::new(PipelineLogicImpl { repository })
}

#[derive(Clone)]
struct PipelineLogicImpl {
    repository: Box<dyn PipelineRepository>,
}

impl PipelineLogicImpl {
    fn map_to_create_pipeline(team_id: Uuid, input: CreatePipelineInput) -> CreatePipeline {
        CreatePipeline {
            team_id,
            name: input.name,
            stages: input
                .stages
                .into_iter()
                .map(|stage| CreateStage {
                    environment_id: Uuid::try_from(stage.environment_id).unwrap(),
                    parent_stage_id: stage.parent_stage_id.map(|id| Uuid::try_from(id).unwrap()),
                    order_index: stage.order,
                })
                .collect(),
        }
    }

    fn map_to_update_pipeline(id: ID, input: UpdatePipelineInput) -> UpdatePipeline {
        let id = Uuid::try_from(id).unwrap();
        UpdatePipeline {
            id: id.clone(),
            name: input.name,
            active: input.active,
            stages: input
                .stages
                .into_iter()
                .map(|stage| UpdateCreateStage {
                    id: Uuid::try_from(id).unwrap(),
                    pipeline_id: id.clone(),
                    environment_id: Uuid::try_from(stage.environment_id).unwrap(),
                    parent_stage_id: stage.parent_stage_id.map(|id| Uuid::try_from(id).unwrap()),
                    order_index: stage.order,
                })
                .collect::<Vec<UpdateCreateStage>>(),
        }
    }
}

#[async_trait::async_trait]
impl PipelineLogic for PipelineLogicImpl {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error> {
        let pipeline = self.repository.get_pipeline_by_id(env_id).await?;
        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            stages: vec![],
        })
    }

    async fn get_pipelines(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let pipelines = self.repository.get_pipelines(team_id, name, active).await?;
        Ok(pipelines
            .into_iter()
            .map(|pipeline| Pipeline {
                id: pipeline.id.into(),
                name: pipeline.name,
                active: pipeline.active,
                stages: vec![],
            })
            .collect())
    }

    async fn create_pipeline(&self, team_id: ID, input: CreatePipelineInput) -> Result<ID, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let input = Self::map_to_create_pipeline(team_id, input);
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
            stages: vec![],
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::database::pipeline::MockPipelineRepository;
    use async_graphql::ID;
    use feature_toggle_shared::graphql::UpdateStageInput;

    #[tokio::test]
    async fn test_get_pipeline_by_id() {
        let mut repository = MockPipelineRepository::new();

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

        let logic = pipeline_logic(Box::new(repository));
        let id = Uuid::parse_str(ID).unwrap();
        let result = logic.get_pipeline_by_id(id).await;
        assert!(result.is_ok());
        let pipeline = result.unwrap();
        assert_eq!(pipeline.id.to_string(), ID);
    }

    #[tokio::test]
    async fn test_get_non_existing_pipeline() {
        let mut repository = MockPipelineRepository::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        repository
            .expect_get_pipeline_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository));
        let id = Uuid::parse_str(ID).unwrap();
        let result = logic.get_pipeline_by_id(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_create_pipeline() {
        let mut repository = MockPipelineRepository::new();

        let input = CreatePipelineInput {
            name: "New Pipeline".to_string(),
            stages: vec![],
        };
        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        // let id = ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27");
        let id = Uuid::parse_str(ID).unwrap();
        repository
            .expect_create_pipeline()
            .withf(|input| input.name == "New Pipeline")
            .times(1)
            .returning(move |_| Ok(id));

        let logic = pipeline_logic(Box::new(repository));
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

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        const NAME: &str = "Updated Pipeline";

        let stages = vec![UpdateStageInput {
            id: ID::from("3eef17bc-9e06-411d-b5f4-7a786e68bb96"),
            environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
            parent_stage_id: None,
            order: 1,
            pipeline_id: ID::from(ID),
        }];

        let input = UpdatePipelineInput {
            name: Some(NAME.to_string()),
            active: Some(true),
            stages: stages,
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
                        order_index: 1,
                        parent_stage_id: None,
                    }],
                })
            });

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.update_pipeline(ID::from(ID), input).await;

        assert!(result.is_ok());
        let pipeline = result.unwrap();
        assert_eq!(pipeline.name, "Updated Pipeline");
        assert!(pipeline.active);
    }

    #[tokio::test]
    async fn test_not_existing_pipeline_update() {
        let mut repository = MockPipelineRepository::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        let input = UpdatePipelineInput {
            name: Some("Non-existing Pipeline".to_string()),
            active: Some(false),
            stages: vec![UpdateStageInput {
                id: ID::from(ID),
                environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                parent_stage_id: None,
                order: 1,
                pipeline_id: ID::from(ID),
            }],
        };

        repository
            .expect_update_pipeline()
            .withf(|input| input.id == input.id)
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.update_pipeline(ID::from(ID), input).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_delete_pipeline() {
        let mut repository = MockPipelineRepository::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_delete_pipeline()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.delete_pipeline(ID::from(ID)).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_non_existing_pipeline() {
        let mut repository = MockPipelineRepository::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        repository
            .expect_delete_pipeline()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.delete_pipeline(ID::from(ID)).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_get_pipelines() {
        let mut repository = MockPipelineRepository::new();

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

        let logic = pipeline_logic(Box::new(repository));
        let result = logic
            .get_pipelines(ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"), None, None)
            .await;
        assert!(result.is_ok());
    }
}
