use crate::graphql::schema::{CreateTeamInput, UpdateTeamInput};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use async_graphql::{Context, Error, ID};

impl CreateInputValidator for CreateTeamInput {
    async fn validate(
        &self,
        _team_id: Option<ID>,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<(), Error> {
        let logic = ctx.data::<Box<dyn crate::logic::team::TeamLogic>>()?;
        let teams = logic.get_teams(Some(self.name.clone())).await?;
        if !teams.is_empty() {
            return Err(Error::new(format!(
                "Team with name '{}' already exists",
                self.name
            )));
        }

        Ok(())
    }
}

impl UpdateInputValidator for UpdateTeamInput {
    async fn validate(
        &self,
        id: Option<ID>,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<(), Error> {
        let logic = ctx.data::<Box<dyn crate::logic::team::TeamLogic>>()?;
        if let Some(name) = &self.name {
            let teams = logic.get_teams(Some(name.clone())).await?;
            if !teams.is_empty() && teams.iter().any(|t| t.id != id.clone().unwrap()) {
                return Err(Error::new(format!(
                    "Team with name '{name:?}' already exists"
                )));
            }
        }
        Ok(())
    }
}
