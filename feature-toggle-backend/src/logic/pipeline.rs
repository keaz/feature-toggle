use crate::Error;
use crate::database::entity::DBStage;
use crate::database::pipeline::{CreatePipeline, CreateStage, PipelineRepository, UpdatePipeline};
use crate::model::{
    CreatePipelineInput, CreateRelationshipInput, CreateStageInput, Environment, Pipeline,
    PipelineRelationship, PipelineStage, UpdatePipelineInput,
};
use crate::logic::environment::EnvironmentLogic;
use crate::logic::stage_builder::{build_stage_relationships, id_to_uuid};
use crate::logic::{create_relationships, get_environment_map, map_stages};
use crate::model::ID;
use uuid::Uuid;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
#[async_trait::async_trait]
pub trait PipelineLogic: Send + Sync {
    async fn get_pipeline_by_id(&self, env_id: ID) -> Result<Pipeline, Error>;
    async fn get_pipelines(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
    ) -> Result<Vec<Pipeline>, Error>;

    async fn get_pipelines_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Pipeline>, i64), Error>;
    async fn get_pipelines_with_offset(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Pipeline>, i64), Error>;

    async fn create_pipeline(
        &self,
        team_id: ID,
        input: CreatePipelineInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ID, Error>;
    async fn update_pipeline(
        &self,
        id: ID,
        input: UpdatePipelineInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Pipeline, Error>;
    async fn delete_pipeline(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error>;
    fn clone_box(&self) -> Box<dyn PipelineLogic>;
}

impl Clone for Box<dyn PipelineLogic> {
    fn clone(&self) -> Box<dyn PipelineLogic> {
        self.clone_box()
    }
}

pub fn pipeline_logic(
    repository: Box<dyn PipelineRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
) -> Box<dyn PipelineLogic> {
    Box::new(PipelineLogicImpl {
        repository,
        environment_logic,
        activity_log_repository,
    })
}

struct PipelineLogicImpl {
    repository: Box<dyn PipelineRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
}

impl Clone for PipelineLogicImpl {
    fn clone(&self) -> Self {
        Self {
            repository: self.repository.clone_box(),
            environment_logic: self.environment_logic.clone_box(),
            activity_log_repository: self.activity_log_repository.clone_box(),
        }
    }
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
    async fn get_pipeline_by_id(&self, env_id: ID) -> Result<Pipeline, Error> {
        let pipeline_id = Uuid::try_from(env_id).unwrap();
        let pipeline = self.repository.get_pipeline_by_id(pipeline_id).await?;
        let pipelines = vec![pipeline.clone()]; // Wrap in a vector to reuse the same logic

        let stages = pipelines
            .iter()
            .flat_map(|feature| &feature.stages)
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect::<Vec<Box<dyn DBStage>>>();

        let environment_map = get_environment_map(&*self.environment_logic, &stages, true).await?;
        let db_stages = pipeline
            .stages
            .iter()
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect();

        let stages = map_stages(true, &environment_map, &db_stages, stage_factory);
        let relationships = create_relationships(true, db_stages, relationship_factory);

        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            stages,
            relationships,
            team_id: pipeline.team_id.into(),
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

        let stages = pipelines
            .iter()
            .flat_map(|feature| &feature.stages)
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect::<Vec<Box<dyn DBStage>>>();

        let environment_map = get_environment_map(&*self.environment_logic, &stages, true).await?;

        Ok(pipelines
            .into_iter()
            .map(|pipeline| {
                let db_stages = pipeline
                    .stages
                    .iter()
                    .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
                    .collect();
                let stages = map_stages(has_stage, &environment_map, &db_stages, stage_factory);
                let relationships =
                    create_relationships(has_stage, db_stages, relationship_factory);
                Pipeline {
                    id: pipeline.id.into(),
                    name: pipeline.name,
                    active: pipeline.active,
                    stages,
                    relationships,
                    team_id: pipeline.team_id.into(),
                }
            })
            .collect())
    }

    async fn get_pipelines_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Pipeline>, i64), Error> {
        let team_id = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let (pipelines, total) = self
            .repository
            .get_pipelines_paginated(team_id, name, active, page_number, page_size)
            .await?;
        let has_stage = fields.contains(&"stages".to_string());

        let stages = pipelines
            .iter()
            .flat_map(|feature| &feature.stages)
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect::<Vec<Box<dyn DBStage>>>();

        let environment_map = get_environment_map(&*self.environment_logic, &stages, true).await?;

        let mapped_pipelines = pipelines
            .into_iter()
            .map(|pipeline| {
                let db_stages = pipeline
                    .stages
                    .iter()
                    .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
                    .collect();
                let stages = map_stages(has_stage, &environment_map, &db_stages, stage_factory);
                let relationships =
                    create_relationships(has_stage, db_stages, relationship_factory);
                Pipeline {
                    id: pipeline.id.into(),
                    name: pipeline.name,
                    active: pipeline.active,
                    stages,
                    relationships,
                    team_id: pipeline.team_id.into(),
                }
            })
            .collect();

        Ok((mapped_pipelines, total))
    }

    async fn get_pipelines_with_offset(
        &self,
        team_id: ID,
        name: Option<String>,
        active: Option<bool>,
        fields: Vec<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Pipeline>, i64), Error> {
        let team_id = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let (pipelines, total) = self
            .repository
            .get_pipelines_with_offset(team_id, name, active, offset, limit)
            .await?;
        let has_stage = fields.contains(&"stages".to_string());

        let stages = pipelines
            .iter()
            .flat_map(|feature| &feature.stages)
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect::<Vec<Box<dyn DBStage>>>();

        let environment_map = get_environment_map(&*self.environment_logic, &stages, true).await?;

        let mapped_pipelines = pipelines
            .into_iter()
            .map(|pipeline| {
                let db_stages = pipeline
                    .stages
                    .iter()
                    .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
                    .collect();
                let stages = map_stages(has_stage, &environment_map, &db_stages, stage_factory);
                let relationships =
                    create_relationships(has_stage, db_stages, relationship_factory);
                Pipeline {
                    id: pipeline.id.into(),
                    name: pipeline.name,
                    active: pipeline.active,
                    stages,
                    relationships,
                    team_id: pipeline.team_id.into(),
                }
            })
            .collect();

        Ok((mapped_pipelines, total))
    }

    async fn create_pipeline(
        &self,
        team_id: ID,
        input: CreatePipelineInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ID, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let pipeline_name = input.name.clone();
        let input_mapped = map_to_create_pipeline(team_id, input);
        let pipeline = self.repository.create_pipeline(input_mapped).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_pipeline_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::PIPELINE_CREATED,
            &pipeline.to_string(),
            actor_id,
            actor_name,
            format!("Created pipeline '{}'", pipeline_name),
            Some(serde_json::json!({
                "pipeline_id": pipeline.to_string(),
                "pipeline_name": pipeline_name,
                "team_id": team_id.to_string(),
            })),
        )
        .await;

        Ok(ID::from(pipeline.to_string()))
    }

    async fn update_pipeline(
        &self,
        id: ID,
        input: UpdatePipelineInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<Pipeline, Error> {
        let input_mapped = Self::map_to_update_pipeline(id, input);
        let pipeline = self.repository.update_pipeline(input_mapped).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_pipeline_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::PIPELINE_UPDATED,
            &pipeline.id.to_string(),
            actor_id,
            actor_name,
            format!("Updated pipeline '{}'", pipeline.name),
            Some(serde_json::json!({
                "pipeline_id": pipeline.id.to_string(),
                "pipeline_name": pipeline.name.clone(),
                "active": pipeline.active,
            })),
        )
        .await;

        Ok(Pipeline {
            id: pipeline.id.into(),
            name: pipeline.name,
            active: pipeline.active,
            team_id: pipeline.team_id.into(),
            stages: vec![],        //#FIXME: Stages are not included in this mapping
            relationships: vec![], //#FIXME: Relationships are not included in this mapping
        })
    }

    async fn delete_pipeline(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error> {
        let pipeline_id = Uuid::try_from(id).unwrap();

        // Get pipeline name before deletion for activity log
        let pipeline = self.repository.get_pipeline_by_id(pipeline_id).await?;

        self.repository.delete_pipeline(pipeline_id).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_pipeline_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::PIPELINE_DELETED,
            &pipeline_id.to_string(),
            actor_id,
            actor_name,
            format!("Deleted pipeline '{}'", pipeline.name),
            Some(serde_json::json!({
                "pipeline_id": pipeline_id.to_string(),
                "pipeline_name": pipeline.name,
            })),
        )
        .await;

        Ok(())
    }

    fn clone_box(&self) -> Box<dyn PipelineLogic> {
        Box::new(self.clone())
    }
}

fn relationship_factory(source_id: i32, target_id: i32) -> PipelineRelationship {
    PipelineRelationship {
        source_id,
        target_id,
    }
}

fn stage_factory(
    id: ID,
    environment: Environment,
    order_index: i32,
    position: String,
) -> PipelineStage {
    PipelineStage {
        id,
        environment,
        order_index,
        position,
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
    let stages = stages
        .into_iter()
        .map(|stage| {
            CreateStage::new(
                Uuid::new_v4(),
                id_to_uuid(stage.environment_id).unwrap(),
                stage.order_index,
                None,
                stage.position,
            )
        })
        .collect::<Vec<CreateStage>>();

    // Use shared relationship building logic
    build_stage_relationships(stages, relationships)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::database::activity_log::{ActivityLogRepository, MockActivityLogRepository};
    use crate::database::pipeline::MockPipelineRepository;
    use crate::model::{CreateRelationshipInput, CreateStageInput};
    use crate::logic::environment::MockEnvironmentLogic;
    use crate::model::ID;

    fn create_mock_activity_log() -> Box<dyn ActivityLogRepository> {
        let mut mock = MockActivityLogRepository::new();
        mock.expect_create_activity().returning(|_| {
            Ok(crate::database::activity_log::ActivityLogRow {
                id: uuid::Uuid::new_v4(),
                activity_type: "TEST".to_string(),
                entity_type: "test".to_string(),
                entity_id: "test".to_string(),
                actor_id: None,
                actor_name: None,
                description: "test".to_string(),
                metadata: None,
                created_at: chrono::Utc::now(),
            })
        });
        mock.expect_clone_box()
            .returning(|| create_mock_activity_log());
        Box::new(mock)
    }

    #[test]
    pub fn test_map_to_create_pipeline_with_single_stage() {
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let input = CreatePipelineInput {
            name: "Test Pipeline".to_string(),
            stages: vec![CreateStageInput {
                environment_id: ID::from("3eef17bc-9e06-411d-b5f4-7a786e68bb96"),
                order_index: 0,
                position: "".to_string(),
            }],
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
            },
        ];
        let input = CreatePipelineInput {
            name: "Test Pipeline".to_string(),
            stages: vec![
                CreateStageInput {
                    environment_id: ID::from("e74a6c91-33b7-467f-b2ec-b01434a0bc96"),
                    order_index: 0,
                    position: "".to_string(),
                },
                CreateStageInput {
                    environment_id: ID::from("81cf8b7d-4945-4a30-96a2-e27559e97fac"),
                    order_index: 1,
                    position: "".to_string(),
                },
                CreateStageInput {
                    environment_id: ID::from("13728519-a82b-4987-b82a-3fb57652388f"),
                    order_index: 2,
                    position: "".to_string(),
                },
                CreateStageInput {
                    environment_id: ID::from("cb1d22be-bc57-4626-abf2-7534de556586"),
                    order_index: 3,
                    position: "".to_string(),
                },
                CreateStageInput {
                    environment_id: ID::from("06f28625-df1d-499f-a4ee-5629a8b6a169"),
                    order_index: 4,
                    position: "".to_string(),
                },
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
        assert_eq!(
            parent_stage.environment_id,
            Uuid::parse_str("e74a6c91-33b7-467f-b2ec-b01434a0bc96").unwrap()
        );

        let parent_stage = &stages.get(2).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(
            parent_stage.environment_id,
            Uuid::parse_str("81cf8b7d-4945-4a30-96a2-e27559e97fac").unwrap()
        );

        let parent_stage = &stages.get(3).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(
            parent_stage.environment_id,
            Uuid::parse_str("e74a6c91-33b7-467f-b2ec-b01434a0bc96").unwrap()
        );

        let parent_stage = &stages.get(4).unwrap().parent_stage;
        assert!(parent_stage.is_some());
        let parent_stage = parent_stage.as_ref().unwrap();
        assert_eq!(
            parent_stage.environment_id,
            Uuid::parse_str("cb1d22be-bc57-4626-abf2-7534de556586").unwrap()
        );
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

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic.get_pipeline_by_id(ID::from(ID)).await;
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

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic.get_pipeline_by_id(ID::from(ID)).await;

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

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic
            .create_pipeline(
                ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                input,
                None,
            )
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
            position: "".to_string(),
        }];

        let input = UpdatePipelineInput {
            name: Some(NAME.to_string()),
            active: Some(true),
            stages,
            relationships: vec![],
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
                    stages: vec![crate::database::entity::PipelineStage {
                        id: Uuid::parse_str(ID).unwrap(),
                        pipeline_id: Uuid::parse_str(ID).unwrap(),
                        environment_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27")
                            .unwrap(),
                        order_index: 0,
                        parent_stage_id: None,
                        position: "".to_string(),
                    }],
                })
            });

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic.update_pipeline(ID::from(ID), input, None).await;

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
            relationships: vec![],
        };

        repository
            .expect_update_pipeline()
            .withf(|input| input.id == input.id)
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic.update_pipeline(ID::from(ID), input, None).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_delete_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        let pipeline_id = Uuid::parse_str(ID).unwrap();

        // Mock get_pipeline_by_id (called for activity logging)
        repository
            .expect_get_pipeline_by_id()
            .withf(move |mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| {
                Ok(crate::database::entity::Pipeline {
                    id: pipeline_id,
                    name: "Test Pipeline".to_string(),
                    active: true,
                    team_id: Uuid::new_v4(),
                    stages: vec![],
                })
            });

        repository
            .expect_delete_pipeline()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic.delete_pipeline(ID::from(ID), None).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_non_existing_pipeline() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        let pipeline_id = Uuid::parse_str(ID).unwrap();

        // Mock get_pipeline_by_id (called for activity logging) - returns error
        repository
            .expect_get_pipeline_by_id()
            .withf(move |mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(pipeline_id)));

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic.delete_pipeline(ID::from(ID), None).await;

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
            .withf(|_, name, active| name.is_none() && active.is_none())
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

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic
            .get_pipelines(
                ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                None,
                None,
                vec![],
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_pipelines_paginated_success() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
        let pipeline1_id = Uuid::new_v4();
        let pipeline2_id = Uuid::new_v4();

        let expected_pipelines = vec![
            crate::database::entity::Pipeline {
                id: pipeline1_id,
                name: "Production Pipeline".to_string(),
                active: true,
                team_id,
                stages: vec![],
            },
            crate::database::entity::Pipeline {
                id: pipeline2_id,
                name: "Development Pipeline".to_string(),
                active: false,
                team_id,
                stages: vec![],
            },
        ];

        repository
            .expect_get_pipelines_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(move |_, _, _, _, _| Ok((expected_pipelines.clone(), 15)));

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let (pipelines, total) = logic
            .get_pipelines_paginated(ID::from(team_id), None, None, vec![], 1, 10)
            .await
            .unwrap();

        assert_eq!(pipelines.len(), 2);
        assert_eq!(total, 15);
        assert_eq!(pipelines[0].name, "Production Pipeline");
        assert_eq!(pipelines[0].active, true);
        assert_eq!(pipelines[1].name, "Development Pipeline");
        assert_eq!(pipelines[1].active, false);
    }

    #[tokio::test]
    async fn test_get_pipelines_paginated_with_filters() {
        let mut repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();
        let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

        repository
            .expect_get_pipelines_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(Some("prod".to_string())),
                mockall::predicate::eq(Some(true)),
                mockall::predicate::eq(2),
                mockall::predicate::eq(5),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok((vec![], 0)));

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let (pipelines, total) = logic
            .get_pipelines_paginated(
                ID::from(team_id),
                Some("prod".to_string()),
                Some(true),
                vec![],
                2,
                5,
            )
            .await
            .unwrap();

        assert_eq!(pipelines.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_pipelines_paginated_invalid_team_id() {
        let repository = MockPipelineRepository::new();
        let environment_repo = MockEnvironmentLogic::new();

        let logic = pipeline_logic(
            Box::new(repository),
            Box::new(environment_repo),
            create_mock_activity_log(),
        );
        let result = logic
            .get_pipelines_paginated(ID::from("invalid-uuid"), None, None, vec![], 1, 10)
            .await;

        assert!(matches!(result, Err(Error::InvalidInput(_))));
    }
}
