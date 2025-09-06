use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use crate::Error;
use mockall::automock;

#[derive(Debug, Clone)]
pub struct JwtToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub is_revoked: bool,
}

#[automock]
#[async_trait]
pub trait JwtTokenRepository: Send + Sync {
    async fn store_token(&self, user_id: Uuid, token_hash: String, expires_at: DateTime<Utc>) -> Result<JwtToken, Error>;
    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_all_user_tokens(&self, user_id: Uuid) -> Result<u64, Error>;
    async fn cleanup_expired_tokens(&self) -> Result<u64, Error>;
    async fn get_user_active_tokens(&self, user_id: Uuid) -> Result<Vec<JwtToken>, Error>;
    fn clone_box(&self) -> Box<dyn JwtTokenRepository>;
}

impl Clone for Box<dyn JwtTokenRepository> {
    fn clone(&self) -> Box<dyn JwtTokenRepository> {
        self.clone_box()
    }
}

pub fn jwt_token_repository(pool: PgPool) -> Box<dyn JwtTokenRepository> {
    Box::new(JwtTokenRepositoryImpl { pool })
}

#[derive(Clone)]
struct JwtTokenRepositoryImpl {
    pool: PgPool,
}

#[async_trait]
impl JwtTokenRepository for JwtTokenRepositoryImpl {
    async fn store_token(&self, user_id: Uuid, token_hash: String, expires_at: DateTime<Utc>) -> Result<JwtToken, Error> {
        let token = sqlx::query_as!(
            JwtToken,
            r#"
            INSERT INTO jwt_tokens (user_id, token_hash, expires_at)
            VALUES ($1, $2, $3)
            RETURNING id, user_id, token_hash, expires_at, created_at, revoked_at, is_revoked
            "#,
            user_id,
            token_hash,
            expires_at
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(token)
    }

    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error> {
        let result = sqlx::query!(
            r#"
            SELECT COUNT(*) as count 
            FROM jwt_tokens 
            WHERE token_hash = $1 
            AND is_revoked = FALSE 
            AND expires_at > CURRENT_TIMESTAMP
            "#,
            token_hash
        )
        .fetch_one(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.count.unwrap_or(0) > 0)
    }

    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error> {
        let result = sqlx::query!(
            r#"
            UPDATE jwt_tokens 
            SET is_revoked = TRUE, revoked_at = CURRENT_TIMESTAMP
            WHERE token_hash = $1 AND is_revoked = FALSE
            "#,
            token_hash
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected() > 0)
    }

    async fn revoke_all_user_tokens(&self, user_id: Uuid) -> Result<u64, Error> {
        let result = sqlx::query!(
            r#"
            UPDATE jwt_tokens 
            SET is_revoked = TRUE, revoked_at = CURRENT_TIMESTAMP
            WHERE user_id = $1 AND is_revoked = FALSE
            "#,
            user_id
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected())
    }

    async fn cleanup_expired_tokens(&self) -> Result<u64, Error> {
        let result = sqlx::query!(
            r#"
            DELETE FROM jwt_tokens 
            WHERE expires_at < CURRENT_TIMESTAMP 
            OR (is_revoked = TRUE AND revoked_at < CURRENT_TIMESTAMP - INTERVAL '7 days')
            "#
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected())
    }

    async fn get_user_active_tokens(&self, user_id: Uuid) -> Result<Vec<JwtToken>, Error> {
        let tokens = sqlx::query_as!(
            JwtToken,
            r#"
            SELECT id, user_id, token_hash, expires_at, created_at, revoked_at, is_revoked
            FROM jwt_tokens 
            WHERE user_id = $1 
            AND is_revoked = FALSE 
            AND expires_at > CURRENT_TIMESTAMP
            ORDER BY created_at DESC
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(tokens)
    }

    fn clone_box(&self) -> Box<dyn JwtTokenRepository> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use mockall::mock;

    mock! {
        JwtTokenRepository {}

        #[async_trait]
        impl JwtTokenRepository for JwtTokenRepository {
            async fn store_token(&self, user_id: Uuid, token_hash: String, expires_at: DateTime<Utc>) -> Result<JwtToken, Error>;
            async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error>;
            async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error>;
            async fn revoke_all_user_tokens(&self, user_id: Uuid) -> Result<u64, Error>;
            async fn cleanup_expired_tokens(&self) -> Result<u64, Error>;
            async fn get_user_active_tokens(&self, user_id: Uuid) -> Result<Vec<JwtToken>, Error>;
            fn clone_box(&self) -> Box<dyn JwtTokenRepository>;
        }
    }

    fn sample_token() -> JwtToken {
        JwtToken {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            token_hash: "hash123".to_string(),
            expires_at: Utc::now() + Duration::hours(24),
            created_at: Utc::now(),
            revoked_at: None,
            is_revoked: false,
        }
    }

    #[tokio::test]
    async fn test_is_token_valid_returns_true_for_valid_token() {
        let mut mock = MockJwtTokenRepository::new();
        mock.expect_is_token_valid()
            .with(mockall::predicate::eq("valid_hash"))
            .times(1)
            .returning(|_| Ok(true));

        let result = mock.is_token_valid("valid_hash").await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_is_token_valid_returns_false_for_invalid_token() {
        let mut mock = MockJwtTokenRepository::new();
        mock.expect_is_token_valid()
            .with(mockall::predicate::eq("invalid_hash"))
            .times(1)
            .returning(|_| Ok(false));

        let result = mock.is_token_valid("invalid_hash").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_revoke_token_returns_true_when_successful() {
        let mut mock = MockJwtTokenRepository::new();
        mock.expect_revoke_token()
            .with(mockall::predicate::eq("token_hash"))
            .times(1)
            .returning(|_| Ok(true));

        let result = mock.revoke_token("token_hash").await.unwrap();
        assert!(result);
    }
}
