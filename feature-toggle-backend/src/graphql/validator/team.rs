use crate::graphql::schema::CreateTeamInput;
use crate::graphql::validator::InputValidator;
use async_graphql::{Context, Error, ID};

impl InputValidator for CreateTeamInput {
    async fn validate(&self, id: Option<ID>, team_id: Option<ID>, ctx: &Context<'_>) -> async_graphql::Result<(), Error> {
        todo!()
    }
}