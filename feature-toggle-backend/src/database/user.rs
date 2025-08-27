use crate::database::handle_error;
use crate::database::Error;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgPool, Row, Postgres, QueryBuilder};
use uuid::Uuid;
use crate::database::entity::Team;

#[derive(Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

pub struct CreateUser {
    pub username: String,
    pub password_hash: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
}

pub struct UpdateUser {
    pub id: Uuid,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub is_admin: Option<bool>,
    pub enabled: Option<bool>,
}

#[automock]
#[async_trait::async_trait]
pub trait UserRepository: Send + Sync {
    async fn get_user_by_id(&self, id: Uuid) -> Result<User, Error>;
    async fn get_user_by_username(&self, username: &str) -> Result<User, Error>;
    async fn get_user_by_email(&self, email: &str) -> Result<User, Error>;
    async fn user_exists_by_username(&self, username: &str) -> Result<bool, Error>;
    async fn user_exists_by_email(&self, email: &str, exclude_id: Option<Uuid>) -> Result<bool, Error>;
    async fn create_user(&self, input: CreateUser) -> Result<User, Error>;
    async fn update_user(&self, input: UpdateUser) -> Result<User, Error>;
    async fn update_last_login(&self, id: Uuid, when: DateTime<Utc>) -> Result<(), Error>;
    async fn set_user_teams(&self, id: Uuid, team_ids: Vec<Uuid>) -> Result<(), Error>;
    async fn search_users(&self, team_id: Option<Uuid>, name: Option<String>, page_number: i32, page_size: i32) -> Result<(Vec<User>, i64), Error>;
    async fn get_user_teams(&self, id: Uuid) -> Result<Vec<Team>, Error>;
    fn clone_box(&self) -> Box<dyn UserRepository>;
}

impl Clone for Box<dyn UserRepository> {
    fn clone(&self) -> Box<dyn UserRepository> {
        self.clone_box()
    }
}

pub fn user_repository(pool: PgPool) -> Box<dyn UserRepository> {
    Box::new(UserRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct UserRepositoryImpl {
    pool: PgPool,
}

impl UserRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserRepository for UserRepositoryImpl {
    async fn get_user_by_id(&self, id: Uuid) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, is_admin, enabled,
                       created_at, updated_at, last_login
                FROM users WHERE id = $1"#,
            id
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(Some(id), result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, is_admin, enabled,
                       created_at, updated_at, last_login
                FROM users WHERE username = $1"#,
            username
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(None, result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn get_user_by_email(&self, email: &str) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, is_admin, enabled,
                       created_at, updated_at, last_login
                FROM users WHERE email = $1"#,
            email
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(None, result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn create_user(&self, input: CreateUser) -> Result<User, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
            r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING id, username, password_hash, first_name, last_name, email, is_admin, enabled, created_at, updated_at, last_login"#,
            id,
            input.username,
            input.password_hash,
            input.first_name,
            input.last_name,
            input.email,
            input.is_admin
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(None, result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn update_user(&self, input: UpdateUser) -> Result<User, Error> {
        let existing = self.get_user_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE users
               SET first_name = $1, last_name = $2, email = $3, is_admin = $4, enabled = $5, updated_at = now()
               WHERE id = $6
               RETURNING id, username, password_hash, first_name, last_name, email, is_admin, enabled, created_at, updated_at, last_login"#,
            input.first_name.unwrap_or(existing.first_name),
            input.last_name.unwrap_or(existing.last_name),
            input.email.unwrap_or(existing.email),
            input.is_admin.unwrap_or(existing.is_admin),
            input.enabled.unwrap_or(existing.enabled),
            input.id
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(Some(input.id), result)?;
        Ok(User {
            id: row.id,
            username: row.username,
            password_hash: row.password_hash,
            first_name: row.first_name,
            last_name: row.last_name,
            email: row.email,
            is_admin: row.is_admin,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn update_last_login(&self, id: Uuid, when: DateTime<Utc>) -> Result<(), Error> {
        let _ = handle_error(
            Some(id),
            sqlx::query!(
                r#"UPDATE users SET last_login = $1, updated_at = now() WHERE id = $2"#,
                when,
                id
            )
            .execute(&self.pool)
            .await,
        )?;
        Ok(())
    }

    async fn user_exists_by_username(&self, username: &str) -> Result<bool, Error> {
        let row = handle_error(
            None,
            sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE username = $1) AS exists")
                .bind(username)
                .fetch_one(&self.pool)
                .await,
        )?;
        let exists: bool = row.get::<bool, _>("exists");
        Ok(exists)
    }

    async fn user_exists_by_email(&self, email: &str, exclude_id: Option<Uuid>) -> Result<bool, Error> {
        let query = match exclude_id {
            Some(id) => sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1 AND id <> $2) AS exists")
                .bind(email)
                .bind(id),
            None => sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1) AS exists")
                .bind(email),
        };
        let row = handle_error(None, query.fetch_one(&self.pool).await)?;
        let exists: bool = row.get::<bool, _>("exists");
        Ok(exists)
    }

    async fn set_user_teams(&self, id: Uuid, team_ids: Vec<Uuid>) -> Result<(), Error> {
        let mut tx = self.pool.begin().await.map_err(|e| Error::DatabaseError(e.into()))?;
        // delete existing assignments
        handle_error(
            Some(id),
            sqlx::query("DELETE FROM user_teams WHERE user_id = $1").bind(id).execute(&mut *tx).await,
        )?;
        // insert new ones, if any
        if !team_ids.is_empty() {
            // Build a single multi-values insert for efficiency
            // Use UNNEST for cleaner binding
            let user_ids: Vec<Uuid> = team_ids.iter().map(|_| id).collect();
            handle_error(
                Some(id),
                sqlx::query(
                    r#"INSERT INTO user_teams (user_id, team_id)
                       SELECT * FROM UNNEST($1::uuid[], $2::uuid[])"#,
                )
                .bind(&user_ids)
                .bind(&team_ids)
                .execute(&mut *tx)
                .await,
            )?;
        }
        tx.commit().await.map_err(|e| Error::DatabaseError(e.into()))?;
        Ok(())
    }

    async fn search_users(&self, team_id: Option<Uuid>, name: Option<String>, page_number: i32, page_size: i32) -> Result<(Vec<User>, i64), Error> {
        let mut base = QueryBuilder::<Postgres>::new("SELECT u.id, u.username, u.password_hash, u.first_name, u.last_name, u.email, u.is_admin, u.enabled, u.created_at, u.updated_at, u.last_login FROM users u");
        if team_id.is_some() {
            base.push(" JOIN user_teams ut ON ut.user_id = u.id");
        }
        base.push(" WHERE 1=1");
        if let Some(tid) = team_id {
            base.push(" AND ut.team_id = ").push_bind(tid);
        }
        if let Some(n) = name.clone() {
            let pattern = format!("%{}%", n);
            base.push(" AND (u.first_name ILIKE ").push_bind(pattern.clone())
                .push(" OR u.last_name ILIKE ").push_bind(pattern.clone())
                .push(" OR u.username ILIKE ").push_bind(pattern)
                .push(")");
        }
        // Pagination
        let page = if page_number < 1 { 1 } else { page_number } as i64;
        let size = if page_size < 1 { 10 } else { page_size } as i64;
        let offset = (page - 1) * size;
        base.push(" ORDER BY u.username ASC LIMIT ").push_bind(size).push(" OFFSET ").push_bind(offset);

        let rows = handle_error(None, base.build().fetch_all(&self.pool).await)?;
        let mut users: Vec<User> = Vec::with_capacity(rows.len());
        for row in rows {
            users.push(User {
                id: row.get::<Uuid, _>(0),
                username: row.get::<String, _>(1),
                password_hash: row.get::<String, _>(2),
                first_name: row.get::<String, _>(3),
                last_name: row.get::<String, _>(4),
                email: row.get::<String, _>(5),
                is_admin: row.get::<bool, _>(6),
                enabled: row.get::<bool, _>(7),
                created_at: row.get::<DateTime<Utc>, _>(8),
                updated_at: row.get::<DateTime<Utc>, _>(9),
                last_login: row.try_get::<DateTime<Utc>, _>(10).ok(),
            });
        }

        // Total count
        let mut cnt = QueryBuilder::<Postgres>::new("SELECT COUNT(DISTINCT u.id) AS c FROM users u");
        if team_id.is_some() {
            cnt.push(" JOIN user_teams ut ON ut.user_id = u.id");
        }
        cnt.push(" WHERE 1=1");
        if let Some(tid) = team_id {
            cnt.push(" AND ut.team_id = ").push_bind(tid);
        }
        if let Some(n) = name {
            let pattern = format!("%{}%", n);
            cnt.push(" AND (u.first_name ILIKE ").push_bind(pattern.clone())
                .push(" OR u.last_name ILIKE ").push_bind(pattern.clone())
                .push(" OR u.username ILIKE ").push_bind(pattern)
                .push(")");
        }
        let row = handle_error(None, cnt.build().fetch_one(&self.pool).await)?;
        let total: i64 = row.get::<i64, _>(0);

        Ok((users, total))
    }

    async fn get_user_teams(&self, id: Uuid) -> Result<Vec<Team>, Error> {
        let result = sqlx::query_as::<_, Team>(
            r#"SELECT t.id, t.name, t.description
               FROM teams t
               INNER JOIN user_teams ut ON ut.team_id = t.id
               WHERE ut.user_id = $1
               ORDER BY t.name"#
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await;
        let teams = handle_error(Some(id), result)?;
        Ok(teams)
    }

    fn clone_box(&self) -> Box<dyn UserRepository> {
        Box::new(self.clone())
    }
}
