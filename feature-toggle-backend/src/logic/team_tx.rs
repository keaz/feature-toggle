//! Transactional logic operations for team management.
//!
//! This module provides functions that execute team operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::team::{CreateTeam, TeamRepositoryTx, UpdateTeam};
use crate::model::{CreateTeamInput, Team, UpdateTeamInput};
use crate::logic::ActorContext;
use crate::utils::activity_logger::activity_types;
use crate::model::ID;
use sqlx::PgConnection;
use uuid::Uuid;

/// Create a team within a transaction.
///
/// This function performs both the team creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_team_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    input: CreateTeamInput,
    actor: Option<ActorContext>,
) -> Result<Team, Error>
where
    R: TeamRepositoryTx,
{
    let db_input = CreateTeam {
        name: input.name.clone(),
        description: input.description,
    };

    if db_input.name.is_empty() {
        return Err(Error::InvalidInput("Team name cannot be empty".to_string()));
    }

    // Create team within transaction
    let team = repo.create_team_tx(conn, db_input).await?;
    let id = ID::from(team.id);

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::TEAM_CREATED.to_string(),
        entity_type: "team".to_string(),
        entity_id: team.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created team '{}'", team.name),
        metadata: Some(serde_json::json!({
            "team_id": team.id.to_string(),
            "team_name": team.name.clone(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(Team {
        id,
        name: team.name,
        description: team.description,
    })
}

/// Update a team within a transaction.
///
/// This function performs both the team update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_team_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    input: UpdateTeamInput,
    actor: Option<ActorContext>,
) -> Result<Team, Error>
where
    R: TeamRepositoryTx,
{
    let db_input = UpdateTeam {
        id: Uuid::try_from(id).unwrap(),
        name: input.name.clone(),
        description: input.description.clone(),
    };

    // Update team within transaction
    let team = repo.update_team_tx(conn, db_input).await?;
    let result_id = ID::from(team.id);

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Build description
    let mut changes = Vec::new();
    if input.name.is_some() {
        changes.push(format!("name to '{}'", input.name.as_ref().unwrap()));
    }
    if input.description.is_some() {
        changes.push("description".to_string());
    }
    let description = if changes.is_empty() {
        format!("Updated team '{}'", team.name)
    } else {
        format!(
            "Updated team '{}': changed {}",
            team.name,
            changes.join(", ")
        )
    };

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::TEAM_UPDATED.to_string(),
        entity_type: "team".to_string(),
        entity_id: team.id.to_string(),
        actor_id,
        actor_name,
        description,
        metadata: Some(serde_json::json!({
            "team_id": team.id.to_string(),
            "team_name": team.name.clone(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(Team {
        id: result_id,
        name: team.name,
        description: team.description,
    })
}
