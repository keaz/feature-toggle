//! Transactional logic operations for environment management.
//!
//! This module provides functions that execute environment operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::environment::{CreateEnvironment, EnvironmentRepositoryTx, UpdateEnvironment};
use crate::logic::ActorContext;
use crate::model::ID;
use crate::model::{CreateEnvironmentInput, UpdateEnvironmentInput};
use crate::utils::activity_logger::activity_types;
use sqlx::PgConnection;
use uuid::Uuid;

/// Create an environment within a transaction.
///
/// This function performs both the environment creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_environment_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    team_id: ID,
    input: CreateEnvironmentInput,
    actor: Option<ActorContext>,
) -> Result<crate::model::Environment, Error>
where
    R: EnvironmentRepositoryTx,
{
    let team_uuid = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let environment_name = input.name.clone();

    let db_input = CreateEnvironment {
        name: input.name,
        active: input.active,
        environment_type: input.environment_type,
    };

    if db_input.name.is_empty() {
        return Err(Error::InvalidInput(
            "Environment name cannot be empty".to_string(),
        ));
    }

    // Create environment within transaction
    let environment = repo
        .create_environment_tx(conn, team_uuid, db_input)
        .await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::ENVIRONMENT_CREATED.to_string(),
        entity_type: "environment".to_string(),
        entity_id: environment.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created environment '{}'", environment_name),
        metadata: Some(serde_json::json!({
            "environment_id": environment.id.to_string(),
            "environment_name": environment_name,
            "team_id": team_uuid.to_string(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(crate::model::Environment {
        id: ID::from(environment.id),
        name: environment.name,
        active: environment.active,
        team_id: ID::from(environment.team_id),
        environment_type: environment.environment_type,
    })
}

/// Update an environment within a transaction.
///
/// This function performs both the environment update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_environment_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    input: UpdateEnvironmentInput,
    actor: Option<ActorContext>,
) -> Result<crate::model::Environment, Error>
where
    R: EnvironmentRepositoryTx,
{
    let env_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let db_input = UpdateEnvironment {
        name: input.name,
        active: input.active,
        environment_type: input.environment_type,
    };

    // Update environment within transaction
    let environment = repo.update_environment_tx(conn, env_uuid, db_input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::ENVIRONMENT_UPDATED.to_string(),
        entity_type: "environment".to_string(),
        entity_id: environment.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated environment '{}'", environment.name),
        metadata: Some(serde_json::json!({
            "environment_id": environment.id.to_string(),
            "environment_name": environment.name.clone(),
            "active": environment.active,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(crate::model::Environment {
        id: ID::from(environment.id),
        name: environment.name,
        active: environment.active,
        team_id: ID::from(environment.team_id),
        environment_type: environment.environment_type,
    })
}

/// Delete an environment within a transaction.
///
/// This function performs both the environment deletion and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn delete_environment_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    environment_name: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: EnvironmentRepositoryTx,
{
    let env_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    // Delete environment within transaction
    repo.delete_environment_tx(conn, env_uuid).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::ENVIRONMENT_DELETED.to_string(),
        entity_type: "environment".to_string(),
        entity_id: env_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted environment '{}'", environment_name),
        metadata: Some(serde_json::json!({
            "environment_id": env_uuid.to_string(),
            "environment_name": environment_name,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}
