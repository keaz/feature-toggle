use crate::database::entity::{Context, ContextEntry};
use crate::database::{handle_error, Error};
use log::{debug, info};
use mockall::automock;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreateContextInput {
    pub key: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateContextInput {
    pub key: Option<String>,
    pub entries: Option<Vec<String>>, // full replacement for simplicity
}

#[automock]
#[async_trait::async_trait]
pub trait ContextRepository: Send + Sync {
    async fn get_context_by_id(&self, id: Uuid) -> Result<Context, Error>;
    async fn get_contexts(&self, team_id: Uuid, key: Option<String>) -> Result<Vec<Context>, Error>;
    async fn create_context(&self, team_id: Uuid, input: CreateContextInput) -> Result<Context, Error>;
    async fn update_context(&self, id: Uuid, input: UpdateContextInput) -> Result<Context, Error>;
    async fn delete_context(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn ContextRepository>;
}

impl Clone for Box<dyn ContextRepository> {
    fn clone(&self) -> Box<dyn ContextRepository> { self.clone_box() }
}

pub fn context_repository(pool: PgPool) -> Box<dyn ContextRepository> {
    Box::new(ContextRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct ContextRepositoryImpl { pool: PgPool }

impl ContextRepositoryImpl { pub fn new(pool: PgPool) -> Self { Self { pool } } }

#[async_trait::async_trait]
impl ContextRepository for ContextRepositoryImpl {
    async fn get_context_by_id(&self, id: Uuid) -> Result<Context, Error> {
        debug!("DB: get_context_by_id {id}");
        let ctx_row = sqlx::query!(
            r#"SELECT id, team_id, key FROM contexts WHERE id = $1"#,
            id
        )
        .fetch_one(&self.pool)
        .await;
        let ctx_row = handle_error(Some(id), ctx_row)?;

        let entries = sqlx::query!(
            r#"SELECT id, value FROM context_entries WHERE context_id = $1 ORDER BY value"#,
            id
        )
        .fetch_all(&self.pool)
        .await;
        let entries = handle_error(Some(id), entries)?;

        Ok(Context {
            id: ctx_row.id,
            team_id: ctx_row.team_id,
            key: ctx_row.key,
            entries: entries
                .into_iter()
                .map(|e| ContextEntry { id: e.id, value: e.value })
                .collect(),
        })
    }

    async fn get_contexts(&self, team_id: Uuid, key: Option<String>) -> Result<Vec<Context>, Error> {
        debug!("DB: get_contexts team={team_id} key={key:?}");
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT c.id, c.team_id, c.key FROM contexts c WHERE c.team_id = ",
        );
        qb.push_bind(team_id);
        if let Some(k) = key {
            let pattern = format!("%{}%", k);
            qb.push(" AND c.key ILIKE ").push_bind(pattern);
        }
        qb.push(" ORDER BY c.key");
        let rows = qb.build().fetch_all(&self.pool).await;
        let rows = handle_error(None, rows)?;
        // collect ids to batch entries
        let ids: Vec<Uuid> = rows.iter().map(|r| r.get::<Uuid, _>(0)).collect();
        // Since QueryBuilder::build returns generic Row, we'll fetch entries per id for simplicity
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Uuid = row.get::<Uuid, _>(0);
            let team_id: Uuid = row.get::<Uuid, _>(1);
            let key: String = row.get::<String, _>(2);
            let entries = sqlx::query!(
                r#"SELECT id, value FROM context_entries WHERE context_id = $1 ORDER BY value"#,
                id
            )
            .fetch_all(&self.pool)
            .await;
            let entries = handle_error(Some(id), entries)?;
            result.push(Context {
                id,
                team_id,
                key,
                entries: entries
                    .into_iter()
                    .map(|e| ContextEntry { id: e.id, value: e.value })
                    .collect(),
            });
        }
        Ok(result)
    }

    async fn create_context(&self, team_id: Uuid, input: CreateContextInput) -> Result<Context, Error> {
        info!("DB: create_context team={team_id} key={}", input.key);
        // Check duplicate key for team
        let exists = sqlx::query_scalar!(
            r#"SELECT 1 FROM contexts WHERE team_id = $1 AND key = $2"#,
            team_id,
            input.key
        )
        .fetch_optional(&self.pool)
        .await;
        let exists = handle_error(None, exists)?;
        if exists.is_some() { return Err(Error::RecordAlreadyExists("Context key already exists for team".to_string())); }

        let id = Uuid::new_v4();
        let ctx_row = sqlx::query!(
            r#"INSERT INTO contexts (id, team_id, key) VALUES ($1, $2, $3) RETURNING id, team_id, key"#,
            id,
            team_id,
            input.key
        )
        .fetch_one(&self.pool)
        .await;
        let ctx_row = handle_error(None, ctx_row)?;

        // insert entries unique per context
        for value in input.entries {
            let eid = Uuid::new_v4();
            let _ = handle_error(None, sqlx::query!(
                r#"INSERT INTO context_entries (id, context_id, value) VALUES ($1, $2, $3)"#,
                eid,
                ctx_row.id,
                value
            )
            .execute(&self.pool)
            .await)?;
        }

        self.get_context_by_id(ctx_row.id).await
    }

    async fn update_context(&self, id: Uuid, input: UpdateContextInput) -> Result<Context, Error> {
        info!("DB: update_context id={id}");
        // Ensure exists
        let existing = self.get_context_by_id(id).await?;
        let new_key = input.key.unwrap_or(existing.key.clone());
        if new_key != existing.key {
            // check unique within team
            let exists = sqlx::query_scalar!(
                r#"SELECT 1 FROM contexts WHERE team_id = $1 AND key = $2 AND id <> $3"#,
                existing.team_id,
                new_key,
                id
            )
            .fetch_optional(&self.pool)
            .await;
            let exists = handle_error(None, exists)?;
            if exists.is_some() { return Err(Error::RecordAlreadyExists("Context key already exists for team".to_string())); }
        }
        let _ = handle_error(Some(id), sqlx::query!(
            r#"UPDATE contexts SET key = $1 WHERE id = $2"#,
            new_key,
            id
        )
        .execute(&self.pool)
        .await)?;

        if let Some(entries) = input.entries {
            // Replace entries
            let _ = handle_error(Some(id), sqlx::query!(
                r#"DELETE FROM context_entries WHERE context_id = $1"#,
                id
            )
            .execute(&self.pool)
            .await)?;
            for v in entries {
                let eid = Uuid::new_v4();
                let _ = handle_error(None, sqlx::query!(
                    r#"INSERT INTO context_entries (id, context_id, value) VALUES ($1, $2, $3)"#,
                    eid,
                    id,
                    v
                )
                .execute(&self.pool)
                .await)?;
            }
        }

        self.get_context_by_id(id).await
    }

    async fn delete_context(&self, id: Uuid) -> Result<(), Error> {
        info!("DB: delete_context id={id}");
        // ensure exists
        let _ = self.get_context_by_id(id).await?;
        let _ = handle_error(Some(id), sqlx::query!("DELETE FROM context_entries WHERE context_id = $1", id)
            .execute(&self.pool)
            .await)?;
        let result = sqlx::query!("DELETE FROM contexts WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn ContextRepository> { Box::new(self.clone()) }
}
