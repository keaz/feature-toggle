//! Transactional logic operations for pipeline management.
//!
//! This module provides functions that execute pipeline operations within
//! a shared database transaction, ensuring atomicity across repository
//! and activity logging operations.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::pipeline::{
    CreatePipeline, CreateStage, PipelineRepositoryTx, UpdatePipeline,
};
use crate::graphql::schema::{
    CreatePipelineInput, CreateRelationshipInput, CreateStageInput, UpdatePipelineInput,
};
use crate::logic::ActorContext;
use crate::logic::stage_builder::{build_stage_relationships, id_to_uuid};
use crate::utils::activity_logger::activity_types;
use async_graphql::ID;
use sqlx::PgConnection;
use uuid::Uuid;

/// Create a pipeline within a transaction.
///
/// This function performs both the pipeline creation and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn create_pipeline_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    team_id: ID,
    input: CreatePipelineInput,
    actor: Option<ActorContext>,
) -> Result<ID, Error>
where
    R: PipelineRepositoryTx,
{
    let team_uuid = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;
    let pipeline_name = input.name.clone();

    let db_input = map_to_create_pipeline(team_uuid, input);

    if db_input.name.is_empty() {
        return Err(Error::InvalidInput(
            "Pipeline name cannot be empty".to_string(),
        ));
    }

    // Create pipeline within transaction
    let pipeline_id = repo.create_pipeline_tx(conn, db_input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::PIPELINE_CREATED.to_string(),
        entity_type: "pipeline".to_string(),
        entity_id: pipeline_id.to_string(),
        actor_id,
        actor_name,
        description: format!("Created pipeline '{}'", pipeline_name),
        metadata: Some(serde_json::json!({
            "pipeline_id": pipeline_id.to_string(),
            "pipeline_name": pipeline_name,
            "team_id": team_uuid.to_string(),
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(ID::from(pipeline_id.to_string()))
}

/// Update a pipeline within a transaction.
///
/// This function performs both the pipeline update and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn update_pipeline_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    input: UpdatePipelineInput,
    actor: Option<ActorContext>,
) -> Result<crate::database::entity::Pipeline, Error>
where
    R: PipelineRepositoryTx,
{
    let db_input = map_to_update_pipeline(id, input);

    // Update pipeline within transaction
    let pipeline = repo.update_pipeline_tx(conn, db_input).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::PIPELINE_UPDATED.to_string(),
        entity_type: "pipeline".to_string(),
        entity_id: pipeline.id.to_string(),
        actor_id,
        actor_name,
        description: format!("Updated pipeline '{}'", pipeline.name),
        metadata: Some(serde_json::json!({
            "pipeline_id": pipeline.id.to_string(),
            "pipeline_name": pipeline.name.clone(),
            "active": pipeline.active,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(pipeline)
}

/// Delete a pipeline within a transaction.
///
/// This function performs both the pipeline deletion and activity logging
/// within the provided database connection, ensuring atomicity.
pub async fn delete_pipeline_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: ID,
    pipeline_name: String,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: PipelineRepositoryTx,
{
    let pipeline_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    // Delete pipeline within transaction
    repo.delete_pipeline_tx(conn, pipeline_uuid).await?;

    // Extract actor information
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None));

    // Log activity within the same transaction
    let activity = CreateActivityLog {
        activity_type: activity_types::PIPELINE_DELETED.to_string(),
        entity_type: "pipeline".to_string(),
        entity_id: pipeline_uuid.to_string(),
        actor_id,
        actor_name,
        description: format!("Deleted pipeline '{}'", pipeline_name),
        metadata: Some(serde_json::json!({
            "pipeline_id": pipeline_uuid.to_string(),
            "pipeline_name": pipeline_name,
        })),
    };

    // Log activity - propagate errors in transaction context
    activity_repo
        .create_activity_tx(conn, activity)
        .await
        .map_err(Error::DatabaseError)?;

    Ok(())
}

// Helper functions copied from pipeline.rs to avoid circular dependencies

fn map_to_create_pipeline(team_id: Uuid, input: CreatePipelineInput) -> CreatePipeline {
    let mut pipeline = CreatePipeline {
        team_id,
        name: input.name.clone(),
        stages: vec![],
    };

    let stages = get_stages_to_create(input.stages, input.relationships);
    pipeline.stages = stages;
    pipeline
}

fn map_to_update_pipeline(id: ID, input: UpdatePipelineInput) -> UpdatePipeline {
    let id = Uuid::try_from(id).unwrap();
    let mut update = UpdatePipeline {
        id,
        name: input.name,
        active: input.active,
        stages: vec![],
    };

    update.stages = get_stages_to_create(input.stages, input.relationships);
    update
}

fn get_stages_to_create(
    stages: Vec<CreateStageInput>,
    relationships: Vec<CreateRelationshipInput>,
) -> Vec<CreateStage> {
    let stages = stages
        .into_iter()
        .map(|stage| {
            CreateStage::new(
                Uuid::new_v4(),
                id_to_uuid(stage.environment_id).unwrap(),
                stage.order_index,
                None,
                stage.position,
            )
        })
        .collect::<Vec<CreateStage>>();

    // Use shared relationship building logic
    build_stage_relationships(stages, relationships)
}
