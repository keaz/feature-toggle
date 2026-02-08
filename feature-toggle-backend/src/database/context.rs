use crate::database::entity::{Context, ContextEntry};
use crate::database::{Error, handle_error};
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
    async fn get_contexts(&self, team_id: Uuid, key: Option<String>)
    -> Result<Vec<Context>, Error>;
    async fn get_contexts_paginated(
        &self,
        team_id: Uuid,
        key: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Context>, i64), Error>;
    async fn get_contexts_with_offset(
        &self,
        team_id: Uuid,
        key: Option<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Context>, i64), Error>;
    async fn create_context(
        &self,
        team_id: Uuid,
        input: CreateContextInput,
    ) -> Result<Context, Error>;
    async fn update_context(&self, id: Uuid, input: UpdateContextInput) -> Result<Context, Error>;
    async fn delete_context(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn ContextRepository>;
}

impl Clone for Box<dyn ContextRepository> {
    fn clone(&self) -> Box<dyn ContextRepository> {
        self.clone_box()
    }
}

pub fn context_repository(pool: PgPool) -> Box<dyn ContextRepository> {
    Box::new(ContextRepositoryImpl::new(pool))
}

pub fn context_repository_tx(pool: PgPool) -> ContextRepositoryImpl {
    ContextRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct ContextRepositoryImpl {
    pool: PgPool,
}

impl ContextRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ContextRepository for ContextRepositoryImpl {
    async fn get_context_by_id(&self, id: Uuid) -> Result<Context, Error> {
        debug!("DB: get_context_by_id {id}");
        let ctx_row = sqlx::query!(r#"SELECT id, team_id, key FROM contexts WHERE id = $1"#, id)
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
                .map(|e| ContextEntry {
                    id: e.id,
                    value: e.value,
                })
                .collect(),
        })
    }

    async fn get_contexts(
        &self,
        team_id: Uuid,
        key: Option<String>,
    ) -> Result<Vec<Context>, Error> {
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
        let _ids: Vec<Uuid> = rows.iter().map(|r| r.get::<Uuid, _>(0)).collect();
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
                    .map(|e| ContextEntry {
                        id: e.id,
                        value: e.value,
                    })
                    .collect(),
            });
        }
        Ok(result)
    }

    async fn get_contexts_paginated(
        &self,
        team_id: Uuid,
        key: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Context>, i64), Error> {
        debug!(
            "DB: get_contexts_paginated team={team_id} key={key:?} page={page_number} size={page_size}"
        );

        // First, get the total count
        let mut count_qb =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM contexts c WHERE c.team_id = ");
        count_qb.push_bind(team_id);
        if let Some(k) = &key {
            let pattern = format!("%{}%", k);
            count_qb.push(" AND c.key ILIKE ").push_bind(pattern);
        }

        let total_count: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;

        // Now get the paginated results
        let offset = (page_number - 1) * page_size;
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT c.id, c.team_id, c.key FROM contexts c WHERE c.team_id = ",
        );
        qb.push_bind(team_id);
        if let Some(k) = key {
            let pattern = format!("%{}%", k);
            qb.push(" AND c.key ILIKE ").push_bind(pattern);
        }
        qb.push(" ORDER BY c.key");
        qb.push(" LIMIT ").push_bind(page_size);
        qb.push(" OFFSET ").push_bind(offset);

        let rows = qb.build().fetch_all(&self.pool).await;
        let rows = handle_error(None, rows)?;

        // collect ids to batch entries
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
                    .map(|e| ContextEntry {
                        id: e.id,
                        value: e.value,
                    })
                    .collect(),
            });
        }
        Ok((result, total_count))
    }

    async fn get_contexts_with_offset(
        &self,
        team_id: Uuid,
        key: Option<String>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Context>, i64), Error> {
        debug!(
            "DB: get_contexts_with_offset team={team_id} key={key:?} offset={offset} limit={limit}"
        );

        let offset = offset.max(0);
        let limit = limit.max(1);

        let mut count_qb =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM contexts c WHERE c.team_id = ");
        count_qb.push_bind(team_id);
        if let Some(k) = &key {
            let pattern = format!("%{}%", k);
            count_qb.push(" AND c.key ILIKE ").push_bind(pattern);
        }

        let total_count: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;

        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT c.id, c.team_id, c.key FROM contexts c WHERE c.team_id = ",
        );
        qb.push_bind(team_id);
        if let Some(k) = key {
            let pattern = format!("%{}%", k);
            qb.push(" AND c.key ILIKE ").push_bind(pattern);
        }
        qb.push(" ORDER BY c.key");
        qb.push(" LIMIT ").push_bind(limit);
        qb.push(" OFFSET ").push_bind(offset);

        let rows = qb.build().fetch_all(&self.pool).await;
        let rows = handle_error(None, rows)?;

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
                    .map(|e| ContextEntry {
                        id: e.id,
                        value: e.value,
                    })
                    .collect(),
            });
        }

        Ok((result, total_count))
    }

    async fn create_context(
        &self,
        team_id: Uuid,
        input: CreateContextInput,
    ) -> Result<Context, Error> {
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
        if exists.is_some() {
            return Err(Error::RecordAlreadyExists(
                "Context key already exists for team".to_string(),
            ));
        }

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
            let _ = handle_error(
                None,
                sqlx::query!(
                    r#"INSERT INTO context_entries (id, context_id, value) VALUES ($1, $2, $3)"#,
                    eid,
                    ctx_row.id,
                    value
                )
                .execute(&self.pool)
                .await,
            )?;
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
            if exists.is_some() {
                return Err(Error::RecordAlreadyExists(
                    "Context key already exists for team".to_string(),
                ));
            }
        }
        let _ = handle_error(
            Some(id),
            sqlx::query!(r#"UPDATE contexts SET key = $1 WHERE id = $2"#, new_key, id)
                .execute(&self.pool)
                .await,
        )?;

        if let Some(entries) = input.entries {
            // Replace entries
            let _ = handle_error(
                Some(id),
                sqlx::query!(r#"DELETE FROM context_entries WHERE context_id = $1"#, id)
                    .execute(&self.pool)
                    .await,
            )?;
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
        let _ = handle_error(
            Some(id),
            sqlx::query!("DELETE FROM context_entries WHERE context_id = $1", id)
                .execute(&self.pool)
                .await,
        )?;
        let result = sqlx::query!("DELETE FROM contexts WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn ContextRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
pub trait ContextRepositoryTx: Send + Sync {
    async fn create_context_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        team_id: Uuid,
        input: CreateContextInput,
    ) -> Result<Context, Error>;
    async fn update_context_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        id: Uuid,
        input: UpdateContextInput,
    ) -> Result<Context, Error>;
    async fn delete_context_tx(&self, conn: &mut sqlx::PgConnection, id: Uuid)
    -> Result<(), Error>;
    async fn get_context_by_id_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        id: Uuid,
    ) -> Result<Context, Error>;
}

#[async_trait::async_trait]
impl ContextRepositoryTx for ContextRepositoryImpl {
    async fn create_context_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        team_id: Uuid,
        input: CreateContextInput,
    ) -> Result<Context, Error> {
        // Check duplicate key for team
        let exists = sqlx::query_scalar!(
            r#"SELECT 1 FROM contexts WHERE team_id = $1 AND key = $2"#,
            team_id,
            input.key
        )
        .fetch_optional(&mut *conn)
        .await;
        let exists = handle_error(None, exists)?;
        if exists.is_some() {
            return Err(Error::RecordAlreadyExists(
                "Context key already exists for team".to_string(),
            ));
        }

        let id = Uuid::new_v4();
        let ctx_row = sqlx::query!(
            r#"INSERT INTO contexts (id, team_id, key) VALUES ($1, $2, $3) RETURNING id, team_id, key"#,
            id,
            team_id,
            input.key
        )
        .fetch_one(&mut *conn)
        .await;
        let ctx_row = handle_error(None, ctx_row)?;

        // insert entries unique per context
        for value in input.entries {
            let eid = Uuid::new_v4();
            let _ = handle_error(
                None,
                sqlx::query!(
                    r#"INSERT INTO context_entries (id, context_id, value) VALUES ($1, $2, $3)"#,
                    eid,
                    ctx_row.id,
                    value
                )
                .execute(&mut *conn)
                .await,
            )?;
        }

        self.get_context_by_id_tx(conn, ctx_row.id).await
    }

    async fn update_context_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        id: Uuid,
        input: UpdateContextInput,
    ) -> Result<Context, Error> {
        // Ensure exists
        let existing = self.get_context_by_id_tx(conn, id).await?;
        let new_key = input.key.unwrap_or(existing.key.clone());
        if new_key != existing.key {
            // check unique within team
            let exists = sqlx::query_scalar!(
                r#"SELECT 1 FROM contexts WHERE team_id = $1 AND key = $2 AND id <> $3"#,
                existing.team_id,
                new_key,
                id
            )
            .fetch_optional(&mut *conn)
            .await;
            let exists = handle_error(None, exists)?;
            if exists.is_some() {
                return Err(Error::RecordAlreadyExists(
                    "Context key already exists for team".to_string(),
                ));
            }
        }
        let _ = handle_error(
            Some(id),
            sqlx::query!(r#"UPDATE contexts SET key = $1 WHERE id = $2"#, new_key, id)
                .execute(&mut *conn)
                .await,
        )?;

        if let Some(entries) = input.entries {
            // Replace entries
            let _ = handle_error(
                Some(id),
                sqlx::query!(r#"DELETE FROM context_entries WHERE context_id = $1"#, id)
                    .execute(&mut *conn)
                    .await,
            )?;
            for v in entries {
                let eid = Uuid::new_v4();
                let _ = handle_error(None, sqlx::query!(
                    r#"INSERT INTO context_entries (id, context_id, value) VALUES ($1, $2, $3)"#,
                    eid,
                    id,
                    v
                )
                .execute(&mut *conn)
                .await)?;
            }
        }

        self.get_context_by_id_tx(conn, id).await
    }

    async fn delete_context_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        id: Uuid,
    ) -> Result<(), Error> {
        // ensure exists
        let _ = self.get_context_by_id_tx(conn, id).await?;
        let _ = handle_error(
            Some(id),
            sqlx::query!("DELETE FROM context_entries WHERE context_id = $1", id)
                .execute(&mut *conn)
                .await,
        )?;
        let result = sqlx::query!("DELETE FROM contexts WHERE id = $1", id)
            .execute(&mut *conn)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    async fn get_context_by_id_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        id: Uuid,
    ) -> Result<Context, Error> {
        let ctx_row = sqlx::query!(r#"SELECT id, team_id, key FROM contexts WHERE id = $1"#, id)
            .fetch_one(&mut *conn)
            .await;
        let ctx_row = handle_error(Some(id), ctx_row)?;

        let entries = sqlx::query!(
            r#"SELECT id, value FROM context_entries WHERE context_id = $1 ORDER BY value"#,
            id
        )
        .fetch_all(&mut *conn)
        .await;
        let entries = handle_error(Some(id), entries)?;

        Ok(Context {
            id: ctx_row.id,
            team_id: ctx_row.team_id,
            key: ctx_row.key,
            entries: entries
                .into_iter()
                .map(|e| ContextEntry {
                    id: e.id,
                    value: e.value,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_context() -> Context {
        Context {
            id: Uuid::new_v4(),
            key: "user.region".to_string(),
            team_id: Uuid::new_v4(),
            entries: vec![
                ContextEntry {
                    id: Uuid::new_v4(),
                    value: "US".to_string(),
                },
                ContextEntry {
                    id: Uuid::new_v4(),
                    value: "EU".to_string(),
                },
            ],
        }
    }

    fn sample_create_context_input() -> CreateContextInput {
        CreateContextInput {
            key: "user.device".to_string(),
            entries: vec!["mobile".to_string(), "desktop".to_string()],
        }
    }

    fn sample_update_context_input() -> UpdateContextInput {
        UpdateContextInput {
            key: Some("user.tier".to_string()),
            entries: Some(vec!["premium".to_string(), "basic".to_string()]),
        }
    }

    #[test]
    fn test_context_struct_creation() {
        let context = sample_context();
        assert_eq!(context.key, "user.region");
        assert_eq!(context.entries.len(), 2);
        assert_eq!(context.entries[0].value, "US");
        assert_eq!(context.entries[1].value, "EU");
    }

    #[test]
    fn test_create_context_input_struct() {
        let create_input = sample_create_context_input();
        assert_eq!(create_input.key, "user.device");
        assert_eq!(create_input.entries.len(), 2);
        assert!(create_input.entries.contains(&"mobile".to_string()));
        assert!(create_input.entries.contains(&"desktop".to_string()));
    }

    #[test]
    fn test_update_context_input_struct() {
        let update_input = sample_update_context_input();
        assert_eq!(update_input.key, Some("user.tier".to_string()));
        assert!(update_input.entries.is_some());
        let entries = update_input.entries.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&"premium".to_string()));
        assert!(entries.contains(&"basic".to_string()));
    }

    #[test]
    fn test_context_repository_factory() {
        // Test that the factory function has correct signature
        use sqlx::PgPool;

        fn _verify_signature(_pool: PgPool) -> Box<dyn ContextRepository> {
            context_repository(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[test]
    fn test_context_repository_impl_creation() {
        // Test the repository constructor signature
        use sqlx::PgPool;

        fn _verify_signature(_pool: PgPool) -> ContextRepositoryImpl {
            ContextRepositoryImpl::new(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[tokio::test]
    async fn test_mock_context_repository_get_context_by_id() {
        let mut mock_repo = MockContextRepository::new();
        let context = sample_context();
        let context_id = context.id;

        mock_repo
            .expect_get_context_by_id()
            .with(mockall::predicate::eq(context_id))
            .times(1)
            .returning(move |_| Ok(context.clone()));

        let result = mock_repo.get_context_by_id(context_id).await;
        assert!(result.is_ok());
        let retrieved_context = result.unwrap();
        assert_eq!(retrieved_context.key, "user.region");
        assert_eq!(retrieved_context.entries.len(), 2);
    }

    #[tokio::test]
    async fn test_mock_context_repository_get_contexts() {
        let mut mock_repo = MockContextRepository::new();
        let team_id = Uuid::new_v4();
        let contexts = vec![sample_context()];

        mock_repo
            .expect_get_contexts()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
            )
            .times(1)
            .returning(move |_, _| Ok(contexts.clone()));

        let result = mock_repo.get_contexts(team_id, None).await;
        assert!(result.is_ok());
        let ctxs = result.unwrap();
        assert_eq!(ctxs.len(), 1);
        assert_eq!(ctxs[0].key, "user.region");
    }

    #[tokio::test]
    async fn test_mock_context_repository_create_context() {
        let mut mock_repo = MockContextRepository::new();
        let team_id = Uuid::new_v4();
        let create_input = sample_create_context_input();
        let expected_context = Context {
            id: Uuid::new_v4(),
            key: "user.device".to_string(),
            team_id,
            entries: vec![
                ContextEntry {
                    id: Uuid::new_v4(),
                    value: "mobile".to_string(),
                },
                ContextEntry {
                    id: Uuid::new_v4(),
                    value: "desktop".to_string(),
                },
            ],
        };

        mock_repo
            .expect_create_context()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::function(|input: &CreateContextInput| {
                    input.key == "user.device" && input.entries.len() == 2
                }),
            )
            .times(1)
            .returning(move |_, _| Ok(expected_context.clone()));

        let result = mock_repo.create_context(team_id, create_input).await;
        assert!(result.is_ok());
        let created_context = result.unwrap();
        assert_eq!(created_context.key, "user.device");
        assert_eq!(created_context.entries.len(), 2);
    }

    #[tokio::test]
    async fn test_mock_context_repository_update_context() {
        let mut mock_repo = MockContextRepository::new();
        let context_id = Uuid::new_v4();
        let update_input = sample_update_context_input();
        let expected_context = Context {
            id: context_id,
            key: "user.tier".to_string(),
            team_id: Uuid::new_v4(),
            entries: vec![
                ContextEntry {
                    id: Uuid::new_v4(),
                    value: "premium".to_string(),
                },
                ContextEntry {
                    id: Uuid::new_v4(),
                    value: "basic".to_string(),
                },
            ],
        };

        mock_repo
            .expect_update_context()
            .with(
                mockall::predicate::eq(context_id),
                mockall::predicate::function(|input: &UpdateContextInput| {
                    input.key == Some("user.tier".to_string())
                        && input.entries.is_some()
                        && input.entries.as_ref().unwrap().len() == 2
                }),
            )
            .times(1)
            .returning(move |_, _| Ok(expected_context.clone()));

        let result = mock_repo.update_context(context_id, update_input).await;
        assert!(result.is_ok());
        let updated_context = result.unwrap();
        assert_eq!(updated_context.key, "user.tier");
        assert_eq!(updated_context.entries.len(), 2);
    }

    #[tokio::test]
    async fn test_mock_context_repository_delete_context() {
        let mut mock_repo = MockContextRepository::new();
        let context_id = Uuid::new_v4();

        mock_repo
            .expect_delete_context()
            .with(mockall::predicate::eq(context_id))
            .times(1)
            .returning(|_| Ok(()));

        let result = mock_repo.delete_context(context_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_context_repository_error_scenarios() {
        let mut mock_repo = MockContextRepository::new();
        let context_id = Uuid::new_v4();

        // Test not found error
        mock_repo
            .expect_get_context_by_id()
            .with(mockall::predicate::eq(context_id))
            .times(1)
            .returning(move |id| Err(Error::NotFound(id)));

        let result = mock_repo.get_context_by_id(context_id).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            Error::NotFound(id) => assert_eq!(id, context_id),
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_mock_context_repository_with_key_filter() {
        let mut mock_repo = MockContextRepository::new();
        let team_id = Uuid::new_v4();
        let key_filter = Some("user.region".to_string());
        let contexts = vec![sample_context()];

        mock_repo
            .expect_get_contexts()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(key_filter.clone()),
            )
            .times(1)
            .returning(move |_, _| Ok(contexts.clone()));

        let result = mock_repo.get_contexts(team_id, key_filter).await;
        assert!(result.is_ok());
        let ctxs = result.unwrap();
        assert_eq!(ctxs.len(), 1);
        assert_eq!(ctxs[0].key, "user.region");
    }

    #[test]
    fn test_create_context_input_empty_entries() {
        let create_input = CreateContextInput {
            key: "user.empty".to_string(),
            entries: vec![],
        };
        assert_eq!(create_input.key, "user.empty");
        assert!(create_input.entries.is_empty());
    }

    #[test]
    fn test_update_context_input_partial_update() {
        let update_input = UpdateContextInput {
            key: Some("user.updated".to_string()),
            entries: None, // Only updating key, not entries
        };
        assert_eq!(update_input.key, Some("user.updated".to_string()));
        assert!(update_input.entries.is_none());
    }
}
