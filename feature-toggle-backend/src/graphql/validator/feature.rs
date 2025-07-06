use crate::graphql::schema::{CreateFeatureInput, FeatureType, UpdateFeatureInput};
use crate::graphql::validator::{validate_duplicate_environment_and_index, validate_relationships_and_stages, CreateInputValidator, UpdateInputValidator};
use crate::logic::feature::FeatureLogic;
use crate::logic::pipeline::PipelineLogic;
use async_graphql::{Context, Error, Result, ID};
use std::collections::HashSet;

impl CreateInputValidator for CreateFeatureInput {
    async fn validate(
        &self,
        team_id: Option<ID>,
        ctx: &Context<'_>,
    ) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn PipelineLogic>>()?;
        let pipelines = logic
            .get_pipelines(
                team_id.unwrap(),
                Some(self.name.clone()),
                Some(true),
                vec![],
            )
            .await?;
        if !pipelines.is_empty() {
            return Err(Error::new(format!(
                "Feature with name '{}' already exists",
                self.name
            )));
        }

        validate_relationships_and_stages(&self.stages, &self.relationships)?;
        validate_duplicate_environment_and_index(&self.stages)?;

        if self.feature_type == FeatureType::Contextual {
            if self.context.is_none() || self.context.as_ref().unwrap().is_empty() {
                return Err(Error::new("Contextual features must have at least one context defined"));
            }

            for context in self.context.as_ref().unwrap() {
                let set: HashSet<&String> = context.entries.iter().collect();
                if set.len() != context.entries.len() {
                    return Err(Error::new(format!(
                        "Contextual feature '{}' has duplicate entries in context '{}'",
                        self.name, context.key
                    )));
                }
            }
        }

        Ok(())
    }
}

impl UpdateInputValidator for UpdateFeatureInput {
    async fn validate(
        &self,
        id: Option<ID>,
        ctx: &Context<'_>,
    ) -> Result<(), Error> {
        let logic = ctx.data::<Box<dyn FeatureLogic>>()?;
        let feature = logic.get_feature_by_id(id.clone().unwrap()).await?;
        let pipelines = logic
            .get_features(feature.team_id, Some(self.name.clone()), None)
            .await?;

        if !pipelines.is_empty() && pipelines.iter().any(|p| p.id != id.clone().unwrap()) {
            return Err(Error::new(format!(
                "Feature with name '{:?}' already exists",
                self.name
            )));
        }

        validate_relationships_and_stages(&self.stages, &self.relationships)?;
        validate_duplicate_environment_and_index(&self.stages)?;

        Ok(())
    }
}
