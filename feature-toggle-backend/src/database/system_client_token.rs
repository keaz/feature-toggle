use crate::Error;
use crate::database::entity::SystemClientToken;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

#[automock]
#[async_trait]
pub trait SystemClientTokenRepository: Send + Sync {
    async fn store_token(
        &self,
        system_client_id: Uuid,
        token_hash: String,
        name: String,
        scopes: Vec<String>,
        expires_at: DateTime<Utc>,
    ) -> Result<SystemClientToken, Error>;
    async fn get_token_by_hash(&self, token_hash: &str)
    -> Result<Option<SystemClientToken>, Error>;
    async fn list_tokens(&self, system_client_id: Uuid) -> Result<Vec<SystemClientToken>, Error>;
    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_token_by_id(&self, id: Uuid) -> Result<bool, Error>;
    async fn revoke_all_tokens_for_client(&self, system_client_id: Uuid) -> Result<u64, Error>;
    async fn touch_token_last_used(&self, token_hash: &str) -> Result<(), Error>;
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
        name: String,
        scopes: Vec<String>,
        expires_at: DateTime<Utc>,
    ) -> Result<SystemClientToken, Error> {
        sqlx::query_as::<_, SystemClientToken>(
            r#"
            INSERT INTO system_client_tokens (system_client_id, token_hash, name, scopes, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, system_client_id, token_hash, name, scopes, expires_at,
                      created_at, revoked_at, is_revoked, last_used_at
            "#,
        )
        .bind(system_client_id)
        .bind(token_hash)
        .bind(name)
        .bind(scopes)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn get_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<SystemClientToken>, Error> {
        sqlx::query_as::<_, SystemClientToken>(
            r#"
            SELECT id, system_client_id, token_hash, name, scopes, expires_at,
                   created_at, revoked_at, is_revoked, last_used_at
            FROM system_client_tokens
            WHERE token_hash = $1
            "#,
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn list_tokens(&self, system_client_id: Uuid) -> Result<Vec<SystemClientToken>, Error> {
        sqlx::query_as::<_, SystemClientToken>(
            r#"
            SELECT id, system_client_id, token_hash, name, scopes, expires_at,
                   created_at, revoked_at, is_revoked, last_used_at
            FROM system_client_tokens
            WHERE system_client_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(system_client_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)
    }

    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error> {
        Ok(self
            .get_token_by_hash(token_hash)
            .await?
            .map(|token| !token.is_revoked && token.expires_at > Utc::now())
            .unwrap_or(false))
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

    async fn revoke_token_by_id(&self, id: Uuid) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"
            UPDATE system_client_tokens
            SET is_revoked = TRUE, revoked_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND is_revoked = FALSE
            "#,
        )
        .bind(id)
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

    async fn touch_token_last_used(&self, token_hash: &str) -> Result<(), Error> {
        sqlx::query(
            r#"
            UPDATE system_client_tokens
            SET last_used_at = CURRENT_TIMESTAMP
            WHERE token_hash = $1
            "#,
        )
        .bind(token_hash)
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(())
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
