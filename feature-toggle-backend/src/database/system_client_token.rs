use crate::Error;
use crate::database::entity::SystemClientToken;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[automock]
#[async_trait]
pub trait SystemClientTokenRepository: Send + Sync {
    async fn store_token(
        &self,
        system_client_id: Uuid,
        token_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<SystemClientToken, Error>;
    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_all_tokens_for_client(&self, system_client_id: Uuid) -> Result<u64, Error>;
    async fn cleanup_expired_tokens(&self) -> Result<u64, Error>;
    fn clone_box(&self) -> Box<dyn SystemClientTokenRepository>;
}

impl Clone for Box<dyn SystemClientTokenRepository> {
    fn clone(&self) -> Box<dyn SystemClientTokenRepository> {
        self.clone_box()
    }
}

pub fn system_client_token_repository(pool: PgPool) -> Box<dyn SystemClientTokenRepository> {
    Box::new(SystemClientTokenRepositoryImpl { pool })
}

#[derive(Clone)]
struct SystemClientTokenRepositoryImpl {
    pool: PgPool,
}

#[async_trait]
impl SystemClientTokenRepository for SystemClientTokenRepositoryImpl {
    async fn store_token(
        &self,
        system_client_id: Uuid,
        token_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<SystemClientToken, Error> {
        let row = sqlx::query(
            r#"
            INSERT INTO system_client_tokens (system_client_id, token_hash, expires_at)
            VALUES ($1, $2, $3)
            RETURNING id, system_client_id, token_hash, expires_at, created_at, revoked_at, is_revoked
            "#,
        )
        .bind(system_client_id)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(SystemClientToken {
            id: row.get("id"),
            system_client_id: row.get("system_client_id"),
            token_hash: row.get("token_hash"),
            expires_at: row.get("expires_at"),
            created_at: row.get("created_at"),
            revoked_at: row.get("revoked_at"),
            is_revoked: row.get("is_revoked"),
        })
    }

    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)::BIGINT AS count
            FROM system_client_tokens
            WHERE token_hash = $1
              AND is_revoked = FALSE
              AND expires_at > CURRENT_TIMESTAMP
            "#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        let count: i64 = row.get("count");
        Ok(count > 0)
    }

    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"
            UPDATE system_client_tokens
            SET is_revoked = TRUE, revoked_at = CURRENT_TIMESTAMP
            WHERE token_hash = $1 AND is_revoked = FALSE
            "#,
        )
        .bind(token_hash)
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected() > 0)
    }

    async fn revoke_all_tokens_for_client(&self, system_client_id: Uuid) -> Result<u64, Error> {
        let result = sqlx::query(
            r#"
            UPDATE system_client_tokens
            SET is_revoked = TRUE, revoked_at = CURRENT_TIMESTAMP
            WHERE system_client_id = $1 AND is_revoked = FALSE
            "#,
        )
        .bind(system_client_id)
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected())
    }

    async fn cleanup_expired_tokens(&self) -> Result<u64, Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM system_client_tokens
            WHERE expires_at < CURRENT_TIMESTAMP
               OR (is_revoked = TRUE AND revoked_at < CURRENT_TIMESTAMP - INTERVAL '7 days')
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected())
    }

    fn clone_box(&self) -> Box<dyn SystemClientTokenRepository> {
        Box::new(self.clone())
    }
}
