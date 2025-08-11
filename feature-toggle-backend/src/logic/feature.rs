use crate::database::entity::{DBStage, FeatureType as EntityFeatureType};
use crate::database::feature::{CreateFeature, CreateFeatureStage, FeatureRepository, UpdateFeature};
use crate::graphql::schema::{CreateFeatureInput, CreateFeatureStageInput, CreateRelationshipInput, Environment, Feature, FeatureRelationship, FeatureStage, FeatureType as GraphQLFeatureType, UpdateFeatureInput};
use crate::logic::environment::EnvironmentLogic;
use crate::logic::{create_relationships, get_environment_map, map_stages};
use crate::Error;
use async_graphql::ID;
use uuid::Uuid;

use mockall::automock;

#[automock]
#[async_trait::async_trait]
pub trait FeatureLogic: Send + Sync {
    async fn get_feature_by_id(&self, id: ID) -> Result<Feature, Error>;
    async fn get_features(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<GraphQLFeatureType>,
    ) -> Result<Vec<Feature>, Error>;

    async fn create_feature(&self, team_id: ID, input: CreateFeatureInput) -> Result<ID, Error>;
    async fn update_feature(&self, id: ID, input: UpdateFeatureInput) -> Result<Feature, Error>;
    async fn delete_feature(&self, id: ID) -> Result<(), Error>;

    // Stage-contexts
    async fn get_stage_contexts(&self, stage_id: ID) -> Result<Vec<crate::graphql::schema::Context>, Error>;
    async fn set_stage_contexts(&self, stage_id: ID, context_ids: Vec<ID>) -> Result<Vec<crate::graphql::schema::Context>, Error>;

    // Stage-criteria
    async fn get_stage_criteria(&self, stage_id: ID) -> Result<Vec<crate::graphql::schema::StageCriterion>, Error>;
    async fn set_stage_criteria(&self, stage_id: ID, criteria: Vec<crate::graphql::schema::CreateStageCriterionInput>) -> Result<Vec<crate::graphql::schema::StageCriterion>, Error>;

    fn clone_box(&self) -> Box<dyn FeatureLogic>;
}

impl Clone for Box<dyn FeatureLogic> {
    fn clone(&self) -> Box<dyn FeatureLogic> {
        self.clone_box()
    }
}

pub fn feature_logic(
    repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>,
) -> Box<dyn FeatureLogic> {
    Box::new(FeatureLogicImpl {
        repository,
        environment_logic,
    })
}

#[derive(Clone)]
struct FeatureLogicImpl {
    repository: Box<dyn FeatureRepository>,
    environment_logic: Box<dyn EnvironmentLogic>, // Assuming you have an EnvironmentLogic trait
}

impl FeatureLogicImpl {
    fn map_graphql_to_entity_feature_type(feature_type: GraphQLFeatureType) -> EntityFeatureType {
        match feature_type {
            GraphQLFeatureType::Simple => EntityFeatureType::Simple,
            GraphQLFeatureType::Contextual => EntityFeatureType::Contextual,
        }
    }

    fn map_entity_to_graphql_feature_type(feature_type: EntityFeatureType) -> GraphQLFeatureType {
        match feature_type {
            EntityFeatureType::Simple => GraphQLFeatureType::Simple,
            EntityFeatureType::Contextual => GraphQLFeatureType::Contextual,
        }
    }

    fn map_to_create_feature(team_id: Uuid, input: CreateFeatureInput) -> CreateFeature {
        let feature_type = Self::map_graphql_to_entity_feature_type(input.feature_type);
        let stages = Self::get_create_stages_to_create(input.stages, input.relationships);

        CreateFeature {
            team_id,
            name: input.name,
            description: input.description,
            feature_type,
            stages,
            dependencies: input.dependencies.into_iter().map(|id| Uuid::try_from(id).unwrap()).collect(),
        }
    }


    fn get_create_stages_to_create(
        stages: Vec<CreateFeatureStageInput>,
        relationships: Vec<CreateRelationshipInput>,
    ) -> Vec<CreateFeatureStage> {
        let mut stages = stages
            .into_iter()
            .map(|stage| CreateFeatureStage {
                id: stage
                    .id
                    .map_or_else(Uuid::new_v4, |id| Uuid::try_from(id).unwrap()),
                environment_id: Uuid::try_from(stage.environment_id.clone()).unwrap(),
                order_index: stage.order_index,
                position: stage.position,
                enabled: stage.enabled,
                bucketing_key: stage.bucketing_key.clone(),
                parent_stage: None,
            })
            .collect::<Vec<CreateFeatureStage>>();

        //#FIXME: this duplicate logic should be refactored, this is in both pipeline.rs and feature.rs
        let cloned_stages = stages.clone();
        let relationships_map = relationships
            .iter()
            .map(|relationship| {
                let stage = cloned_stages
                    .iter()
                    .find(|stage| stage.order_index == relationship.source_id);
                (
                    relationship.source_id,
                    relationship.target_id,
                    stage.unwrap(),
                ) // Stage should always be present
            })
            .collect::<Vec<(i32, i32, &CreateFeatureStage)>>();

        for (_, target_id, stage) in relationships_map {
            if let Some(target_stage) = stages.iter_mut().find(|s| s.order_index == target_id) {
                target_stage.parent_stage = Some(Box::new(stage.clone()));
            }
        }

        stages
    }

    fn map_to_update_feature(id: ID, input: UpdateFeatureInput) -> UpdateFeature {
        let id = Uuid::try_from(id).unwrap();
        let feature_type = Some(Self::map_graphql_to_entity_feature_type(input.feature_type));

        let stages = Self::get_create_stages_to_create(input.stages, input.relationships);
        UpdateFeature {
            id,
            name: Some(input.name),
            description: input.description,
            feature_type,
            stages,
            dependencies: vec![],
        }
    }

    fn map_entity_to_graphql_feature(feature: crate::database::entity::Feature) -> Feature {
        Feature {
            id: feature.id.into(),
            name: feature.name,
            description: feature.description,
            feature_type: Self::map_entity_to_graphql_feature_type(feature.feature_type),
            enabled: None, // This would need to be determined based on the feature's stages
            team_id: feature.team_id.into(),
            dependencies: feature
                .dependencies
                .into_iter()
                .map(|d| d.depends_on_id.into())
                .collect(),
            stages: vec![],
            relationships: vec![],
        }
    }
}

#[async_trait::async_trait]
impl FeatureLogic for FeatureLogicImpl {
    async fn get_feature_by_id(&self, id: ID) -> Result<Feature, Error> {
        let feature = self
            .repository
            .get_feature_by_id(Uuid::try_from(id).unwrap())
            .await?;
        let features = vec![feature.clone()]; // Wrap in a vector to reuse the same logic
        let stages = features
            .iter()
            .flat_map(|feature| &feature.stages)
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect::<Vec<Box<dyn DBStage>>>();


        let environment_map = get_environment_map(&*self.environment_logic, &stages, true).await?;
        let db_stages = feature
            .stages
            .iter()
            .map(|stage| Box::new(stage.clone()) as Box<dyn DBStage>)
            .collect();

        let stages = map_stages(true, &environment_map, &db_stages, stage_factory);
        let relationships = create_relationships(true, db_stages, relationship_factory);

        let mut feature = Self::map_entity_to_graphql_feature(feature);
        feature.stages = stages;
        feature.relationships = relationships;
        Ok(feature)
    }

    async fn get_features(
        &self,
        team_id: ID,
        name: Option<String>,
        feature_type: Option<GraphQLFeatureType>,
    ) -> Result<Vec<Feature>, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let entity_feature_type = feature_type.map(Self::map_graphql_to_entity_feature_type);
        let features = self
            .repository
            .get_features(team_id, name, entity_feature_type)
            .await?;

        Ok(features
            .into_iter()
            .map(Self::map_entity_to_graphql_feature)
            .collect())
    }

    async fn create_feature(&self, team_id: ID, input: CreateFeatureInput) -> Result<ID, Error> {
        let team_id = Uuid::try_from(team_id).unwrap();
        let input = Self::map_to_create_feature(team_id, input);
        let feature_id = self.repository.create_feature(input).await?;
        Ok(ID::from(feature_id.to_string()))
    }

    async fn update_feature(&self, id: ID, input: UpdateFeatureInput) -> Result<Feature, Error> {
        let input = Self::map_to_update_feature(id, input);
        let feature = self.repository.update_feature(input).await?;
        Ok(Self::map_entity_to_graphql_feature(feature))
    }

    async fn delete_feature(&self, id: ID) -> Result<(), Error> {
        self.repository
            .delete_feature(Uuid::try_from(id).unwrap())
            .await?;
        Ok(())
    }

    async fn get_stage_contexts(&self, stage_id: ID) -> Result<Vec<crate::graphql::schema::Context>, Error> {
        let stage_id = Uuid::try_from(stage_id).unwrap();
        let list = self.repository.get_stage_contexts(stage_id).await?;
        Ok(list.into_iter().map(map_db_ctx_to_gql).collect())
    }

    async fn set_stage_contexts(&self, stage_id: ID, context_ids: Vec<ID>) -> Result<Vec<crate::graphql::schema::Context>, Error> {
        let stage_id = Uuid::try_from(stage_id).unwrap();
        let context_ids: Vec<Uuid> = context_ids.into_iter().map(|id| Uuid::try_from(id).unwrap()).collect();
        let list = self.repository.set_stage_contexts(stage_id, context_ids).await?;
        Ok(list.into_iter().map(map_db_ctx_to_gql).collect())
    }

    async fn get_stage_criteria(&self, stage_id: ID) -> Result<Vec<crate::graphql::schema::StageCriterion>, Error> {
        let stage_id = Uuid::try_from(stage_id).unwrap();
        let list = self.repository.get_stage_criteria(stage_id).await?;
        Ok(list.into_iter().map(map_db_criterion_to_gql).collect())
    }

    async fn set_stage_criteria(&self, stage_id: ID, criteria: Vec<crate::graphql::schema::CreateStageCriterionInput>) -> Result<Vec<crate::graphql::schema::StageCriterion>, Error> {
        let stage_id = Uuid::try_from(stage_id).unwrap();
        let create: Vec<crate::database::feature::CreateStageCriterion> = criteria.into_iter().map(|c| crate::database::feature::CreateStageCriterion {
            context_key: c.context_key,
            context_id: Uuid::try_from(c.context_id).unwrap(),
            rollout_percentage: c.rollout_percentage,
        }).collect();
        let list = self.repository.set_stage_criteria(stage_id, create).await?;
        Ok(list.into_iter().map(map_db_criterion_to_gql).collect())
    }

    fn clone_box(&self) -> Box<dyn FeatureLogic> {
        Box::new(self.clone())
    }
}

fn map_db_ctx_to_gql(c: crate::database::entity::Context) -> crate::graphql::schema::Context {
    crate::graphql::schema::Context {
        id: ID::from(c.id),
        team_id: ID::from(c.team_id),
        key: c.key,
        entries: c.entries.into_iter().map(|e| crate::graphql::schema::ContextEntry { id: ID::from(e.id), value: e.value }).collect(),
    }
}

fn map_db_criterion_to_gql(sc: crate::database::entity::StageCriterion) -> crate::graphql::schema::StageCriterion {
    crate::graphql::schema::StageCriterion {
        id: ID::from(sc.id),
        stage_id: ID::from(sc.stage_id),
        context_key: sc.context_key,
        context: map_db_ctx_to_gql(sc.context),
        rollout_percentage: sc.rollout_percentage,
    }
}

fn relationship_factory(source_id: i32, target_id: i32) -> FeatureRelationship {
    FeatureRelationship {
        source_id,
        target_id,
    }
}

fn stage_factory(
    id: ID,
    environment: Environment,
    order_index: i32,
    position: String,
    enabled: bool,
) -> FeatureStage {
    FeatureStage {
        id,
        environment,
        order_index,
        position,
        enabled,
        bucketing_key: None,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::database::entity::Feature as EntityFeature;
    use crate::database::feature::MockFeatureRepository;
    use crate::logic::environment::MockEnvironmentLogic;

    #[test]
    fn test_get_create_stages_to_create() {
        let stages = create_dummy_stages();

        let relationships = vec![];

        let result = FeatureLogicImpl::get_create_stages_to_create(stages, relationships);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].order_index, 0);
        assert_eq!(result[1].order_index, 1);
    }

    #[test]
    fn test_get_create_stages_to_create_with_relationships() {
        let stages = create_dummy_stages();

        let relationships = vec![CreateRelationshipInput {
            source_id: 0,
            target_id: 1,
        }];

        let result = FeatureLogicImpl::get_create_stages_to_create(stages, relationships);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].order_index, 0);
        assert_eq!(result[1].order_index, 1);
        assert!(result[1].parent_stage.is_some());
    }

    fn create_dummy_stages() -> Vec<CreateFeatureStageInput> {
        let stages = vec![
            CreateFeatureStageInput {
                id: None,
                environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                order_index: 0,
                position: "top".to_string(),
                enabled: true,
                bucketing_key: None,
            },
            CreateFeatureStageInput {
                id: Some(ID::from("3eef17bc-9e06-411d-b5f4-7a786e68bb96")),
                environment_id: ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"),
                order_index: 1,
                position: "bottom".to_string(),
                enabled: true,
                bucketing_key: None,
            },
        ];
        stages
    }

    #[tokio::test]
    async fn test_get_feature_by_id() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_get_feature_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| {
                Ok(EntityFeature {
                    id: Uuid::parse_str(ID).unwrap(),
                    name: "Test Feature".to_string(),
                    description: Some("Test description".to_string()),
                    feature_type: EntityFeatureType::Simple,
                    team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                    created_at: chrono::Utc::now(),
                    stages: vec![],
                    dependencies: vec![],
                })
            });

        let logic = feature_logic(Box::new(repository), Box::new(environment_logic));
        let result = logic.get_feature_by_id(ID::from(ID)).await;

        assert!(result.is_ok());
        let feature = result.unwrap();
        assert_eq!(feature.id.to_string(), ID);
        assert_eq!(feature.name, "Test Feature");
        assert_eq!(feature.description, Some("Test description".to_string()));
        assert!(matches!(feature.feature_type, GraphQLFeatureType::Simple));
    }

    #[tokio::test]
    async fn test_get_non_existing_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98fca";
        repository
            .expect_get_feature_by_id()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Err(Error::NotFound(Uuid::parse_str(ID).unwrap())));

        let logic = feature_logic(Box::new(repository), Box::new(environment_logic));
        let result = logic.get_feature_by_id(ID::from(ID)).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert!(matches!(error, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn test_create_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        let input = CreateFeatureInput {
            name: "New Feature".to_string(),
            description: Some("New feature description".to_string()),
            feature_type: GraphQLFeatureType::Simple,
            enabled: Some(true),
            dependencies: vec![],
            relationships: vec![],
            stages: vec![],
        };

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        let id = Uuid::parse_str(ID).unwrap();
        repository
            .expect_create_feature()
            .withf(|input| input.name == "New Feature")
            .times(1)
            .returning(move |_| Ok(id));

        let logic = feature_logic(Box::new(repository), Box::new(environment_logic));
        let result = logic
            .create_feature(ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"), input)
            .await;

        assert!(result.is_ok());
        let feature_id = result.unwrap();
        assert_eq!(feature_id, ID::from(ID));
    }

    #[tokio::test]
    async fn test_update_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        const NAME: &str = "Updated Feature";

        let input = UpdateFeatureInput {
            name: NAME.to_string(),
            description: Some("Updated description".to_string()),
            feature_type: GraphQLFeatureType::Contextual,
            enabled: Some(true),
            dependencies: vec![],
            relationships: vec![],
            stages: vec![],
        };

        repository
            .expect_update_feature()
            .withf(|input| {
                input.id == Uuid::parse_str(ID).unwrap() && input.name == Some(NAME.to_string())
            })
            .times(1)
            .returning(move |_| {
                Ok(EntityFeature {
                    id: Uuid::parse_str(ID).unwrap(),
                    name: NAME.to_string(),
                    description: Some("Updated description".to_string()),
                    feature_type: EntityFeatureType::Contextual,
                    team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                    created_at: chrono::Utc::now(),
                    stages: vec![],
                    dependencies: vec![],
                })
            });

        let logic = feature_logic(Box::new(repository), Box::new(environment_logic));
        let result = logic.update_feature(ID::from(ID), input).await;

        assert!(result.is_ok());
        let feature = result.unwrap();
        assert_eq!(feature.name, NAME);
        assert_eq!(feature.description, Some("Updated description".to_string()));
        assert!(matches!(
            feature.feature_type,
            GraphQLFeatureType::Contextual
        ));
    }

    #[tokio::test]
    async fn test_delete_feature() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        const ID: &str = "3eef17bc-9e06-411d-b5f4-7a786e68bb96";
        repository
            .expect_delete_feature()
            .withf(|mock_id| mock_id.eq(&Uuid::parse_str(ID).unwrap()))
            .times(1)
            .returning(move |_| Ok(()));

        let logic = feature_logic(Box::new(repository), Box::new(environment_logic));
        let result = logic.delete_feature(ID::from(ID)).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_features() {
        let mut repository = MockFeatureRepository::new();
        let environment_logic = MockEnvironmentLogic::new();

        repository
            .expect_get_features()
            .withf(|_, name, feature_type| name.is_none() && feature_type.is_none())
            .times(1)
            .returning(move |_, _, _| {
                Ok(vec![
                    EntityFeature {
                        id: Uuid::new_v4(),
                        name: "Test Feature".to_string(),
                        description: Some("Test description".to_string()),
                        feature_type: EntityFeatureType::Simple,
                        team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                        created_at: chrono::Utc::now(),
                        stages: vec![],
                        dependencies: vec![],
                    },
                    EntityFeature {
                        id: Uuid::new_v4(),
                        name: "Another Feature".to_string(),
                        description: Some("Another description".to_string()),
                        feature_type: EntityFeatureType::Contextual,
                        team_id: Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap(),
                        created_at: chrono::Utc::now(),
                        stages: vec![],
                        dependencies: vec![],
                    },
                ])
            });

        let logic = feature_logic(Box::new(repository), Box::new(environment_logic));
        let result = logic
            .get_features(ID::from("51ecc366-f1cd-4d3d-ab73-fa60bad98f27"), None, None)
            .await;

        assert!(result.is_ok());
        let features = result.unwrap();
        assert_eq!(features.len(), 2);
    }
}
