use crate::Error;
use crate::database::context::{
    ContextRepositoryTx, CreateContextInput as DbCreate, UpdateContextInput as DbUpdate,
};
use crate::database::entity;
use crate::model::{
    Context as ModelContext, ContextEntry as ModelContextEntry, CreateContextInput,
    UpdateContextInput,
};
use crate::model::ID;
use sqlx::PgConnection;
use uuid::Uuid;

pub async fn create_context_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    team_id: ID,
    input: CreateContextInput,
) -> Result<ModelContext, Error>
where
    R: ContextRepositoryTx + ?Sized,
{
    // Basic validation
    if input.key.trim().is_empty() {
        return Err(Error::InvalidInput(
            "Context key cannot be empty".to_string(),
        ));
    }
    let mut set = std::collections::HashSet::new();
    for v in &input.entries {
        if !set.insert(v) {
            return Err(Error::InvalidInput("Duplicate context entry".to_string()));
        }
    }

    let team_uuid = Uuid::try_from(team_id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let created = repo
        .create_context_tx(
            conn,
            team_uuid,
            DbCreate {
                key: input.key,
                entries: input.entries,
            },
        )
        .await?;
    Ok(map_db_to_model(created))
}

pub async fn update_context_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    id: ID,
    input: UpdateContextInput,
) -> Result<ModelContext, Error>
where
    R: ContextRepositoryTx + ?Sized,
{
    if let Some(k) = &input.key
        && k.trim().is_empty()
    {
        return Err(Error::InvalidInput(
            "Context key cannot be empty".to_string(),
        ));
    }
    if let Some(entries) = &input.entries {
        let mut set = std::collections::HashSet::new();
        for v in entries {
            if !set.insert(v) {
                return Err(Error::InvalidInput("Duplicate context entry".to_string()));
            }
        }
    }

    let id_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let updated = repo
        .update_context_tx(
            conn,
            id_uuid,
            DbUpdate {
                key: input.key,
                entries: input.entries,
            },
        )
        .await?;

    Ok(map_db_to_model(updated))
}

pub async fn delete_context_in_tx<R>(conn: &mut PgConnection, repo: &R, id: ID) -> Result<(), Error>
where
    R: ContextRepositoryTx + ?Sized,
{
    let id_uuid = Uuid::try_from(id).map_err(|e| Error::InvalidInput(e.to_string()))?;
    repo.delete_context_tx(conn, id_uuid).await
}

fn map_db_to_model(c: entity::Context) -> ModelContext {
    ModelContext {
        id: ID::from(c.id),
        team_id: ID::from(c.team_id),
        key: c.key,
        entries: c
            .entries
            .into_iter()
            .map(|e| ModelContextEntry {
                id: ID::from(e.id),
                value: e.value,
            })
            .collect(),
    }
}
