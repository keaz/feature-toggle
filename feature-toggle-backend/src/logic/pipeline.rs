use crate::database::Error;
use crate::database::pipeline::PipelineRepository;
use feature_toggle_shared::graphql::{CreatePipelineInput, Pipeline, UpdatePipelineInput};
use uuid::Uuid;

#[async_trait::async_trait]
pub trait PipelineLogic: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: Uuid) -> Result<Pipeline, Error>;
    async fn get_pipelines(
        &self,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error>;

    async fn create_pipeline(&self, input: CreatePipelineInput) -> Result<Pipeline, Error>;
    async fn update_pipeline(&self, input: UpdatePipelineInput) -> Result<Pipeline, Error>;
    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error>;
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
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Pipeline>, Error> {
        let pipelines = self.repository.get_pipelines(name, active).await?;
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

    async fn create_pipeline(&self, input: CreatePipelineInput) -> Result<Pipeline, Error> {
        let input = crate::database::pipeline::CreatePipeline { name: input.name };
        let pipeline = self.repository.create_pipeline(input).await?;
        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            stages: vec![],
        })
    }

    async fn update_pipeline(&self, input: UpdatePipelineInput) -> Result<Pipeline, Error> {
        let input = crate::database::pipeline::UpdatePipeline {
            id: Uuid::try_from(input.id).unwrap(),
            name: input.name,
            active: input.active,
        };
        let pipeline = self.repository.update_pipeline(input).await?;
        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            stages: vec![],
        })
    }

    async fn delete_pipeline(&self, id: Uuid) -> Result<(), Error> {
        self.repository.delete_pipeline(id).await?;
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
        };
        repository
            .expect_create_pipeline()
            .withf(|input| input.name == "New Pipeline")
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Pipeline {
                    id: Uuid::new_v4(),
                    name: "New Pipeline".to_string(),
                    active: true,
                })
            });

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.create_pipeline(input).await;

        assert!(result.is_ok());
        let pipeline = result.unwrap();
        assert_eq!(pipeline.name, "New Pipeline");
        assert!(pipeline.active);
    }

    #[tokio::test]
    async fn test_update_pipeline() {
        let mut repository = MockPipelineRepository::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        const NAME: &str = "Updated Pipeline";

        let input = UpdatePipelineInput {
            id: ID::from(ID),
            name: Some(NAME.to_string()),
            active: Some(true),
        };

        repository
            .expect_update_pipeline()
            .withf(|input| input.id == input.id && input.name == Some(NAME.to_string()))
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Pipeline {
                    id: Uuid::parse_str(ID).unwrap(),
                    name: "Updated Pipeline".to_string(),
                    active: true,
                })
            });

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.update_pipeline(input).await;

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
            id: ID::from(ID),
            name: Some("Non-existing Pipeline".to_string()),
            active: Some(false),
        };

        repository
            .expect_update_pipeline()
            .withf(|input| input.id == input.id)
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.update_pipeline(input).await;

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
        let id = Uuid::parse_str(ID).unwrap();
        let result = logic.delete_pipeline(id).await;

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
        let id = Uuid::parse_str(ID).unwrap();
        let result = logic.delete_pipeline(id).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_get_pipelines() {
        let mut repository = MockPipelineRepository::new();

        repository
            .expect_get_pipelines()
            .withf(|name, active| name.is_none() && active.is_none())
            .times(1)
            .returning(move |_, _| {
                Ok(vec![
                    crate::database::entity::Pipeline {
                        id: Uuid::new_v4(),
                        name: "Test Pipeline".to_string(),
                        active: true,
                    },
                    crate::database::entity::Pipeline {
                        id: Uuid::new_v4(),
                        name: "Another Pipeline".to_string(),
                        active: false,
                    },
                ])
            });

        let logic = pipeline_logic(Box::new(repository));
        let result = logic.get_pipelines(None, None).await;
        assert!(result.is_ok());
    }
}
