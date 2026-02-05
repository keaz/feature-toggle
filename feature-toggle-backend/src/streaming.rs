use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::model::ID;

/// Calculate from_time and to_time based on the given period.
pub fn calculate_time_range(period: TimePeriod, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
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
pub async fn activity_matches_team(
    activity: &crate::database::activity_log::ActivityLogRow,
    team_id: Uuid,
    feature_repo: &std::sync::Arc<Box<dyn crate::database::feature::FeatureRepository>>,
    environment_logic: &Box<dyn crate::logic::environment::EnvironmentLogic>,
    client_logic: &Box<dyn crate::logic::client::ClientLogic>,
    pipeline_logic: &Box<dyn crate::logic::pipeline::PipelineLogic>,
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
            if let Ok(fid) = Uuid::parse_str(&activity.entity_id) {
                if let Ok(f) = feature_repo.get_feature_by_id(fid).await {
                    return f.team_id == team_id;
                }
            }
            metadata_team_id.map(|id| id == team_id).unwrap_or(false)
        }
        "stage" => {
            if let Some(fid) = metadata_feature_id {
                if let Ok(f) = feature_repo.get_feature_by_id(fid).await {
                    if f.team_id == team_id {
                        return true;
                    }
                }
            }

            if let Ok(stage_uuid) = Uuid::parse_str(&activity.entity_id) {
                if let Ok(Some(fid)) = feature_repo.get_feature_id_by_stage_id(stage_uuid).await {
                    if let Ok(feature) = feature_repo.get_feature_by_id(fid).await {
                        if feature.team_id == team_id {
                            return true;
                        }
                    }
                }
            }

            if let Some(env_id) = metadata_environment_id {
                if let Ok(env) = environment_logic
                    .get_environment_by_id(ID::from(env_id))
                    .await
                {
                    if env.team_id == ID::from(team_id) {
                        return true;
                    }
                }
            }

            metadata_team_id.map(|id| id == team_id).unwrap_or(false)
        }
        "environment" => {
            if let Ok(eid) = Uuid::parse_str(&activity.entity_id) {
                if let Ok(env) = environment_logic.get_environment_by_id(ID::from(eid)).await {
                    return env.team_id == ID::from(team_id);
                }
            }
            metadata_team_id.map(|id| id == team_id).unwrap_or(false)
        }
        "client" => {
            if let Ok(cid) = Uuid::parse_str(&activity.entity_id) {
                if let Ok(c) = client_logic.get_client_by_id(ID::from(cid)).await {
                    return c.team_id == ID::from(team_id);
                }
            }
            metadata_team_id.map(|id| id == team_id).unwrap_or(false)
        }
        "pipeline" => {
            if let Ok(pid) = Uuid::parse_str(&activity.entity_id) {
                if let Ok(p) = pipeline_logic.get_pipeline_by_id(ID::from(pid)).await {
                    return p.team_id == ID::from(team_id);
                }
            }
            metadata_team_id.map(|id| id == team_id).unwrap_or(false)
        }
        _ => metadata_team_id.map(|id| id == team_id).unwrap_or(false),
    }
}
