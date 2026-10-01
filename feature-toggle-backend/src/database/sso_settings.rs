//! Persistence for the singleton SSO settings row.

use crate::Error;
use mockall::automock;
use sqlx::{PgConnection, PgPool};

#[automock]
#[async_trait::async_trait]
pub trait SsoSettingsRepository: Send + Sync {
    async fn get_enforce_sso(&self) -> Result<bool, Error>;
    fn clone_box(&self) -> Box<dyn SsoSettingsRepository>;
}

impl Clone for Box<dyn SsoSettingsRepository> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[async_trait::async_trait]
pub trait SsoSettingsRepositoryTx: SsoSettingsRepository {
    async fn get_enforce_sso_tx(&self, conn: &mut PgConnection) -> Result<bool, Error>;
    async fn set_enforce_sso_tx(
        &self,
        conn: &mut PgConnection,
        enforce: bool,
    ) -> Result<bool, Error>;
}

pub fn sso_settings_repository(pool: PgPool) -> Box<dyn SsoSettingsRepository> {
    Box::new(SsoSettingsRepositoryImpl { pool })
}

pub fn sso_settings_repository_tx(pool: PgPool) -> SsoSettingsRepositoryImpl {
    SsoSettingsRepositoryImpl { pool }
}

#[derive(Clone)]
pub struct SsoSettingsRepositoryImpl {
    pool: PgPool,
}

impl SsoSettingsRepositoryImpl {
    async fn get(conn: &mut PgConnection) -> Result<bool, Error> {
        // A missing row (it is seeded by the migration) reads as "not enforced".
        let value = sqlx::query_scalar!("SELECT enforce_sso FROM sso_settings WHERE id")
            .fetch_optional(&mut *conn)
            .await
            .map_err(Error::DatabaseError)?;
        Ok(value.unwrap_or(false))
    }
}

#[async_trait::async_trait]
impl SsoSettingsRepository for SsoSettingsRepositoryImpl {
    async fn get_enforce_sso(&self) -> Result<bool, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::get(&mut conn).await
    }

    fn clone_box(&self) -> Box<dyn SsoSettingsRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl SsoSettingsRepositoryTx for SsoSettingsRepositoryImpl {
    async fn get_enforce_sso_tx(&self, conn: &mut PgConnection) -> Result<bool, Error> {
        Self::get(conn).await
    }

    async fn set_enforce_sso_tx(
        &self,
        conn: &mut PgConnection,
        enforce: bool,
    ) -> Result<bool, Error> {
        sqlx::query_scalar!(
            r#"
            INSERT INTO sso_settings (id, enforce_sso) VALUES (TRUE, $1)
            ON CONFLICT (id) DO UPDATE SET enforce_sso = EXCLUDED.enforce_sso
            RETURNING enforce_sso
            "#,
            enforce
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(Error::DatabaseError)
    }
}
