use crate::database::entity::Environment;
use crate::database::{Error, handle_error};
use mockall::automock;
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

pub struct CreateEnvironment {
    pub name: String,
    pub active: bool,
    pub environment_type: Option<String>,
}

pub struct UpdateEnvironment {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub environment_type: Option<String>,
}

#[automock]
#[async_trait::async_trait]
pub trait EnvironmentRepository: Send + Sync {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error>;
    async fn get_environments(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error>;
    async fn get_environments_paginated(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Environment>, i64), Error>;
    async fn create_environment(
        &self,
        team_id: Uuid,
        input: CreateEnvironment,
    ) -> Result<Environment, Error>;
    async fn update_environment(
        &self,
        id: Uuid,
        input: UpdateEnvironment,
    ) -> Result<Environment, Error>;
    async fn delete_environment(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn EnvironmentRepository>;
}

impl Clone for Box<dyn EnvironmentRepository> {
    fn clone(&self) -> Box<dyn EnvironmentRepository> {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait EnvironmentRepositoryTx: EnvironmentRepository {
    async fn create_environment_tx(
        &self,
        conn: &mut PgConnection,
        team_id: Uuid,
        input: CreateEnvironment,
    ) -> Result<Environment, Error>;
    async fn update_environment_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateEnvironment,
    ) -> Result<Environment, Error>;
    async fn delete_environment_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error>;
}

pub fn environment_repository(pool: PgPool) -> Box<dyn EnvironmentRepository> {
    Box::new(EnvironmentRepositoryImpl::new(pool))
}

/// Returns a repository that also implements EnvironmentRepositoryTx for transaction support.
pub fn environment_repository_tx(pool: PgPool) -> EnvironmentRepositoryImpl {
    EnvironmentRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct EnvironmentRepositoryImpl {
    pool: PgPool,
}

impl EnvironmentRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl EnvironmentRepository for EnvironmentRepositoryImpl {
    async fn get_environment_by_id(&self, id: Uuid) -> Result<Environment, Error> {
        let result = sqlx::query_as::<_, Environment>(
            r#"SELECT id, name, active, team_id, environment_type FROM environments WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_environments(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error> {
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT id, name, active, team_id, environment_type FROM environments WHERE team_id = ",
        );
        qb.push_bind(team_id);

        if let Some(filter_name) = name {
            let pattern = format!("%{filter_name}%");
            qb.push(" AND name ILIKE ").push_bind(pattern);
        }

        if let Some(active_value) = active {
            qb.push(" AND active = ").push_bind(active_value);
        }
        qb.push(" ORDER BY name");

        let query = qb.build_query_as::<Environment>();
        let result = query.fetch_all(&self.pool).await;

        let environments = handle_error(None, result)?;
        Ok(environments)
    }

    async fn get_environments_paginated(
        &self,
        team_id: Uuid,
        name: Option<String>,
        active: Option<bool>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Environment>, i64), Error> {
        // First, get the total count
        let mut count_qb =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM environments WHERE team_id = ");
        count_qb.push_bind(team_id);

        if let Some(filter_name) = &name {
            let pattern = format!("%{filter_name}%");
            count_qb.push(" AND name ILIKE ").push_bind(pattern);
        }

        if let Some(active_value) = active {
            count_qb.push(" AND active = ").push_bind(active_value);
        }

        let total_count: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::DatabaseError(e))?;

        // Now get the paginated results
        let offset = (page_number - 1) * page_size;
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT id, name, active, team_id, environment_type FROM environments WHERE team_id = ",
        );
        qb.push_bind(team_id);

        if let Some(filter_name) = name {
            let pattern = format!("%{filter_name}%");
            qb.push(" AND name ILIKE ").push_bind(pattern);
        }

        if let Some(active_value) = active {
            qb.push(" AND active = ").push_bind(active_value);
        }
        qb.push(" ORDER BY name");
        qb.push(" LIMIT ").push_bind(page_size);
        qb.push(" OFFSET ").push_bind(offset);

        let query = qb.build_query_as::<Environment>();
        let result = query.fetch_all(&self.pool).await;

        let environments = handle_error(None, result)?;
        Ok((environments, total_count))
    }

    async fn create_environment(
        &self,
        team_id: Uuid,
        input: CreateEnvironment,
    ) -> Result<Environment, Error> {
        let existing_result = self
            .get_environments(team_id, Some(input.name.clone()), None)
            .await;
        if let Ok(existing_environments) = existing_result {
            if !existing_environments.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Environment with name '{}' already exists for team {}",
                    input.name, team_id
                )));
            }
        }
        let id = Uuid::new_v4();
        let environment_type = input
            .environment_type
            .unwrap_or_else(|| "Development".to_string());
        let result = sqlx::query!(
        r#"INSERT INTO environments (id, name, active, team_id, environment_type) VALUES ($1, $2, $3, $4, $5) RETURNING id,name,active, team_id, environment_type"#,
        id,
        input.name,
        input.active,
        team_id,
        environment_type
    )
            .fetch_one(&self.pool)
            .await;

        let handled_error = handle_error(None, result)?;
        Ok(Environment {
            id: handled_error.id,
            name: handled_error.name,
            active: handled_error.active,
            team_id: handled_error.team_id,
            environment_type: handled_error.environment_type,
        })
    }

    async fn update_environment(
        &self,
        id: Uuid,
        input: UpdateEnvironment,
    ) -> Result<Environment, Error> {
        let existing_env = self.get_environment_by_id(id).await?;
        let result = sqlx::query!(
            r#"UPDATE environments SET name = $1, active = $2, environment_type = $3 WHERE id = $4 RETURNING id, name, active, team_id, environment_type"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.environment_type.unwrap_or(existing_env.environment_type),
            id
        ).fetch_one(&self.pool)
            .await;

        let environment = handle_error(Some(id), result)?;

        Ok(Environment {
            id: environment.id,
            name: environment.name,
            active: environment.active,
            team_id: environment.team_id,
            environment_type: environment.environment_type,
        })
    }

    async fn delete_environment(&self, id: Uuid) -> Result<(), Error> {
        self.get_environment_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM environments WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn EnvironmentRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl EnvironmentRepositoryTx for EnvironmentRepositoryImpl {
    async fn create_environment_tx(
        &self,
        conn: &mut PgConnection,
        team_id: Uuid,
        input: CreateEnvironment,
    ) -> Result<Environment, Error> {
        // Check for existing environment (uses pool for read)
        let existing_result = self
            .get_environments(team_id, Some(input.name.clone()), None)
            .await;
        if let Ok(existing_environments) = existing_result {
            if !existing_environments.is_empty() {
                return Err(Error::RecordAlreadyExists(format!(
                    "Environment with name '{}' already exists for team {}",
                    input.name, team_id
                )));
            }
        }

        let id = Uuid::new_v4();
        let environment_type = input
            .environment_type
            .unwrap_or_else(|| "Development".to_string());
        let result = sqlx::query!(
            r#"INSERT INTO environments (id, name, active, team_id, environment_type) VALUES ($1, $2, $3, $4, $5) RETURNING id,name,active, team_id, environment_type"#,
            id,
            input.name,
            input.active,
            team_id,
            environment_type
        )
        .fetch_one(&mut *conn)
        .await;

        let handled_error = handle_error(None, result)?;
        Ok(Environment {
            id: handled_error.id,
            name: handled_error.name,
            active: handled_error.active,
            team_id: handled_error.team_id,
            environment_type: handled_error.environment_type,
        })
    }

    async fn update_environment_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateEnvironment,
    ) -> Result<Environment, Error> {
        let existing_env = self.get_environment_by_id(id).await?;
        let result = sqlx::query!(
            r#"UPDATE environments SET name = $1, active = $2, environment_type = $3 WHERE id = $4 RETURNING id, name, active, team_id, environment_type"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.environment_type.unwrap_or(existing_env.environment_type),
            id
        )
        .fetch_one(&mut *conn)
        .await;

        let environment = handle_error(Some(id), result)?;
        Ok(Environment {
            id: environment.id,
            name: environment.name,
            active: environment.active,
            team_id: environment.team_id,
            environment_type: environment.environment_type,
        })
    }

    async fn delete_environment_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        self.get_environment_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM environments WHERE id = $1", id)
            .execute(&mut *conn)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_environment() -> Environment {
        Environment {
            id: Uuid::new_v4(),
            name: "Test Environment".to_string(),
            active: true,
            team_id: Uuid::new_v4(),
            environment_type: "Development".to_string(),
        }
    }

    fn sample_create_environment() -> CreateEnvironment {
        CreateEnvironment {
            name: "New Environment".to_string(),
            active: true,
            environment_type: Some("Development".to_string()),
        }
    }

    fn sample_update_environment() -> UpdateEnvironment {
        UpdateEnvironment {
            name: Some("Updated Environment".to_string()),
            active: Some(false),
            environment_type: Some("Production".to_string()),
        }
    }

    #[test]
    fn test_environment_struct_creation() {
        let env = sample_environment();
        assert_eq!(env.name, "Test Environment");
        assert!(env.active);
    }

    #[test]
    fn test_create_environment_struct() {
        let create_env = sample_create_environment();
        assert_eq!(create_env.name, "New Environment");
        assert!(create_env.active);
    }

    #[test]
    fn test_update_environment_struct() {
        let update_env = sample_update_environment();
        assert_eq!(update_env.name, Some("Updated Environment".to_string()));
        assert_eq!(update_env.active, Some(false));
    }

    #[test]
    fn test_environment_repository_factory() {
        // Test that the factory function has correct signature
        use sqlx::PgPool;

        fn _verify_signature(_pool: PgPool) -> Box<dyn EnvironmentRepository> {
            environment_repository(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[test]
    fn test_environment_repository_impl_creation() {
        // Test the repository constructor signature
        use sqlx::PgPool;

        fn _verify_signature(_pool: PgPool) -> EnvironmentRepositoryImpl {
            EnvironmentRepositoryImpl::new(_pool)
        }

        // Test passes if it compiles
        assert!(true);
    }

    #[tokio::test]
    async fn test_mock_environment_repository_get_environment_by_id() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let env = sample_environment();
        let env_id = env.id;

        mock_repo
            .expect_get_environment_by_id()
            .with(mockall::predicate::eq(env_id))
            .times(1)
            .returning(move |_| Ok(env.clone()));

        let result = mock_repo.get_environment_by_id(env_id).await;
        assert!(result.is_ok());
        let retrieved_env = result.unwrap();
        assert_eq!(retrieved_env.name, "Test Environment");
        assert!(retrieved_env.active);
    }

    #[tokio::test]
    async fn test_mock_environment_repository_get_environments() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let team_id = Uuid::new_v4();
        let environments = vec![sample_environment()];

        mock_repo
            .expect_get_environments()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
            )
            .times(1)
            .returning(move |_, _, _| Ok(environments.clone()));

        let result = mock_repo.get_environments(team_id, None, None).await;
        assert!(result.is_ok());
        let envs = result.unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].name, "Test Environment");
    }

    #[tokio::test]
    async fn test_mock_environment_repository_create_environment() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let team_id = Uuid::new_v4();
        let create_input = sample_create_environment();
        let expected_env = Environment {
            id: Uuid::new_v4(),
            name: "New Environment".to_string(),
            active: true,
            team_id,
            environment_type: "Development".to_string(),
        };

        mock_repo
            .expect_create_environment()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::function(|input: &CreateEnvironment| {
                    input.name == "New Environment" && input.active
                }),
            )
            .times(1)
            .returning(move |_, _| Ok(expected_env.clone()));

        let result = mock_repo.create_environment(team_id, create_input).await;
        assert!(result.is_ok());
        let created_env = result.unwrap();
        assert_eq!(created_env.name, "New Environment");
        assert!(created_env.active);
    }

    #[tokio::test]
    async fn test_mock_environment_repository_update_environment() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let env_id = Uuid::new_v4();
        let update_input = sample_update_environment();
        let expected_env = Environment {
            id: env_id,
            name: "Updated Environment".to_string(),
            active: false,
            team_id: Uuid::new_v4(),
            environment_type: "Production".to_string(),
        };

        mock_repo
            .expect_update_environment()
            .with(
                mockall::predicate::eq(env_id),
                mockall::predicate::function(|input: &UpdateEnvironment| {
                    input.name == Some("Updated Environment".to_string())
                        && input.active == Some(false)
                }),
            )
            .times(1)
            .returning(move |_, _| Ok(expected_env.clone()));

        let result = mock_repo.update_environment(env_id, update_input).await;
        assert!(result.is_ok());
        let updated_env = result.unwrap();
        assert_eq!(updated_env.name, "Updated Environment");
        assert!(!updated_env.active);
    }

    #[tokio::test]
    async fn test_mock_environment_repository_delete_environment() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let env_id = Uuid::new_v4();

        mock_repo
            .expect_delete_environment()
            .with(mockall::predicate::eq(env_id))
            .times(1)
            .returning(|_| Ok(()));

        let result = mock_repo.delete_environment(env_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_environment_repository_error_scenarios() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let env_id = Uuid::new_v4();

        // Test not found error
        mock_repo
            .expect_get_environment_by_id()
            .with(mockall::predicate::eq(env_id))
            .times(1)
            .returning(move |id| Err(Error::NotFound(id)));

        let result = mock_repo.get_environment_by_id(env_id).await;
        assert!(result.is_err());
        match result.err().unwrap() {
            Error::NotFound(id) => assert_eq!(id, env_id),
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_mock_environment_repository_with_filters() {
        let mut mock_repo = MockEnvironmentRepository::new();
        let team_id = Uuid::new_v4();
        let name_filter = Some("Test".to_string());
        let active_filter = Some(true);
        let environments = vec![sample_environment()];

        mock_repo
            .expect_get_environments()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(name_filter.clone()),
                mockall::predicate::eq(active_filter),
            )
            .times(1)
            .returning(move |_, _, _| Ok(environments.clone()));

        let result = mock_repo
            .get_environments(team_id, name_filter, active_filter)
            .await;
        assert!(result.is_ok());
        let envs = result.unwrap();
        assert_eq!(envs.len(), 1);
    }
}
