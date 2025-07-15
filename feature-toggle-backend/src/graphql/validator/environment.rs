use crate::graphql::schema::{CreateEnvironmentInput, UpdateEnvironmentInput};
use crate::graphql::validator::{CreateInputValidator, UpdateInputValidator};
use crate::logic::environment::EnvironmentLogic;
use async_graphql::{Context, Error, ID, Result};

impl CreateInputValidator for CreateEnvironmentInput {
    async fn validate(&self, team_id: Option<ID>, ctx: &Context<'_>) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        let environments = logic
            .get_environments(team_id.unwrap(), Some(self.name.clone()), None)
            .await?;

        if !environments.is_empty() {
            return Err(Error::new(format!(
                "Environment with name '{:?}' already exists",
                self.name
            )));
        }

        Ok(())
    }
}

impl UpdateInputValidator for UpdateEnvironmentInput {
    async fn validate(&self, id: Option<ID>, ctx: &Context<'_>) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn EnvironmentLogic>>()?;
        let environment = logic.get_environment_by_id(id.clone().unwrap()).await?;
        let environments = logic
            .get_environments(environment.team_id, self.name.clone(), None)
            .await?;

        if !environments.is_empty() {
            return Err(Error::new(format!(
                "Pipeline with name '{:?}' already exists",
                self.name
            )));
        }

        Ok(())
    }
}
