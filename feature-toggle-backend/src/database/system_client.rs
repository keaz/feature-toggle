use crate::Error;
use crate::database::entity::SystemClient;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

const ROLE_ID_APPROVER: &str = "00000000-0000-0000-0000-000000000001";
const ROLE_ID_REQUESTER: &str = "00000000-0000-0000-0000-000000000002";

pub struct CreateSystemClient {
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub expires_at: DateTime<Utc>,
}

pub struct UpdateSystemClient {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[automock]
#[async_trait]
pub trait SystemClientRepository: Send + Sync {
    async fn list_system_clients(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<SystemClient>, i64), Error>;

    async fn get_system_client_by_id(&self, id: Uuid) -> Result<SystemClient, Error>;

    async fn create_system_client(
        &self,
        team_id: Uuid,
        input: CreateSystemClient,
    ) -> Result<SystemClient, Error>;

    async fn update_system_client(
        &self,
        id: Uuid,
        input: UpdateSystemClient,
    ) -> Result<SystemClient, Error>;

    async fn touch_last_used(&self, id: Uuid) -> Result<(), Error>;

    async fn get_team_id_for_system_client(&self, id: Uuid) -> Result<Option<Uuid>, Error>;

    fn clone_box(&self) -> Box<dyn SystemClientRepository>;
}

impl Clone for Box<dyn SystemClientRepository> {
    fn clone(&self) -> Box<dyn SystemClientRepository> {
        self.clone_box()
    }
}

pub fn system_client_repository(pool: PgPool) -> Box<dyn SystemClientRepository> {
    Box::new(SystemClientRepositoryImpl { pool })
}

#[derive(Clone)]
struct SystemClientRepositoryImpl {
    pool: PgPool,
}

impl SystemClientRepositoryImpl {
    fn map_db_error(error: sqlx::Error, duplicate_field: &'static str) -> Error {
        match error {
            sqlx::Error::RowNotFound => Error::NotFound(Uuid::nil()),
            sqlx::Error::Database(db_err) => {
                if let Some(code) = db_err.code()
                    && code == "23505"
                {
                    return Error::RecordAlreadyExists(duplicate_field.to_string());
                }
                Error::DatabaseError(sqlx::Error::Database(db_err))
            }
            other => Error::DatabaseError(other),
        }
    }

    fn shadow_username(system_client_id: Uuid) -> String {
        format!("system_client_{}", system_client_id.simple())
    }

    fn shadow_email(system_client_id: Uuid) -> String {
        format!(
            "system-client-{}@automation.local",
            system_client_id.simple()
        )
    }
}

#[async_trait]
impl SystemClientRepository for SystemClientRepositoryImpl {
    async fn list_system_clients(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<SystemClient>, i64), Error> {
        let name_filter = name;

        let clients = sqlx::query_as::<_, SystemClient>(
            r#"
            SELECT id, team_id, name, description, enabled, expires_at, created_at, updated_at, last_used_at
            FROM system_clients
            WHERE team_id = $1
              AND ($2::TEXT IS NULL OR name ILIKE ('%' || $2 || '%'))
              AND ($3::BOOLEAN IS NULL OR enabled = $3)
            ORDER BY created_at DESC
            OFFSET $4
            LIMIT $5
            "#,
        )
        .bind(team_id)
        .bind(name_filter.clone())
        .bind(enabled)
        .bind(offset.max(0))
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        let total_row = sqlx::query(
            r#"
            SELECT COUNT(*)::BIGINT AS total
            FROM system_clients
            WHERE team_id = $1
              AND ($2::TEXT IS NULL OR name ILIKE ('%' || $2 || '%'))
              AND ($3::BOOLEAN IS NULL OR enabled = $3)
            "#,
        )
        .bind(team_id)
        .bind(name_filter)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        let total: i64 = total_row.get("total");
        Ok((clients, total))
    }

    async fn get_system_client_by_id(&self, id: Uuid) -> Result<SystemClient, Error> {
        sqlx::query_as::<_, SystemClient>(
            r#"
            SELECT id, team_id, name, description, enabled, expires_at, created_at, updated_at, last_used_at
            FROM system_clients
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => Error::NotFound(id),
            other => Error::DatabaseError(other),
        })
    }

    async fn create_system_client(
        &self,
        team_id: Uuid,
        input: CreateSystemClient,
    ) -> Result<SystemClient, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;

        let created = sqlx::query_as::<_, SystemClient>(
            r#"
            INSERT INTO system_clients (team_id, name, description, enabled, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, team_id, name, description, enabled, expires_at, created_at, updated_at, last_used_at
            "#,
        )
        .bind(team_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.enabled)
        .bind(input.expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| Self::map_db_error(e, "system client"))?;

        let shadow_username = Self::shadow_username(created.id);
        let shadow_email = Self::shadow_email(created.id);

        sqlx::query(
            r#"
            INSERT INTO users (
                id, username, password_hash, first_name, last_name, email, is_admin, enabled, is_temporary_password
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(created.id)
        .bind(shadow_username)
        .bind("SYSTEM_CLIENT_NO_LOGIN")
        .bind(created.name.clone())
        .bind("Automation")
        .bind(shadow_email)
        .bind(true)
        .bind(true)
        .bind(false)
        .execute(&mut *tx)
        .await
        .map_err(Error::DatabaseError)?;

        let approver_role_id =
            Uuid::parse_str(ROLE_ID_APPROVER).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let requester_role_id =
            Uuid::parse_str(ROLE_ID_REQUESTER).map_err(|e| Error::InvalidInput(e.to_string()))?;

        for role_id in [approver_role_id, requester_role_id] {
            sqlx::query(
                r#"
                INSERT INTO user_roles (user_id, role_id, assigned_by)
                VALUES ($1, $2, NULL)
                ON CONFLICT (user_id, role_id) DO NOTHING
                "#,
            )
            .bind(created.id)
            .bind(role_id)
            .execute(&mut *tx)
            .await
            .map_err(Error::DatabaseError)?;
        }

        tx.commit().await.map_err(Error::DatabaseError)?;

        Ok(created)
    }

    async fn update_system_client(
        &self,
        id: Uuid,
        input: UpdateSystemClient,
    ) -> Result<SystemClient, Error> {
        let mut query = QueryBuilder::<Postgres>::new(
            "UPDATE system_clients SET updated_at = CURRENT_TIMESTAMP",
        );

        if let Some(name) = input.name {
            query.push(", name = ");
            query.push_bind(name);
        }
        if let Some(description) = input.description {
            query.push(", description = ");
            query.push_bind(description);
        }
        if let Some(enabled) = input.enabled {
            query.push(", enabled = ");
            query.push_bind(enabled);
        }
        if let Some(expires_at) = input.expires_at {
            query.push(", expires_at = ");
            query.push_bind(expires_at);
        }

        query.push(" WHERE id = ");
        query.push_bind(id);
        query.push(
            " RETURNING id, team_id, name, description, enabled, expires_at, created_at, updated_at, last_used_at",
        );

        query
            .build_query_as::<SystemClient>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::NotFound(id),
                other => Self::map_db_error(other, "system client"),
            })
    }

    async fn touch_last_used(&self, id: Uuid) -> Result<(), Error> {
        sqlx::query(
            r#"
            UPDATE system_clients
            SET last_used_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(())
    }

    async fn get_team_id_for_system_client(&self, id: Uuid) -> Result<Option<Uuid>, Error> {
        let row = sqlx::query("SELECT team_id FROM system_clients WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;

        Ok(row.map(|r| r.get("team_id")))
    }

    fn clone_box(&self) -> Box<dyn SystemClientRepository> {
        Box::new(self.clone())
    }
}
