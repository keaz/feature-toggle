//! Transactional logic operations for user management.
//!
//! This module provides functions that execute user operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::user::{CreateUser, UpdateUser, UserRepositoryTx};
use crate::logic::ActorContext;
use crate::logic::user::{GqlUser, RegisterUserInput, UpdateGqlUserInput};
use crate::utils::activity_logger::activity_types;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use crate::model::ID;
use sqlx::PgConnection;
use uuid::Uuid;

/// Register a new user within a transaction.
///
/// This function performs user creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn register_user_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    input: RegisterUserInput,
    actor: Option<ActorContext>,
) -> Result<GqlUser, Error>
where
    R: UserRepositoryTx,
{
    if input.username.is_empty() || input.password.is_empty() {
        return Err(Error::InvalidInput(
            "Username and password are required".to_string(),
        ));
    }

    // Check if username already exists
    if repo.user_exists_by_username(&input.username).await? {
        return Err(Error::RecordAlreadyExists("username".to_string()));
    }

    // Check if email already exists
    if repo.user_exists_by_email(&input.email, None).await? {
        return Err(Error::RecordAlreadyExists("email".to_string()));
    }

    // Hash the password
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(input.password.as_bytes(), &salt)
        .map_err(|_| Error::InvalidInput("Failed to hash password".to_string()))?
        .to_string();

    // Create user within transaction
    let created = repo
        .create_user_tx(
            conn,
            CreateUser {
                username: input.username.clone(),
                password_hash,
                first_name: input.first_name.clone(),
                last_name: input.last_name.clone(),
                email: input.email,
                is_admin: input.is_admin,
                is_temporary_password: input.is_temporary_password,
            },
        )
        .await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::USER_CREATED.to_string(),
        entity_type: "user".to_string(),
        entity_id: created.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created user '{}'", created.username),
        metadata: Some(serde_json::json!({
            "user_id": created.id.to_string(),
            "username": created.username.clone(),
            "is_admin": created.is_admin,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(GqlUser {
        id: ID::from(created.id),
        username: created.username,
        first_name: created.first_name,
        last_name: created.last_name,
        email: created.email,
        is_admin: created.is_admin,
        created_at: created.created_at,
        updated_at: created.updated_at,
        last_login: created.last_login,
        is_temporary_password: created.is_temporary_password,
    })
}

/// Update an existing user within a transaction.
///
/// This function performs user update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_user_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    input: UpdateGqlUserInput,
    actor: Option<ActorContext>,
) -> Result<GqlUser, Error>
where
    R: UserRepositoryTx,
{
    let user_id = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    // If updating email, validate uniqueness (allow unchanged or same owner)
    if let Some(ref new_email) = input.email {
        if repo.user_exists_by_email(new_email, Some(user_id)).await? {
            return Err(Error::RecordAlreadyExists("email".to_string()));
        }
    }

    // Update user within transaction
    let updated = repo
        .update_user_tx(
            conn,
            UpdateUser {
                id: user_id,
                first_name: input.first_name,
                last_name: input.last_name,
                email: input.email,
                is_admin: input.is_admin,
                enabled: input.enabled,
            },
        )
        .await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::USER_UPDATED.to_string(),
        entity_type: "user".to_string(),
        entity_id: updated.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated user '{}'", updated.username),
        metadata: Some(serde_json::json!({
            "user_id": updated.id.to_string(),
            "username": updated.username.clone(),
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(GqlUser {
        id: ID::from(updated.id),
        username: updated.username,
        first_name: updated.first_name,
        last_name: updated.last_name,
        email: updated.email,
        is_admin: updated.is_admin,
        created_at: updated.created_at,
        updated_at: updated.updated_at,
        last_login: updated.last_login,
        is_temporary_password: updated.is_temporary_password,
    })
}

/// Assign user to teams within a transaction.
///
/// This function updates the user-team assignments and logs activity entries
/// within the same transaction, ensuring atomicity.
pub async fn assign_user_teams_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    team_ids: Vec<ID>,
    actor: Option<ActorContext>,
) -> Result<bool, Error>
where
    R: UserRepositoryTx,
{
    let user_id =
        Uuid::try_from(id).map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;
    let team_ids_uuid: Result<Vec<Uuid>, _> = team_ids
        .iter()
        .map(|id| Uuid::try_from(id.clone()))
        .collect();
    let team_ids_uuid =
        team_ids_uuid.map_err(|e| Error::InvalidInput(format!("Invalid team id: {e}")))?;

    repo.set_user_teams_tx(conn, user_id, team_ids_uuid.clone())
        .await?;

    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    for team_id in &team_ids_uuid {
        let activity = CreateActivityLog {
            activity_type: activity_types::USER_ADDED_TO_TEAM.to_string(),
            entity_type: "team".to_string(),
            entity_id: team_id.to_string(),
            actor_id,
            actor_name: actor_name.clone(),
            description: format!("User '{}' added to team", user_id),
            metadata: Some(serde_json::json!({
                "user_id": user_id.to_string(),
                "team_id": team_id.to_string(),
            })),
        };

        activity_repo
            .create_activity_tx(conn, activity)
            .await
            .map_err(Error::DatabaseError)?;
    }

    Ok(true)
}

/// Reset a user's password within a transaction.
///
/// This function verifies the current password, updates to a new one,
/// and logs the activity within the provided transaction.
pub async fn reset_password_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    current_password: String,
    new_password: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: UserRepositoryTx,
{
    let user_id = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let user = repo.get_user_by_id_tx(conn, user_id).await?;

    let parsed_hash = PasswordHash::new(&user.password_hash)
        .map_err(|_| Error::InvalidInput("Stored password hash is invalid".to_string()))?;
    Argon2::default()
        .verify_password(current_password.as_bytes(), &parsed_hash)
        .map_err(|_| Error::InvalidInput("Current password is incorrect".to_string()))?;

    if Argon2::default()
        .verify_password(new_password.as_bytes(), &parsed_hash)
        .is_ok()
    {
        return Err(Error::InvalidInput(
            "New password must be different from current password".to_string(),
        ));
    }

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let new_password_hash = argon2
        .hash_password(new_password.as_bytes(), &salt)
        .map_err(|_| Error::InvalidInput("Failed to hash new password".to_string()))?
        .to_string();

    repo.update_password_tx(conn, user_id, new_password_hash, false)
        .await?;

    // For reset_password, the actor is the user themselves (self-service password change).
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((Some(user_id), Some(user.username.clone())));

    let activity = CreateActivityLog {
        activity_type: activity_types::USER_PASSWORD_CHANGED.to_string(),
        entity_type: "user".to_string(),
        entity_id: user_id.to_string(),
        actor_id,
        actor_name,
        description: format!("User '{}' changed their password", user.username),
        metadata: Some(serde_json::json!({
            "user_id": user_id.to_string(),
            "username": user.username,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}

/// Set a temporary password within a transaction.
///
/// This function updates the user's password to a temporary one and
/// logs the activity within the provided transaction.
pub async fn set_temporary_password_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    user_id: ID,
    temporary_password: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: UserRepositoryTx,
{
    let user_uuid = Uuid::try_from(user_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let user = repo.get_user_by_id_tx(conn, user_uuid).await?;

    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(temporary_password.as_bytes(), &salt)
        .map_err(|_| Error::InvalidInput("Failed to hash temporary password".to_string()))?
        .to_string();

    repo.update_password_tx(conn, user_uuid, password_hash, true)
        .await?;

    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    let activity = CreateActivityLog {
        activity_type: activity_types::USER_PASSWORD_CHANGED.to_string(),
        entity_type: "user".to_string(),
        entity_id: user_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Temporary password set for user '{}'", user.username),
        metadata: Some(serde_json::json!({
            "user_id": user_uuid.to_string(),
            "username": user.username,
            "temporary": true,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}
