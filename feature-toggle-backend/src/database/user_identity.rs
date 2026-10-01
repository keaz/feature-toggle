//! Persistence for the link between a FluxGate user and an IdP account.
//! Identities are matched by `(provider_id, subject)` only.

use crate::Error;
use crate::database::entity::{UserIdentity, UserIdentityWithProvider};
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CreateUserIdentity {
    pub user_id: Uuid,
    pub provider_id: Uuid,
    pub subject: String,
    pub email: Option<String>,
    pub last_login: Option<DateTime<Utc>>,
}

#[automock]
#[async_trait::async_trait]
pub trait UserIdentityRepository: Send + Sync {
    async fn find_identity(
        &self,
        provider_id: Uuid,
        subject: &str,
    ) -> Result<Option<UserIdentity>, Error>;
    /// Identities of one user with their provider slug, ordered by slug.
    async fn list_identities_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<UserIdentityWithProvider>, Error>;
    /// Batch variant of `list_identities_for_user` for list endpoints.
    async fn list_identities_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<UserIdentityWithProvider>, Error>;
    fn clone_box(&self) -> Box<dyn UserIdentityRepository>;
}

impl Clone for Box<dyn UserIdentityRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait UserIdentityRepositoryTx: UserIdentityRepository {
    async fn find_identity_tx(
        &self,
        conn: &mut PgConnection,
        provider_id: Uuid,
        subject: &str,
    ) -> Result<Option<UserIdentity>, Error>;
    async fn create_identity_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateUserIdentity,
    ) -> Result<UserIdentity, Error>;
    /// Records a login: sets `last_login` and refreshes the email seen at the IdP.
    async fn record_login_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        email: Option<String>,
        when: DateTime<Utc>,
    ) -> Result<(), Error>;
}

pub fn user_identity_repository(pool: PgPool) -> Box<dyn UserIdentityRepository> {
    Box::new(UserIdentityRepositoryImpl { pool })
}

pub fn user_identity_repository_tx(pool: PgPool) -> UserIdentityRepositoryImpl {
    UserIdentityRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct UserIdentityRepositoryImpl {
    pool: PgPool,
}

fn map_error(err: sqlx::Error) -> Error {
    match &err {
        sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("23505") => {
            Error::RecordAlreadyExists("identity".to_string())
        }
        _ => Error::DatabaseError(err),
    }
}

impl UserIdentityRepositoryImpl {
    async fn find(
        conn: &mut PgConnection,
        provider_id: Uuid,
        subject: &str,
    ) -> Result<Option<UserIdentity>, Error> {
        sqlx::query_as!(
            UserIdentity,
            r#"
            SELECT id, user_id, provider_id, subject, email, last_login, created_at
            FROM user_identities
            WHERE provider_id = $1 AND subject = $2
            "#,
            provider_id,
            subject
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(map_error)
    }
}

#[async_trait::async_trait]
impl UserIdentityRepository for UserIdentityRepositoryImpl {
    async fn find_identity(
        &self,
        provider_id: Uuid,
        subject: &str,
    ) -> Result<Option<UserIdentity>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::find(&mut conn, provider_id, subject).await
    }

    async fn list_identities_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<UserIdentityWithProvider>, Error> {
        sqlx::query_as!(
            UserIdentityWithProvider,
            r#"
            SELECT ui.user_id, p.slug AS provider_slug, ui.subject, ui.email, ui.last_login
            FROM user_identities ui
            JOIN sso_providers p ON p.id = ui.provider_id
            WHERE ui.user_id = $1
            ORDER BY p.slug, ui.subject
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_error)
    }

    async fn list_identities_for_users(
        &self,
        user_ids: Vec<Uuid>,
    ) -> Result<Vec<UserIdentityWithProvider>, Error> {
        sqlx::query_as!(
            UserIdentityWithProvider,
            r#"
            SELECT ui.user_id, p.slug AS provider_slug, ui.subject, ui.email, ui.last_login
            FROM user_identities ui
            JOIN sso_providers p ON p.id = ui.provider_id
            WHERE ui.user_id = ANY($1)
            ORDER BY ui.user_id, p.slug, ui.subject
            "#,
            &user_ids
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_error)
    }

    fn clone_box(&self) -> Box<dyn UserIdentityRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl UserIdentityRepositoryTx for UserIdentityRepositoryImpl {
    async fn find_identity_tx(
        &self,
        conn: &mut PgConnection,
        provider_id: Uuid,
        subject: &str,
    ) -> Result<Option<UserIdentity>, Error> {
        Self::find(conn, provider_id, subject).await
    }

    async fn create_identity_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateUserIdentity,
    ) -> Result<UserIdentity, Error> {
        sqlx::query_as!(
            UserIdentity,
            r#"
            INSERT INTO user_identities (id, user_id, provider_id, subject, email, last_login)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, user_id, provider_id, subject, email, last_login, created_at
            "#,
            Uuid::new_v4(),
            input.user_id,
            input.provider_id,
            input.subject,
            input.email,
            input.last_login
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(map_error)
    }

    async fn record_login_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        email: Option<String>,
        when: DateTime<Utc>,
    ) -> Result<(), Error> {
        let result = sqlx::query!(
            "UPDATE user_identities SET last_login = $2, email = $3 WHERE id = $1",
            id,
            when,
            email
        )
        .execute(&mut *conn)
        .await
        .map_err(map_error)?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }
}
