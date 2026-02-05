//! Transactional logic operations for client management.
//!
//! This module provides functions that execute client operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::client::{ClientRepositoryTx, CreateClient, UpdateClient};
use crate::database::entity::ClientType as EntityClientType;
use crate::model::{ClientType as GqlClientType, CreateClientInput, UpdateClientInput};
use crate::logic::ActorContext;
use crate::utils::activity_logger::activity_types;
use crate::model::ID;
use sqlx::PgConnection;
use uuid::Uuid;

/// Convert GraphQL ClientType to entity ClientType
fn to_entity_client_type(gql_type: GqlClientType) -> EntityClientType {
    match gql_type {
        GqlClientType::Web => EntityClientType::Web,
        GqlClientType::Backend => EntityClientType::Backend,
    }
}

/// Convert entity ClientType to GraphQL ClientType
fn to_gql_client_type(entity_type: EntityClientType) -> GqlClientType {
    match entity_type {
        EntityClientType::Web => GqlClientType::Web,
        EntityClientType::Backend => GqlClientType::Backend,
    }
}

/// Create a client within a transaction.
///
/// This function performs both the client creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_client_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    team_id: ID,
    input: CreateClientInput,
    actor: Option<ActorContext>,
) -> Result<crate::model::Client, Error>
where
    R: ClientRepositoryTx,
{
    let team_uuid = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let client_name = input.name.clone();

    let db_input = CreateClient {
        name: input.name,
        description: input.description,
        enabled: input.enabled.unwrap_or(true),
        client_type: to_entity_client_type(input.client_type),
        web_origins: input.web_origins,
    };

    if db_input.name.is_empty() {
        return Err(Error::InvalidInput(
            "Client name cannot be empty".to_string(),
        ));
    }

    // Create client within transaction
    let client = repo.create_client_tx(conn, team_uuid, db_input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::CLIENT_CREATED.to_string(),
        entity_type: "client".to_string(),
        entity_id: client.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created client '{}'", client_name),
        metadata: Some(serde_json::json!({
            "client_id": client.id.to_string(),
            "client_name": client_name,
            "team_id": team_uuid.to_string(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    // Convert entity::Client to schema::Client
    Ok(crate::model::Client {
        id: ID::from(client.id),
        team_id: ID::from(client.team_id),
        name: client.name,
        description: client.description,
        enabled: client.enabled,
        client_type: to_gql_client_type(client.client_type),
        api_key: client.api_key,
        web_origins: client.web_origins.unwrap_or_default(),
    })
}

/// Update a client within a transaction.
///
/// This function performs both the client update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_client_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    input: UpdateClientInput,
    actor: Option<ActorContext>,
) -> Result<crate::model::Client, Error>
where
    R: ClientRepositoryTx,
{
    let client_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let db_input = UpdateClient {
        name: input.name,
        description: input.description,
        enabled: input.enabled,
        client_type: input.client_type.map(to_entity_client_type),
        web_origins: input.web_origins,
    };

    // Update client within transaction
    let client = repo.update_client_tx(conn, client_uuid, db_input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::CLIENT_UPDATED.to_string(),
        entity_type: "client".to_string(),
        entity_id: client.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated client '{}'", client.name),
        metadata: Some(serde_json::json!({
            "client_id": client.id.to_string(),
            "client_name": client.name.clone(),
            "enabled": client.enabled,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    // Convert entity::Client to schema::Client
    Ok(crate::model::Client {
        id: ID::from(client.id),
        team_id: ID::from(client.team_id),
        name: client.name,
        description: client.description,
        enabled: client.enabled,
        client_type: to_gql_client_type(client.client_type),
        api_key: client.api_key,
        web_origins: client.web_origins.unwrap_or_default(),
    })
}

/// Delete a client within a transaction.
///
/// This function performs both the client deletion and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn delete_client_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    client_name: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: ClientRepositoryTx,
{
    let client_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    // Delete client within transaction
    repo.delete_client_tx(conn, client_uuid).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::CLIENT_DELETED.to_string(),
        entity_type: "client".to_string(),
        entity_id: client_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted client '{}'", client_name),
        metadata: Some(serde_json::json!({
            "client_id": client_uuid.to_string(),
            "client_name": client_name,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}
