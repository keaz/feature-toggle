//! Persistence for one-time SSO exchange codes. Only the SHA-256 hash of a code
//! is stored; each code is bound to a user and provider, lives 60 seconds and
//! can be redeemed once.

use crate::Error;
use crate::database::entity::SsoLoginCode;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewSsoLoginCode {
    pub code_hash: String,
    pub user_id: Uuid,
    pub provider_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

#[automock]
#[async_trait::async_trait]
pub trait SsoLoginCodeRepository: Send + Sync {
    async fn create_code(&self, input: NewSsoLoginCode) -> Result<SsoLoginCode, Error>;
    /// Atomically marks the unused, unexpired code with this hash as used and
    /// returns it. A second call, or a call after expiry, returns `None`.
    async fn consume_code(&self, code_hash: &str) -> Result<Option<SsoLoginCode>, Error>;
    /// Deletes codes that expired more than one day ago.
    async fn delete_expired_codes(&self) -> Result<u64, Error>;
    fn clone_box(&self) -> Box<dyn SsoLoginCodeRepository>;
}

impl Clone for Box<dyn SsoLoginCodeRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait SsoLoginCodeRepositoryTx: SsoLoginCodeRepository {
    async fn create_code_tx(
        &self,
        conn: &mut PgConnection,
        input: NewSsoLoginCode,
    ) -> Result<SsoLoginCode, Error>;
    async fn consume_code_tx(
        &self,
        conn: &mut PgConnection,
        code_hash: &str,
    ) -> Result<Option<SsoLoginCode>, Error>;
}

pub fn sso_login_code_repository(pool: PgPool) -> Box<dyn SsoLoginCodeRepository> {
    Box::new(SsoLoginCodeRepositoryImpl { pool })
}

pub fn sso_login_code_repository_tx(pool: PgPool) -> SsoLoginCodeRepositoryImpl {
    SsoLoginCodeRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct SsoLoginCodeRepositoryImpl {
    pool: PgPool,
}

impl SsoLoginCodeRepositoryImpl {
    async fn insert(
        conn: &mut PgConnection,
        input: NewSsoLoginCode,
    ) -> Result<SsoLoginCode, Error> {
        sqlx::query_as!(
            SsoLoginCode,
            r#"
            INSERT INTO sso_login_codes (id, code_hash, user_id, provider_id, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, code_hash, user_id, provider_id, expires_at, used_at, created_at
            "#,
            Uuid::new_v4(),
            input.code_hash,
            input.user_id,
            input.provider_id,
            input.expires_at
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn consume(
        conn: &mut PgConnection,
        code_hash: &str,
    ) -> Result<Option<SsoLoginCode>, Error> {
        sqlx::query_as!(
            SsoLoginCode,
            r#"
            UPDATE sso_login_codes
            SET used_at = now()
            WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()
            RETURNING id, code_hash, user_id, provider_id, expires_at, used_at, created_at
            "#,
            code_hash
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }
}

#[async_trait::async_trait]
impl SsoLoginCodeRepository for SsoLoginCodeRepositoryImpl {
    async fn create_code(&self, input: NewSsoLoginCode) -> Result<SsoLoginCode, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::insert(&mut conn, input).await
    }

    async fn consume_code(&self, code_hash: &str) -> Result<Option<SsoLoginCode>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::consume(&mut conn, code_hash).await
    }

    async fn delete_expired_codes(&self) -> Result<u64, Error> {
        let result =
            sqlx::query!("DELETE FROM sso_login_codes WHERE expires_at < now() - INTERVAL '1 day'")
                .execute(&self.pool)
                .await
                .map_err(Error::DatabaseError)?;
        Ok(result.rows_affected())
    }

    fn clone_box(&self) -> Box<dyn SsoLoginCodeRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl SsoLoginCodeRepositoryTx for SsoLoginCodeRepositoryImpl {
    async fn create_code_tx(
        &self,
        conn: &mut PgConnection,
        input: NewSsoLoginCode,
    ) -> Result<SsoLoginCode, Error> {
        Self::insert(conn, input).await
    }

    async fn consume_code_tx(
        &self,
        conn: &mut PgConnection,
        code_hash: &str,
    ) -> Result<Option<SsoLoginCode>, Error> {
        Self::consume(conn, code_hash).await
    }
}
