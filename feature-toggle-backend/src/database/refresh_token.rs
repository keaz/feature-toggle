//! Persistence for opaque, rotating refresh tokens.
//!
//! Only the SHA-256 hash of a refresh token is stored. Every token issued from
//! one login shares a `family_id`; rotating a token revokes it and records the
//! token that replaced it.

use crate::Error;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub replaced_by: Option<Uuid>,
}

#[automock]
#[async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    /// Stores a new refresh token (by hash) in the given family.
    async fn create_token(
        &self,
        user_id: Uuid,
        family_id: Uuid,
        token_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<RefreshToken, Error>;
    /// Revokes every token in the family of the token with this hash, provided
    /// that token belongs to `user_id`. Returns the number of tokens revoked.
    async fn revoke_family_by_hash(&self, user_id: Uuid, token_hash: &str) -> Result<u64, Error>;
    /// Deletes refresh tokens that expired more than one day ago.
    async fn delete_expired(&self) -> Result<u64, Error>;
    fn clone_box(&self) -> Box<dyn RefreshTokenRepository>;
}

impl Clone for Box<dyn RefreshTokenRepository> {
    fn clone(&self) -> Box<dyn RefreshTokenRepository> {
        self.clone_box()
    }
}

pub fn refresh_token_repository(pool: PgPool) -> Box<dyn RefreshTokenRepository> {
    Box::new(RefreshTokenRepositoryImpl { pool })
}

/// Inserts a refresh token on the given connection.
pub async fn create_token_tx(
    conn: &mut PgConnection,
    user_id: Uuid,
    family_id: Uuid,
    token_hash: String,
    expires_at: DateTime<Utc>,
) -> Result<RefreshToken, Error> {
    sqlx::query_as!(
        RefreshToken,
        r#"
        INSERT INTO refresh_tokens (id, user_id, family_id, token_hash, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, user_id, family_id, token_hash, expires_at, created_at, revoked_at, replaced_by
        "#,
        Uuid::new_v4(),
        user_id,
        family_id,
        token_hash,
        expires_at
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(Error::DatabaseError)
}

/// Loads a refresh token by hash and locks its row until the transaction ends,
/// so concurrent rotations of the same token are serialized.
pub async fn find_by_hash_for_update_tx(
    conn: &mut PgConnection,
    token_hash: &str,
) -> Result<Option<RefreshToken>, Error> {
    sqlx::query_as!(
        RefreshToken,
        r#"
        SELECT id, user_id, family_id, token_hash, expires_at, created_at, revoked_at, replaced_by
        FROM refresh_tokens
        WHERE token_hash = $1
        FOR UPDATE
        "#,
        token_hash
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(Error::DatabaseError)
}

/// Revokes a token as rotated into `replaced_by`. Only an unrevoked token is
/// updated, so returns `false` if the token was already revoked.
pub async fn mark_rotated_tx(
    conn: &mut PgConnection,
    id: Uuid,
    replaced_by: Uuid,
) -> Result<bool, Error> {
    let result = sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = now(), replaced_by = $2
        WHERE id = $1 AND revoked_at IS NULL
        "#,
        id,
        replaced_by
    )
    .execute(&mut *conn)
    .await
    .map_err(Error::DatabaseError)?;

    Ok(result.rows_affected() == 1)
}

/// Whether the family still has an unrevoked, unexpired token, i.e. it was not
/// ended by logout, disable, password reset or reuse detection.
pub async fn family_has_active_token_tx(
    conn: &mut PgConnection,
    family_id: Uuid,
) -> Result<bool, Error> {
    let active = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM refresh_tokens
            WHERE family_id = $1 AND revoked_at IS NULL AND expires_at > now()
        ) AS "active!"
        "#,
        family_id
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(Error::DatabaseError)?;

    Ok(active)
}

/// Revokes every still-active token of a family.
pub async fn revoke_family_tx(conn: &mut PgConnection, family_id: Uuid) -> Result<u64, Error> {
    let result = sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = now()
        WHERE family_id = $1 AND revoked_at IS NULL
        "#,
        family_id
    )
    .execute(&mut *conn)
    .await
    .map_err(Error::DatabaseError)?;

    Ok(result.rows_affected())
}

/// Revokes every still-active refresh token of a user.
pub async fn revoke_all_user_refresh_tokens_tx(
    conn: &mut PgConnection,
    user_id: Uuid,
) -> Result<u64, Error> {
    let result = sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = now()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
        user_id
    )
    .execute(&mut *conn)
    .await
    .map_err(Error::DatabaseError)?;

    Ok(result.rows_affected())
}

/// Revokes every still-active refresh token of every user (emergency JWT
/// secret deactivation).
pub async fn revoke_all_refresh_tokens_tx(conn: &mut PgConnection) -> Result<u64, Error> {
    let result = sqlx::query!(
        r#"
        UPDATE refresh_tokens
        SET revoked_at = now()
        WHERE revoked_at IS NULL
        "#
    )
    .execute(&mut *conn)
    .await
    .map_err(Error::DatabaseError)?;

    Ok(result.rows_affected())
}

#[derive(Clone)]
struct RefreshTokenRepositoryImpl {
    pool: PgPool,
}

#[async_trait]
impl RefreshTokenRepository for RefreshTokenRepositoryImpl {
    async fn create_token(
        &self,
        user_id: Uuid,
        family_id: Uuid,
        token_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<RefreshToken, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        create_token_tx(&mut conn, user_id, family_id, token_hash, expires_at).await
    }

    async fn revoke_family_by_hash(&self, user_id: Uuid, token_hash: &str) -> Result<u64, Error> {
        let result = sqlx::query!(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = now()
            WHERE revoked_at IS NULL
            AND family_id = (
                SELECT family_id FROM refresh_tokens
                WHERE token_hash = $1 AND user_id = $2
            )
            "#,
            token_hash,
            user_id
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected())
    }

    async fn delete_expired(&self) -> Result<u64, Error> {
        let result = sqlx::query!(
            r#"
            DELETE FROM refresh_tokens
            WHERE expires_at < now() - INTERVAL '1 day'
            "#
        )
        .execute(&self.pool)
        .await
        .map_err(Error::DatabaseError)?;

        Ok(result.rows_affected())
    }

    fn clone_box(&self) -> Box<dyn RefreshTokenRepository> {
        Box::new(self.clone())
    }
}
