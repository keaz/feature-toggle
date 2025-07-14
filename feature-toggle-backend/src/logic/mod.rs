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
                    relationships.push(
                        relationship_factory(stage_inner.order_index(), stage.order_index())
                    );
                });
        });

    relationships
}

fn map_stages<R: Stage + 'static>(
    has_stage: bool,
    environment_map: &HashMap<Uuid, Environment>,
    stages: &Vec<Box<dyn DBStage>>,
    stage_factory: impl Fn(ID, Environment, i32, String) -> R,
) -> Vec<R> {
    let mut mapped_stages: Vec<R> = vec![];
    if has_stage {
        stages
            .iter()
            .for_each(|stage| {
                let feature_stage = stage_factory(
                    stage.get_id().into(),
                    environment_map
                        .get(&stage.environment_id())
                        .unwrap()
                        .to_owned(),
                    stage.order_index(),
                    stage.position(),
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