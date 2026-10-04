//! Persistence for CLI device-code logins. Rows live 10 minutes; the device
//! code is stored as a SHA-256 hash and releases one session.

use crate::Error;
use crate::database::entity::CliDeviceAuthorization;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewCliDeviceAuthorization {
    pub device_code_hash: String,
    pub user_code: String,
    pub interval_secs: i32,
    pub expires_at: DateTime<Utc>,
}

#[automock]
#[async_trait::async_trait]
pub trait CliDeviceAuthorizationRepository: Send + Sync {
    /// Fails with a unique violation when the user code is taken.
    async fn create(
        &self,
        input: NewCliDeviceAuthorization,
    ) -> Result<CliDeviceAuthorization, Error>;
    async fn find_by_device_code_hash(
        &self,
        device_code_hash: &str,
    ) -> Result<Option<CliDeviceAuthorization>, Error>;
    async fn record_poll(&self, id: Uuid, at: DateTime<Utc>) -> Result<(), Error>;
    /// Approves or denies the pending, unexpired row with this user code.
    async fn decide(
        &self,
        user_code: &str,
        user_id: Uuid,
        approve: bool,
    ) -> Result<Option<CliDeviceAuthorization>, Error>;
    /// Marks an approved row consumed and returns it; a second call returns
    /// `None`, so one approval releases one session.
    async fn consume_approved(&self, id: Uuid) -> Result<Option<CliDeviceAuthorization>, Error>;
    /// Undoes [`Self::consume_approved`] when the session could not be issued.
    async fn restore_approved(&self, id: Uuid) -> Result<(), Error>;
    /// Deletes rows past their expiry and returns how many were removed.
    async fn delete_expired(&self) -> Result<u64, Error>;
    fn clone_box(&self) -> Box<dyn CliDeviceAuthorizationRepository>;
}

impl Clone for Box<dyn CliDeviceAuthorizationRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait CliDeviceAuthorizationRepositoryTx: CliDeviceAuthorizationRepository {
    /// [`CliDeviceAuthorizationRepository::decide`] inside the caller's
    /// transaction, so the activity entry commits with the decision.
    async fn decide_tx(
        &self,
        conn: &mut PgConnection,
        user_code: &str,
        user_id: Uuid,
        approve: bool,
    ) -> Result<Option<CliDeviceAuthorization>, Error>;
}

pub fn cli_device_authorization_repository(
    pool: PgPool,
) -> Box<dyn CliDeviceAuthorizationRepository> {
    Box::new(CliDeviceAuthorizationRepositoryImpl { pool })
}

pub fn cli_device_authorization_repository_tx(
    pool: PgPool,
) -> CliDeviceAuthorizationRepositoryImpl {
    CliDeviceAuthorizationRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct CliDeviceAuthorizationRepositoryImpl {
    pool: PgPool,
}

impl CliDeviceAuthorizationRepositoryImpl {
    async fn decide_on(
        conn: &mut PgConnection,
        user_code: &str,
        user_id: Uuid,
        approve: bool,
    ) -> Result<Option<CliDeviceAuthorization>, Error> {
        let status = if approve { "approved" } else { "denied" };
        sqlx::query_as!(
            CliDeviceAuthorization,
            r#"
            UPDATE cli_device_authorizations
            SET status = $3, user_id = $2
            WHERE user_code = $1 AND status = 'pending' AND expires_at > now()
            RETURNING id, device_code_hash, user_code, status, user_id, interval_secs,
                      last_polled_at, expires_at, created_at
            "#,
            user_code,
            user_id,
            status
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }
}

#[async_trait::async_trait]
impl CliDeviceAuthorizationRepository for CliDeviceAuthorizationRepositoryImpl {
    async fn create(
        &self,
        input: NewCliDeviceAuthorization,
    ) -> Result<CliDeviceAuthorization, Error> {
        sqlx::query_as!(
            CliDeviceAuthorization,
            r#"
            INSERT INTO cli_device_authorizations (id, device_code_hash, user_code, interval_secs, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, device_code_hash, user_code, status, user_id, interval_secs,
                      last_polled_at, expires_at, created_at
            "#,
            Uuid::new_v4(),
            input.device_code_hash,
            input.user_code,
            input.interval_secs,
            input.expires_at
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn find_by_device_code_hash(
        &self,
        device_code_hash: &str,
    ) -> Result<Option<CliDeviceAuthorization>, Error> {
        sqlx::query_as!(
            CliDeviceAuthorization,
            r#"
            SELECT id, device_code_hash, user_code, status, user_id, interval_secs,
                   last_polled_at, expires_at, created_at
            FROM cli_device_authorizations
            WHERE device_code_hash = $1
            "#,
            device_code_hash
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn record_poll(&self, id: Uuid, at: DateTime<Utc>) -> Result<(), Error> {
        sqlx::query!(
            "UPDATE cli_device_authorizations SET last_polled_at = $2 WHERE id = $1",
            id,
            at
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(())
    }

    async fn decide(
        &self,
        user_code: &str,
        user_id: Uuid,
        approve: bool,
    ) -> Result<Option<CliDeviceAuthorization>, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::decide_on(&mut conn, user_code, user_id, approve).await
    }

    async fn consume_approved(&self, id: Uuid) -> Result<Option<CliDeviceAuthorization>, Error> {
        sqlx::query_as!(
            CliDeviceAuthorization,
            r#"
            UPDATE cli_device_authorizations
            SET status = 'consumed'
            WHERE id = $1 AND status = 'approved' AND expires_at > now()
            RETURNING id, device_code_hash, user_code, status, user_id, interval_secs,
                      last_polled_at, expires_at, created_at
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn restore_approved(&self, id: Uuid) -> Result<(), Error> {
        sqlx::query!(
            "UPDATE cli_device_authorizations SET status = 'approved' WHERE id = $1 AND status = 'consumed'",
            id
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;
        Ok(())
    }

    async fn delete_expired(&self) -> Result<u64, Error> {
        let result =
            sqlx::query!("DELETE FROM cli_device_authorizations WHERE expires_at <= now()")
                .execute(&self.pool)
                .await
                .map_err(Error::DatabaseError)?;
        Ok(result.rows_affected())
    }

    fn clone_box(&self) -> Box<dyn CliDeviceAuthorizationRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl CliDeviceAuthorizationRepositoryTx for CliDeviceAuthorizationRepositoryImpl {
    async fn decide_tx(
        &self,
        conn: &mut PgConnection,
        user_code: &str,
        user_id: Uuid,
        approve: bool,
    ) -> Result<Option<CliDeviceAuthorization>, Error> {
        Self::decide_on(conn, user_code, user_id, approve).await
    }
}
