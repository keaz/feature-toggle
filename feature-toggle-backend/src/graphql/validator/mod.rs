use async_graphql::{Context, Error, ID};

pub mod pipeline;
pub mod team;
pub mod environment;

pub trait CreateInputValidator {
    async fn validate(
        &self,
        team_id: Option<ID>,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<(), Error>;
}

pub trait UpdateInputValidator {
    async fn validate(
        &self,
        id: Option<ID>,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<(), Error>;
}

