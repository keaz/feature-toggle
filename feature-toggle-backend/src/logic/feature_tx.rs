//! Transactional logic operations for feature management.
//!
//! This module provides functions that execute feature operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::feature::{CreateFeature, FeatureRepositoryTx, UpdateFeature};
use crate::logic::ActorContext;
use crate::utils::activity_logger::activity_types;
use async_graphql::ID;
use sqlx::PgConnection;
use uuid::Uuid;

/// Create a feature within a transaction.
///
/// This function performs both the feature creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_feature_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    input: CreateFeature,
    actor: Option<ActorContext>,
) -> Result<Uuid, Error>
where
    R: FeatureRepositoryTx,
{
    let feature_key = input.key.clone();
    let team_id = input.team_id;

    if feature_key.is_empty() {
        return Err(Error::InvalidInput(
            "Feature key cannot be empty".to_string(),
        ));
    }

    // Create feature within transaction
    let feature_id = repo.create_feature_tx(conn, input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::FEATURE_CREATED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature_id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created feature '{}'", feature_key),
        metadata: Some(serde_json::json!({
            "feature_id": feature_id.to_string(),
            "feature_key": feature_key,
            "team_id": team_id.to_string(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(feature_id)
}

/// Update a feature within a transaction.
///
/// This function performs both the feature update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_feature_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    input: UpdateFeature,
    actor: Option<ActorContext>,
) -> Result<crate::database::entity::Feature, Error>
where
    R: FeatureRepositoryTx,
{
    let feature_id = input.id;

    // Update feature within transaction
    let feature = repo.update_feature_tx(conn, input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::FEATURE_UPDATED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature_id.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated feature '{}'", feature.key),
        metadata: Some(serde_json::json!({
            "feature_id": feature_id.to_string(),
            "feature_key": feature.key.clone(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(feature)
}

/// Delete a feature within a transaction.
///
/// This function performs both the feature deletion and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn delete_feature_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    feature_key: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: FeatureRepositoryTx,
{
    let feature_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    // Delete feature within transaction
    repo.delete_feature_tx(conn, feature_uuid).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::FEATURE_DELETED.to_string(),
        entity_type: "feature".to_string(),
        entity_id: feature_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted feature '{}'", feature_key),
        metadata: Some(serde_json::json!({
            "feature_id": feature_uuid.to_string(),
            "feature_key": feature_key,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}
