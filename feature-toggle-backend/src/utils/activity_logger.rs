/// Activity logging helper module
///
/// This module provides convenient functions to log activities throughout the application.
/// It defines common activity types and provides a simple API for creating activity log entries.
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use sqlx::PgConnection;
use uuid::Uuid;

/// Common activity types in the system
pub mod activity_types {
    // Feature activities
    pub const FEATURE_CREATED: &str = "feature_created";
    pub const FEATURE_UPDATED: &str = "feature_updated";
    pub const FEATURE_DELETED: &str = "feature_deleted";
    pub const FEATURE_DEPLOYED: &str = "feature_deployed";
    pub const FEATURE_ENABLED: &str = "feature_enabled";
    pub const FEATURE_DISABLED: &str = "feature_disabled";
    pub const KILL_SWITCH_ACTIVATED: &str = "kill_switch_activated";
    pub const KILL_SWITCH_DEACTIVATED: &str = "kill_switch_deactivated";

    // User activities
    pub const USER_CREATED: &str = "user_created";
    pub const USER_UPDATED: &str = "user_updated";
    pub const USER_DELETED: &str = "user_deleted";
    pub const USER_LOGGED_IN: &str = "user_logged_in";
    pub const USER_LOGGED_OUT: &str = "user_logged_out";
    pub const USER_PASSWORD_CHANGED: &str = "user_password_changed";

    // Team activities
    pub const TEAM_CREATED: &str = "team_created";
    pub const TEAM_UPDATED: &str = "team_updated";
    pub const TEAM_DELETED: &str = "team_deleted";
    pub const USER_ADDED_TO_TEAM: &str = "user_added_to_team";
    pub const USER_REMOVED_FROM_TEAM: &str = "user_removed_from_team";

    // Client activities
    pub const CLIENT_CREATED: &str = "client_created";
    pub const CLIENT_UPDATED: &str = "client_updated";
    pub const CLIENT_DELETED: &str = "client_deleted";
    pub const CLIENT_ENABLED: &str = "client_enabled";
    pub const CLIENT_DISABLED: &str = "client_disabled";

    // Environment activities
    pub const ENVIRONMENT_CREATED: &str = "environment_created";
    pub const ENVIRONMENT_UPDATED: &str = "environment_updated";
    pub const ENVIRONMENT_DELETED: &str = "environment_deleted";

    // Pipeline activities
    pub const PIPELINE_CREATED: &str = "pipeline_created";
    pub const PIPELINE_UPDATED: &str = "pipeline_updated";
    pub const PIPELINE_DELETED: &str = "pipeline_deleted";
    pub const STAGE_APPROVED: &str = "stage_approved";
    pub const STAGE_REJECTED: &str = "stage_rejected";
    pub const STAGE_DEPLOYED: &str = "stage_deployed";
    pub const STAGE_ROLLBACKED: &str = "stage_rollbacked";

    // Role activities
    pub const ROLE_CREATED: &str = "role_created";
    pub const ROLE_DELETED: &str = "role_deleted";
    pub const ROLE_ASSIGNED: &str = "role_assigned";
    pub const ROLE_REVOKED: &str = "role_revoked";

    // Approval policy activities
    pub const APPROVAL_POLICY_CREATED: &str = "approval_policy_created";
    pub const APPROVAL_POLICY_UPDATED: &str = "approval_policy_updated";
    pub const APPROVAL_POLICY_DELETED: &str = "approval_policy_deleted";
}

/// Common entity types in the system
pub mod entity_types {
    pub const FEATURE: &str = "feature";
    pub const USER: &str = "user";
    pub const TEAM: &str = "team";
    pub const CLIENT: &str = "client";
    pub const ENVIRONMENT: &str = "environment";
    pub const PIPELINE: &str = "pipeline";
    pub const STAGE: &str = "stage";
    pub const ROLE: &str = "role";
}

/// Log a feature activity
pub async fn log_feature_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    feature_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::FEATURE.to_string(),
        entity_id: feature_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log a feature activity within an existing transaction
pub async fn log_feature_activity_tx(
    repo: &Box<dyn ActivityLogRepository>,
    conn: &mut PgConnection,
    activity_type: &str,
    feature_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::FEATURE.to_string(),
        entity_id: feature_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity_tx(conn, activity).await?;
    Ok(())
}

/// Log a user activity
pub async fn log_user_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    user_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::USER.to_string(),
        entity_id: user_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log a team activity
pub async fn log_team_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    team_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::TEAM.to_string(),
        entity_id: team_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log a team activity within an existing transaction
pub async fn log_team_activity_tx(
    repo: &Box<dyn ActivityLogRepository>,
    conn: &mut PgConnection,
    activity_type: &str,
    team_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::TEAM.to_string(),
        entity_id: team_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity_tx(conn, activity).await?;
    Ok(())
}

/// Log a client activity
pub async fn log_client_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    client_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::CLIENT.to_string(),
        entity_id: client_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log an environment-related activity
pub async fn log_environment_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    environment_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::ENVIRONMENT.to_string(),
        entity_id: environment_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log a pipeline-related activity
pub async fn log_pipeline_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    pipeline_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::PIPELINE.to_string(),
        entity_id: pipeline_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log a role-related activity
pub async fn log_role_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    user_id: &str, // For role assignments, user is the entity
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_types::USER.to_string(), // Roles are assigned to users
        entity_id: user_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

/// Log a generic activity (for any entity type)
pub async fn log_activity(
    repo: &Box<dyn ActivityLogRepository>,
    activity_type: &str,
    entity_type: &str,
    entity_id: &str,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    description: String,
    metadata: Option<serde_json::Value>,
) -> Result<(), sqlx::Error> {
    let activity = CreateActivityLog {
        activity_type: activity_type.to_string(),
        entity_type: entity_type.to_string(),
        entity_id: entity_id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata,
    };

    repo.create_activity(activity).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_type_constants() {
        assert_eq!(activity_types::FEATURE_CREATED, "feature_created");
        assert_eq!(activity_types::USER_LOGGED_IN, "user_logged_in");
        assert_eq!(
            activity_types::KILL_SWITCH_ACTIVATED,
            "kill_switch_activated"
        );
    }

    #[test]
    fn test_entity_type_constants() {
        assert_eq!(entity_types::FEATURE, "feature");
        assert_eq!(entity_types::USER, "user");
        assert_eq!(entity_types::TEAM, "team");
    }
}
