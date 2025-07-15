use crate::graphql::schema::{CreateRelationshipInput, StageInput};
use async_graphql::{Context, Error, ID};
use std::collections::HashMap;

pub mod environment;
pub mod feature;
pub mod pipeline;
pub mod team;

pub trait CreateInputValidator {
    async fn validate(
        &self,
        team_id: Option<ID>,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<(), Error>;
}

pub trait UpdateInputValidator {
    async fn validate(&self, id: Option<ID>, ctx: &Context<'_>)
    -> async_graphql::Result<(), Error>;
}

pub fn validate_relationships_and_stages<T: StageInput + 'static>(
    stages: &[T],
    relationships: &[CreateRelationshipInput],
) -> async_graphql::Result<(), Error> {
    if stages.is_empty() {
        return Err(Error::new("Pipeline must have at least one stage"));
    }

    if relationships.len() != stages.len() - 1 {
        return Err(Error::new(format!(
            "Pipeline must have at least {} relationships",
            stages.len() - 1
        )));
    }

    Ok(())
}

pub fn validate_duplicate_environment_and_index<T: StageInput + 'static>(
    stages: &[T],
) -> async_graphql::Result<(), Error> {
    let mut env_map: HashMap<&ID, usize> = HashMap::new();
    let mut order_map: HashMap<i32, usize> = HashMap::new();

    for (idx, stage) in stages.iter().enumerate() {
        if env_map.contains_key(stage.environment_id()) {
            return Err(Error::new(format!(
                "Stages should not have the same environment_id: '{:?}'",
                stage.environment_id()
            )));
        }

        if order_map.contains_key(&stage.order_index()) {
            return Err(Error::new(format!(
                "Stages should not have the same order_index: '{}'",
                stage.order_index()
            )));
        }

        env_map.insert(stage.environment_id(), idx);
        order_map.insert(stage.order_index(), idx);
    }
    Ok(())
}
