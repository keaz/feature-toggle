use async_graphql::{Context, Error, ID};

pub mod pipeline;
mod team;

pub trait InputValidator {
    async fn validate(&self, id: Option<ID>, team_id: Option<ID>, ctx: &Context<'_>) -> async_graphql::Result<(), Error>;
}