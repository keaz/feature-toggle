use crate::Error;
use crate::database::entity::JwtSecret;
use crate::database::handle_error;
use mockall::automock;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait JwtSecretRepository: Send + Sync {
    /// Get the currently active JWT secret
    async fn get_active_secret(&self) -> Result<Option<JwtSecret>, Error>;

    /// Get a secret by id (the `kid` header of a token), active or not.
    async fn get_secret_by_id(&self, id: Uuid) -> Result<Option<JwtSecret>, Error>;

    /// Create a new JWT secret and set it as active (deactivates all others)
    async fn create_secret(
        &self,
        secret: String,
        created_by: Option<Uuid>,
    ) -> Result<JwtSecret, Error>;

    /// Generate and store a new random secret
    async fn generate_new_secret(&self, created_by: Option<Uuid>) -> Result<JwtSecret, Error>;

    /// Get all secrets (for admin purposes)
    async fn get_all_secrets(&self) -> Result<Vec<JwtSecret>, Error>;

    /// Get database pool for advanced operations (like advisory locks)
    fn pool(&self) -> &PgPool;

    fn clone_box(&self) -> Box<dyn JwtSecretRepository>;
}

impl Clone for Box<dyn JwtSecretRepository> {
    fn clone(&self) -> Box<dyn JwtSecretRepository> {
        self.clone_box()
    }
}

/// Rotates the signing secret on the given connection: the active secret is
/// deactivated with `deactivated_at = now()` (it keeps verifying the tokens it
/// signed during the rotation grace window) and `secret` becomes the active one.
pub async fn create_secret_tx(
    conn: &mut PgConnection,
    secret: String,
    created_by: Option<Uuid>,
) -> Result<JwtSecret, Error> {
    handle_error(
        None,
        sqlx::query!(
            "UPDATE jwt_secrets SET is_active = false, deactivated_at = now() WHERE is_active = true"
        )
        .execute(&mut *conn)
        .await,
    )?;

    let result = sqlx::query_as!(
        JwtSecret,
        r#"INSERT INTO jwt_secrets (secret, is_active, created_by)
           VALUES ($1, true, $2)
           RETURNING id, secret, is_active, created_at, created_by, expires_at,
                     deactivated_at, revoked_at"#,
        secret,
        created_by
    )
    .fetch_one(&mut *conn)
    .await;

    handle_error(None, result)
}

/// Loads the active, unrevoked signing secret and locks its row `FOR SHARE`
/// until the transaction ends. Call it before writing any session token: an
/// emergency deactivation updates `jwt_secrets` first, so it waits for this
/// transaction, and its later revocations of `jwt_tokens` / `refresh_tokens`
/// then see the rows written here. Returns `None` when no secret is usable.
///
/// A rotation committing while this waits leaves the first read empty (the old
/// row is no longer active and the new row is not in that statement's
/// snapshot), so the read is retried once with a fresh snapshot.
pub async fn lock_active_signing_secret_tx(
    conn: &mut PgConnection,
) -> Result<Option<JwtSecret>, Error> {
    for _ in 0..2 {
        let result = sqlx::query_as!(
            JwtSecret,
            r#"SELECT id, secret, is_active, created_at, created_by, expires_at,
                      deactivated_at, revoked_at
               FROM jwt_secrets
               WHERE is_active = true AND revoked_at IS NULL
               ORDER BY created_at DESC
               LIMIT 1
               FOR SHARE"#
        )
        .fetch_optional(&mut *conn)
        .await;
        if let Some(secret) = handle_error(None, result)? {
            return Ok(Some(secret));
        }
    }
    Ok(None)
}

/// Like [`lock_active_signing_secret_tx`] for a known secret id: locks the
/// secret `FOR SHARE` and returns whether it is still active and unrevoked.
pub async fn lock_signing_secret_tx(conn: &mut PgConnection, id: Uuid) -> Result<bool, Error> {
    let result = sqlx::query_scalar!(
        r#"SELECT id FROM jwt_secrets
           WHERE id = $1 AND is_active = true AND revoked_at IS NULL
           FOR SHARE"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await;
    Ok(handle_error(None, result)?.is_some())
}

/// Emergency deactivation on the given connection: every secret is deactivated
/// and revoked, so no token signed by any of them verifies, with no rotation
/// grace. Returns the number of secrets revoked.
pub async fn revoke_all_secrets_tx(conn: &mut PgConnection) -> Result<u64, Error> {
    let result = handle_error(
        None,
        sqlx::query!(
            r#"UPDATE jwt_secrets
               SET is_active = false,
                   deactivated_at = COALESCE(deactivated_at, now()),
                   revoked_at = now()
               WHERE revoked_at IS NULL"#
        )
        .execute(&mut *conn)
        .await,
    )?;
    Ok(result.rows_affected())
}

pub struct JwtSecretRepositoryImpl {
    pool: PgPool,
}

impl JwtSecretRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl JwtSecretRepository for JwtSecretRepositoryImpl {
    async fn get_active_secret(&self) -> Result<Option<JwtSecret>, Error> {
        let result = sqlx::query_as!(
            JwtSecret,
            r#"SELECT id, secret, is_active, created_at, created_by, expires_at,
                      deactivated_at, revoked_at
               FROM jwt_secrets 
               WHERE is_active = true
               ORDER BY created_at DESC
               LIMIT 1
               -- Use FOR SHARE to ensure consistent reads in multi-instance deployments
               FOR SHARE"#
        )
        .fetch_optional(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn get_secret_by_id(&self, id: Uuid) -> Result<Option<JwtSecret>, Error> {
        let result = sqlx::query_as!(
            JwtSecret,
            r#"SELECT id, secret, is_active, created_at, created_by, expires_at,
                      deactivated_at, revoked_at
               FROM jwt_secrets
               WHERE id = $1"#,
            id
        )
        .fetch_optional(&self.pool)
        .await;

        handle_error(None, result)
    }

    async fn create_secret(
        &self,
        secret: String,
        created_by: Option<Uuid>,
    ) -> Result<JwtSecret, Error> {
        let mut tx = self.pool.begin().await.map_err(Error::DatabaseError)?;
        let jwt_secret = create_secret_tx(&mut tx, secret, created_by).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;

        Ok(jwt_secret)
    }

    async fn generate_new_secret(&self, created_by: Option<Uuid>) -> Result<JwtSecret, Error> {
        // Generate a secure random secret (32 bytes = 256 bits)
        let secret = generate_secure_secret();
        self.create_secret(secret, created_by).await
    }

    async fn get_all_secrets(&self) -> Result<Vec<JwtSecret>, Error> {
        let result = sqlx::query_as!(
            JwtSecret,
            r#"SELECT id, secret, is_active, created_at, created_by, expires_at,
                      deactivated_at, revoked_at
               FROM jwt_secrets 
               ORDER BY created_at DESC"#
        )
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn clone_box(&self) -> Box<dyn JwtSecretRepository> {
        Box::new(Self {
            pool: self.pool.clone(),
        })
    }
}

/// Generate a cryptographically secure random secret for JWT signing
fn generate_secure_secret() -> String {
    use base64::{Engine, engine::general_purpose};
    use rand::Rng;
    let mut rng = rand::rng();
    let mut secret_bytes = [0u8; 32]; // 256 bits
    rng.fill(&mut secret_bytes);
    general_purpose::STANDARD.encode(secret_bytes)
}

pub fn jwt_secret_repository(pool: PgPool) -> Box<dyn JwtSecretRepository> {
    Box::new(JwtSecretRepositoryImpl::new(pool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn test_pool() -> PgPool {
        PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@localhost/test_db")
            .expect("Failed to create test pool")
    }

    #[test]
    fn test_generate_secure_secret() {
        use base64::{Engine, engine::general_purpose};
        let secret1 = generate_secure_secret();
        let secret2 = generate_secure_secret();

        // Secrets should be different
        assert_ne!(secret1, secret2);

        // Should be base64 encoded 32 bytes (44 characters including padding)
        assert_eq!(secret1.len(), 44);
        assert_eq!(secret2.len(), 44);

        // Should be valid base64
        assert!(general_purpose::STANDARD.decode(&secret1).is_ok());
        assert!(general_purpose::STANDARD.decode(&secret2).is_ok());
    }

    #[tokio::test]
    async fn test_jwt_secret_repository_creation() {
        let pool = test_pool();
        let repo = jwt_secret_repository(pool);

        // Just test that we can create the repository
        assert!(!format!("{:p}", repo.as_ref()).is_empty());
    }
}
