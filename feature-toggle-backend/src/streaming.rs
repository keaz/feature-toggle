use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

use crate::model::ID;

/// Calculate from_time and to_time based on the given period.
pub fn calculate_time_range(
    period: TimePeriod,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    match period {
        TimePeriod::H24 => (now - chrono::Duration::hours(24), now),
        TimePeriod::D7 => (now - chrono::Duration::days(7), now),
        TimePeriod::D30 => (now - chrono::Duration::days(30), now),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimePeriod {
    H24,
    D7,
    D30,
}

/// Determine whether an activity belongs to the specified team.
/// Uses best-effort lookups on associated entities and metadata.
#[derive(Default)]
pub struct ActivityTeamMatchCache {
    feature_team_ids: HashMap<Uuid, Option<Uuid>>,
    stage_feature_ids: HashMap<Uuid, Option<Uuid>>,
    environment_team_ids: HashMap<Uuid, Option<Uuid>>,
    client_team_ids: HashMap<Uuid, Option<Uuid>>,
    pipeline_team_ids: HashMap<Uuid, Option<Uuid>>,
}

impl ActivityTeamMatchCache {
    async fn feature_team_id(
        &mut self,
        feature_id: Uuid,
        feature_repo: &dyn crate::database::feature::FeatureRepository,
    ) -> Option<Uuid> {
        if let Some(cached) = self.feature_team_ids.get(&feature_id) {
            return *cached;
        }

        let resolved = feature_repo
            .get_feature_by_id(feature_id)
            .await
            .ok()
            .map(|feature| feature.team_id);
        self.feature_team_ids.insert(feature_id, resolved);
        resolved
    }

    async fn feature_id_for_stage(
        &mut self,
        stage_id: Uuid,
        feature_repo: &dyn crate::database::feature::FeatureRepository,
    ) -> Option<Uuid> {
        if let Some(cached) = self.stage_feature_ids.get(&stage_id) {
            return *cached;
        }

        let resolved = feature_repo
            .get_feature_id_by_stage_id(stage_id)
            .await
            .ok()
            .flatten();
        self.stage_feature_ids.insert(stage_id, resolved);
        resolved
    }

    async fn environment_team_id(
        &mut self,
        environment_id: Uuid,
        environment_logic: &dyn crate::logic::environment::EnvironmentLogic,
    ) -> Option<Uuid> {
        if let Some(cached) = self.environment_team_ids.get(&environment_id) {
            return *cached;
        }

        let resolved = environment_logic
            .get_environment_by_id(ID::from(environment_id))
            .await
            .ok()
            .and_then(|environment| environment.team_id.parse().ok());
        self.environment_team_ids.insert(environment_id, resolved);
        resolved
    }

    async fn client_team_id(
        &mut self,
        client_id: Uuid,
        client_logic: &dyn crate::logic::client::ClientLogic,
    ) -> Option<Uuid> {
        if let Some(cached) = self.client_team_ids.get(&client_id) {
            return *cached;
        }

        let resolved = client_logic
            .get_client_by_id(ID::from(client_id))
            .await
            .ok()
            .and_then(|client| client.team_id.parse().ok());
        self.client_team_ids.insert(client_id, resolved);
        resolved
    }

    async fn pipeline_team_id(
        &mut self,
        pipeline_id: Uuid,
        pipeline_logic: &dyn crate::logic::pipeline::PipelineLogic,
    ) -> Option<Uuid> {
        if let Some(cached) = self.pipeline_team_ids.get(&pipeline_id) {
            return *cached;
        }

        let resolved = pipeline_logic
            .get_pipeline_by_id(ID::from(pipeline_id))
            .await
            .ok()
            .and_then(|pipeline| pipeline.team_id.parse().ok());
        self.pipeline_team_ids.insert(pipeline_id, resolved);
        resolved
    }
}

pub async fn activity_matches_team_cached(
    activity: &crate::database::activity_log::ActivityLogRow,
    team_id: Uuid,
    feature_repo: &std::sync::Arc<Box<dyn crate::database::feature::FeatureRepository>>,
    environment_logic: &dyn crate::logic::environment::EnvironmentLogic,
    client_logic: &dyn crate::logic::client::ClientLogic,
    pipeline_logic: &dyn crate::logic::pipeline::PipelineLogic,
    cache: &mut ActivityTeamMatchCache,
) -> bool {
    let metadata_team_id = activity
        .metadata
        .as_ref()
        .and_then(|m| m.get("teamId").or_else(|| m.get("team_id")))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    if metadata_team_id == Some(team_id) {
        return true;
    }

    let feature_repo = feature_repo.as_ref().as_ref();

    let metadata_feature_id = activity
        .metadata
        .as_ref()
        .and_then(|m| m.get("feature_id").or_else(|| m.get("featureId")))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let metadata_environment_id = activity
        .metadata
        .as_ref()
        .and_then(|m| m.get("environment_id").or_else(|| m.get("environmentId")))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    match activity.entity_type.as_str() {
        "team" => activity.entity_id == team_id.to_string(),
        "feature" => {
            if let Ok(feature_id) = Uuid::parse_str(&activity.entity_id) {
                return cache
                    .feature_team_id(feature_id, feature_repo)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id);
            }
            metadata_team_id.is_some_and(|metadata_id| metadata_id == team_id)
        }
        "stage" => {
            if let Some(feature_id) = metadata_feature_id
                && cache
                    .feature_team_id(feature_id, feature_repo)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id)
            {
                return true;
            }

            if let Ok(stage_uuid) = Uuid::parse_str(&activity.entity_id)
                && let Some(feature_id) = cache.feature_id_for_stage(stage_uuid, feature_repo).await
                && cache
                    .feature_team_id(feature_id, feature_repo)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id)
            {
                return true;
            }

            if let Some(environment_id) = metadata_environment_id
                && cache
                    .environment_team_id(environment_id, environment_logic)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id)
            {
                return true;
            }

            metadata_team_id.is_some_and(|metadata_id| metadata_id == team_id)
        }
        "environment" => {
            if let Ok(environment_id) = Uuid::parse_str(&activity.entity_id) {
                return cache
                    .environment_team_id(environment_id, environment_logic)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id);
            }
            metadata_team_id.is_some_and(|metadata_id| metadata_id == team_id)
        }
        "client" => {
            if let Ok(client_id) = Uuid::parse_str(&activity.entity_id) {
                return cache
                    .client_team_id(client_id, client_logic)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id);
            }
            metadata_team_id.is_some_and(|metadata_id| metadata_id == team_id)
        }
        "pipeline" => {
            if let Ok(pipeline_id) = Uuid::parse_str(&activity.entity_id) {
                return cache
                    .pipeline_team_id(pipeline_id, pipeline_logic)
                    .await
                    .is_some_and(|resolved_team_id| resolved_team_id == team_id);
            }
            metadata_team_id.is_some_and(|metadata_id| metadata_id == team_id)
        }
        _ => metadata_team_id.is_some_and(|metadata_id| metadata_id == team_id),
    }
}

pub async fn activity_matches_team(
    activity: &crate::database::activity_log::ActivityLogRow,
    team_id: Uuid,
    feature_repo: &std::sync::Arc<Box<dyn crate::database::feature::FeatureRepository>>,
    environment_logic: &dyn crate::logic::environment::EnvironmentLogic,
    client_logic: &dyn crate::logic::client::ClientLogic,
    pipeline_logic: &dyn crate::logic::pipeline::PipelineLogic,
) -> bool {
    let mut cache = ActivityTeamMatchCache::default();
    activity_matches_team_cached(
        activity,
        team_id,
        feature_repo,
        environment_logic,
        client_logic,
        pipeline_logic,
        &mut cache,
    )
    .await
}
