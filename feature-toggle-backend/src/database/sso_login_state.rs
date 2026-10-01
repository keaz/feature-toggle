//! Persistence for in-flight OIDC authorization requests (state, nonce, PKCE
//! verifier). Rows are single use and expire after 10 minutes.

use crate::Error;
use crate::database::entity::SsoLoginState;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewSsoLoginState {
    /// SHA-256 hash of the `state` parameter.
    pub state_hash: String,
    pub provider_id: Uuid,
    pub nonce: String,
    pub pkce_verifier: String,
    pub redirect_path: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[automock]
#[async_trait::async_trait]
pub trait SsoLoginStateRepository: Send + Sync {
    async fn create_state(&self, input: NewSsoLoginState) -> Result<SsoLoginState, Error>;
    /// Atomically deletes and returns the unexpired state with this hash. A second
    /// call, or a call after expiry, returns `None`.
    async fn consume_state(&self, state_hash: &str) -> Result<Option<SsoLoginState>, Error>;
    /// Deletes states past their expiry and returns how many were removed.
    async fn delete_expired_states(&self) -> Result<u64, Error>;
    fn clone_box(&self) -> Box<dyn SsoLoginStateRepository>;
}

impl Clone for Box<dyn SsoLoginStateRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait SsoLoginStateRepositoryTx: SsoLoginStateRepository {
    async fn create_state_tx(
        &self,
        conn: &mut PgConnection,
        input: NewSsoLoginState,
    ) -> Result<SsoLoginState, Error>;
    async fn consume_state_tx(
        &self,
        conn: &mut PgConnection,
        state_hash: &str,
    ) -> Result<Option<SsoLoginState>, Error>;
}

pub fn sso_login_state_repository(pool: PgPool) -> Box<dyn SsoLoginStateRepository> {
    Box::new(SsoLoginStateRepositoryImpl { pool })
}

pub fn sso_login_state_repository_tx(pool: PgPool) -> SsoLoginStateRepositoryImpl {
    SsoLoginStateRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct SsoLoginStateRepositoryImpl {
    pool: PgPool,
}

impl SsoLoginStateRepositoryImpl {
    async fn insert(
        conn: &mut PgConnection,
        input: NewSsoLoginState,
    ) -> Result<SsoLoginState, Error> {
        sqlx::query_as!(
            SsoLoginState,
            r#"
            INSERT INTO sso_login_states (
                id, state_hash, provider_id, nonce, pkce_verifier, redirect_path, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, state_hash, provider_id, nonce, pkce_verifier, redirect_path,
                      expires_at, created_at
            "#,
            Uuid::new_v4(),
            input.state_hash,
            input.provider_id,
            input.nonce,
            input.pkce_verifier,
            input.redirect_path,
            input.expires_at
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn consume(
        conn: &mut PgConnection,
        state_hash: &str,
    ) -> Result<Option<SsoLoginState>, Error> {
        sqlx::query_as!(
            SsoLoginState,
            r#"
            DELETE FROM sso_login_states
            WHERE state_hash = $1 AND expires_at > now()
            RETURNING id, state_hash, provider_id, nonce, pkce_verifier, redirect_path,
                      expires_at, created_at
            "#,
            state_hash
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }
}

#[async_trait::async_trait]
impl SsoLoginStateRepository for SsoLoginStateRepositoryImpl {
    async fn create_state(&self, input: NewSsoLoginState) -> Result<SsoLoginState, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::insert(&mut conn, input).await
    }

    async fn consume_state(&self, state_hash: &str) -> Result<Option<SsoLoginState>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::consume(&mut conn, state_hash).await
    }

    async fn delete_expired_states(&self) -> Result<u64, Error> {
        let result = sqlx::query!("DELETE FROM sso_login_states WHERE expires_at <= now()")
            .execute(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;
        Ok(result.rows_affected())
    }

    fn clone_box(&self) -> Box<dyn SsoLoginStateRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl SsoLoginStateRepositoryTx for SsoLoginStateRepositoryImpl {
    async fn create_state_tx(
        &self,
        conn: &mut PgConnection,
        input: NewSsoLoginState,
    ) -> Result<SsoLoginState, Error> {
        Self::insert(conn, input).await
    }

    async fn consume_state_tx(
        &self,
        conn: &mut PgConnection,
        state_hash: &str,
    ) -> Result<Option<SsoLoginState>, Error> {
        Self::consume(conn, state_hash).await
    }
}
