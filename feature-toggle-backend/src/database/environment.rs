use crate::database::entity::Environment;
use crate::database::{handle_error, Error};
use mockall::automock;
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

pub struct CreateEnvironment {
    pub name: String,
}

pub struct UpdateEnvironment {
    pub id: Uuid,
    pub name: Option<String>,
    pub active: Option<bool>,
}

#[automock]
#[async_trait::async_trait]
pub trait EnvironmentRepository: Send + Sync {
    async fn get_environment_by_id(&self, env_id: Uuid) -> Result<Environment, Error>;
    async fn get_environments(
        &self,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error>;
    async fn create_environment(&self, input: CreateEnvironment)
    -> Result<Environment, Error>;
    async fn update_environment(&self, input: UpdateEnvironment)
    -> Result<Environment, Error>;
    async fn delete_environment(&self, id: Uuid) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn EnvironmentRepository>;
}

impl Clone for Box<dyn EnvironmentRepository> {
    fn clone(&self) -> Box<dyn EnvironmentRepository> {
        self.clone_box()
    }
}

pub fn environment_repository(pool: PgPool) -> Box<dyn EnvironmentRepository> {
    Box::new(EnvironmentRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct EnvironmentRepositoryImpl {
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
            r#"SELECT id, name, active FROM environments WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(id), result)
    }

    async fn get_environments(
        &self,
        name: Option<String>,
        active: Option<bool>,
    ) -> Result<Vec<Environment>, Error> {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT id, name, active FROM environments");

        let mut has_where = false;

        if name.is_some() || active.is_some() {
            qb.push(" WHERE ");
        }

        if let Some(filter_name) = name {
            let pattern = format!("%{}%", filter_name);
            qb.push("name ILIKE ").push_bind(pattern);
            has_where = true;
        }

        if let Some(active_value) = active {
            if has_where {
                qb.push(" AND ");
            }
            qb.push("active = ").push_bind(active_value);
        }

        let query = qb.build_query_as::<Environment>();
        let result = query.fetch_all(&self.pool).await;

        let environments = handle_error(None, result)?;
        Ok(environments)
    }

    async fn create_environment(
        &self,
        input: CreateEnvironment,
    ) -> Result<Environment, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
        r#"INSERT INTO environments (id, name, active) VALUES ($1, $2, true) RETURNING id,name,active"#,
        id,
        input.name
    )
            .fetch_one(&self.pool)
            .await;

        let handled_error = handle_error(None, result)?;
        Ok(Environment {
            id: handled_error.id,
            name: handled_error.name,
            active: handled_error.active,
        })
    }

    async fn update_environment(
        &self,
        input: UpdateEnvironment,
    ) -> Result<Environment, Error> {
        let existing_env = self.get_environment_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE environments SET name = $1, active = $2 WHERE id = $3 RETURNING id, name, active"#,
            input.name.unwrap_or(existing_env.name),
            input.active.unwrap_or(existing_env.active),
            input.id
        ).fetch_one(&self.pool)
            .await;

        let environment = handle_error(Some(input.id), result)?;

        Ok(Environment {
            id: environment.id,
            name: environment.name,
            active: environment.active,
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
