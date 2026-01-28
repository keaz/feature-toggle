//! Transactional logic operations for approval policy management.
//!
//! This module provides functions that execute approval policy operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::approval::{
    ApprovalRepositoryTx, CreateApprovalPolicyInput, UpdateApprovalPolicyInput,
};
use crate::database::entity::ApprovalPolicy;
use crate::logic::ActorContext;
use crate::utils::activity_logger::activity_types;
use sqlx::PgConnection;
use uuid::Uuid;

fn actor_details(actor: &Option<ActorContext>) -> (Option<Uuid>, Option<String>) {
    actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None))
}

/// Create an approval policy within a transaction.
///
/// This function performs policy creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_approval_policy_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    input: CreateApprovalPolicyInput,
    actor: Option<ActorContext>,
) -> Result<ApprovalPolicy, Error>
where
    R: ApprovalRepositoryTx,
{
    // Create policy within transaction
    let policy = repo.create_policy_tx(conn, input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor_details(&actor);

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::APPROVAL_POLICY_CREATED.to_string(),
        entity_type: "approval_policy".to_string(),
        entity_id: policy.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created approval policy '{}'", policy.name),
        metadata: Some(serde_json::json!({
            "policy_id": policy.id.to_string(),
            "policy_name": policy.name.clone(),
            "team_id": policy.team_id.to_string(),
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(policy)
}

/// Update an approval policy within a transaction.
///
/// This function performs policy update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_approval_policy_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    policy_id: Uuid,
    input: UpdateApprovalPolicyInput,
    actor: Option<ActorContext>,
) -> Result<ApprovalPolicy, Error>
where
    R: ApprovalRepositoryTx,
{
    // Update policy within transaction
    let policy = repo.update_policy_tx(conn, policy_id, input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor_details(&actor);

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::APPROVAL_POLICY_UPDATED.to_string(),
        entity_type: "approval_policy".to_string(),
        entity_id: policy.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated approval policy '{}'", policy.name),
        metadata: Some(serde_json::json!({
            "policy_id": policy.id.to_string(),
            "policy_name": policy.name.clone(),
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(policy)
}

/// Delete an approval policy within a transaction.
///
/// This function performs policy deletion and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn delete_approval_policy_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    policy_id: Uuid,
    policy_name: String,
    actor: Option<ActorContext>,
) -> Result<bool, Error>
where
    R: ApprovalRepositoryTx,
{
    // Delete policy within transaction
    let result = repo.delete_policy_tx(conn, policy_id).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor_details(&actor);

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::APPROVAL_POLICY_DELETED.to_string(),
        entity_type: "approval_policy".to_string(),
        entity_id: policy_id.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted approval policy '{}'", policy_name),
        metadata: Some(serde_json::json!({
            "policy_id": policy_id.to_string(),
            "policy_name": policy_name,
        })),
    };

    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(result)
}
