use crate::database::handle_error;
use crate::database::Error;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
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
            r#"SELECT id, username, password_hash, first_name, last_name, email, is_admin,
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
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, is_admin,
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
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn get_user_by_email(&self, email: &str) -> Result<User, Error> {
        let result = sqlx::query!(
            r#"SELECT id, username, password_hash, first_name, last_name, email, is_admin,
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
               RETURNING id, username, password_hash, first_name, last_name, email, is_admin, created_at, updated_at, last_login"#,
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
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_login: row.last_login,
        })
    }

    async fn update_user(&self, input: UpdateUser) -> Result<User, Error> {
        let existing = self.get_user_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE users
               SET first_name = $1, last_name = $2, email = $3, is_admin = $4, updated_at = now()
               WHERE id = $5
               RETURNING id, username, password_hash, first_name, last_name, email, is_admin, created_at, updated_at, last_login"#,
            input.first_name.unwrap_or(existing.first_name),
            input.last_name.unwrap_or(existing.last_name),
            input.email.unwrap_or(existing.email),
            input.is_admin.unwrap_or(existing.is_admin),
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

    fn clone_box(&self) -> Box<dyn UserRepository> {
        Box::new(self.clone())
    }
}
