//! Transactional logic operations for role management.
//!
//! This module provides functions that execute role operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::role::RoleRepositoryTx;
use crate::logic::ActorContext;
use crate::logic::role::{ApiRole, SYSTEM_ROLE_NAMES};
use crate::model::ID;
use crate::utils::activity_logger::activity_types;
use sqlx::PgConnection;
use uuid::Uuid;

fn actor_details(actor: &Option<ActorContext>) -> (Option<Uuid>, Option<String>) {
    actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None))
}

fn is_system_role(name: &str) -> bool {
    SYSTEM_ROLE_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(name))
}

/// Create a role within a transaction.
///
/// This function performs role creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_role_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    name: String,
    description: String,
    actor: Option<ActorContext>,
) -> Result<ApiRole, Error>
where
    R: RoleRepositoryTx,
{
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(Error::InvalidInput("Role name cannot be empty".to_string()));
    }
    if trimmed_name.len() > 50 {
        return Err(Error::InvalidInput(
            "Role name cannot exceed 50 characters".to_string(),
        ));
    }

    let trimmed_description = description.trim();
    if trimmed_description.is_empty() {
        return Err(Error::InvalidInput(
            "Role description cannot be empty".to_string(),
        ));
    }

    // Create role within transaction
    let role = repo
        .create_role_tx(conn, trimmed_name, trimmed_description)
        .await?;

    // Extract actor information
    let (actor_id, actor_name) = actor_details(&actor);

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::ROLE_CREATED.to_string(),
        entity_type: "role".to_string(),
        entity_id: role.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created role '{}'", role.name),
        metadata: Some(serde_json::json!({
            "role_id": role.id.to_string(),
            "role_name": role.name.clone(),
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(ApiRole::from(role))
}

/// Delete a role within a transaction.
///
/// This function performs role deletion and activity logging
/// within the provided database connection, ensuring atomicity.
/// System roles cannot be deleted.
pub async fn delete_role_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: RoleRepositoryTx,
{
    let role_id = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    // Get role to check if it's a system role (uses pool, not conn)
    let role = repo.get_role_by_id(role_id).await?;

    if is_system_role(&role.name) {
        return Err(Error::InvalidInput(
            "System roles cannot be deleted".to_string(),
        ));
    }

    // Delete role within transaction
    repo.delete_role_tx(conn, role_id).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor_details(&actor);

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::ROLE_DELETED.to_string(),
        entity_type: "role".to_string(),
        entity_id: role_id.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted role '{}'", role.name),
        metadata: Some(serde_json::json!({
            "role_id": role_id.to_string(),
            "role_name": role.name,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}

/// Assign roles to a user within a transaction.
///
/// This function performs role assignment and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn assign_user_roles_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    user_id: ID,
    role_ids: Vec<ID>,
    actor: Option<ActorContext>,
) -> Result<Vec<ApiRole>, Error>
where
    R: RoleRepositoryTx,
{
    let user_uuid =
        Uuid::try_from(user_id.clone()).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let role_uuids: Result<Vec<Uuid>, _> = role_ids
        .iter()
        .map(|id| Uuid::try_from(id.clone()))
        .collect();
    let role_uuids = role_uuids.map_err(|e| Error::InvalidInput(e.to_string()))?;

    // Extract actor information
    let (actor_id, actor_name) = actor_details(&actor);

    // Assign roles within transaction
    repo.assign_user_roles_tx(conn, user_uuid, role_uuids.clone(), actor_id)
        .await?;

    // Log activity for each role assignment within the same transaction
    for role_uuid in &role_uuids {
        // Get role name for better logging (uses pool, not conn)
        if let Ok(role) = repo.get_role_by_id(*role_uuid).await {
            let activity = CreateActivityLog {
                activity_type: activity_types::ROLE_ASSIGNED.to_string(),
                entity_type: "user".to_string(),
                entity_id: user_uuid.to_string(),
                actor_id,
                actor_name: actor_name.clone(),
                description: format!("Assigned role '{}' to user", role.name),
                metadata: Some(serde_json::json!({
                    "user_id": user_uuid.to_string(),
                    "role_id": role_uuid.to_string(),
                    "role_name": role.name,
                })),
            };

            activity_repo
                .create_activity_tx(conn, activity)
                .await
                .map_err(Error::DatabaseError)?;
        }
    }

    // Get updated roles for the user within the transaction
    let roles = repo.get_user_roles_tx(conn, user_uuid).await?;
    Ok(roles.into_iter().map(ApiRole::from).collect())
}
