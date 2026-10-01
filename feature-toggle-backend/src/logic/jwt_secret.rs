use crate::Error;
use crate::config::AuthConfig;
use crate::database::entity::JwtSecret;
use crate::database::jwt_secret::{JwtSecretRepository, jwt_secret_repository};
use chrono::{DateTime, Duration, Utc};
use log::{error, info, warn};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

/// The active secret used to sign new tokens; `kid` goes into the JWT header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningKey {
    pub kid: Uuid,
    pub secret: String,
}

impl From<JwtSecret> for SigningKey {
    fn from(secret: JwtSecret) -> Self {
        Self {
            kid: secret.id,
            secret: secret.secret,
        }
    }
}

/// Whether `secret` may verify a token whose `kid` names it, at `now`.
///
/// The active secret verifies. A secret deactivated by rotation verifies until
/// `rotation_grace` after its `deactivated_at`. A revoked secret (emergency
/// deactivation) and a secret deactivated without a timestamp verify nothing.
/// Every input comes from the database or configuration, never from the token.
pub fn secret_verifies_tokens(
    secret: &JwtSecret,
    now: DateTime<Utc>,
    rotation_grace: Duration,
) -> bool {
    if secret.revoked_at.is_some() {
        return false;
    }
    if secret.is_active {
        return true;
    }
    secret
        .deactivated_at
        .is_some_and(|deactivated_at| now < deactivated_at + rotation_grace)
}

#[automock]
#[async_trait::async_trait]
pub trait JwtSecretLogic: Send + Sync {
    /// Initialize JWT secret on application startup
    /// Returns the active secret, creating one if none exists
    async fn initialize_secret(&self) -> Result<String, Error>;

    /// Get the current active JWT secret
    async fn get_current_secret(&self) -> Result<String, Error>;

    /// Get the active secret with its id, for signing new tokens.
    async fn get_signing_key(&self) -> Result<SigningKey, Error>;

    /// Get the secret that may verify a token with header `kid`, or `None` if
    /// no secret may: an unknown `kid`, a revoked secret, or a rotated-out
    /// secret past the grace window. A token without `kid` (issued before key
    /// ids existed) is verified with the active secret only.
    async fn get_verification_secret(&self, kid: Option<Uuid>) -> Result<Option<String>, Error>;

    /// Generate a new JWT secret (admin operation)
    async fn generate_new_secret(&self, created_by: Option<Uuid>) -> Result<JwtSecret, Error>;

    /// Verify if a secret is currently active
    async fn verify_secret(&self, secret: &str) -> Result<bool, Error>;

    /// Get all secrets for admin purposes
    async fn get_all_secrets(&self) -> Result<Vec<JwtSecret>, Error>;

    /// Emergency deactivation: revokes every secret (no rotation grace), every
    /// access token and every refresh token. A new secret must be generated
    /// (or the service restarted) before anyone can log in again.
    async fn deactivate_all_secrets(&self) -> Result<(), Error>;

    fn clone_box(&self) -> Box<dyn JwtSecretLogic>;
}

impl Clone for Box<dyn JwtSecretLogic> {
    fn clone(&self) -> Box<dyn JwtSecretLogic> {
        self.clone_box()
    }
}

pub struct JwtSecretLogicImpl {
    jwt_secret_repository: Box<dyn JwtSecretRepository>,
    /// How long a rotated-out secret keeps verifying its tokens: one
    /// access-token lifetime, so no live access token breaks on rotation.
    rotation_grace: Duration,
}

impl JwtSecretLogicImpl {
    pub fn new(jwt_secret_repository: Box<dyn JwtSecretRepository>, auth: AuthConfig) -> Self {
        Self {
            jwt_secret_repository,
            rotation_grace: auth.access_token_ttl(),
        }
    }
}

#[async_trait::async_trait]
impl JwtSecretLogic for JwtSecretLogicImpl {
    async fn initialize_secret(&self) -> Result<String, Error> {
        // Use PostgreSQL advisory lock to prevent race conditions when multiple pods start simultaneously
        // Advisory lock ID: Hash of "jwt_secret_init" = 1234567890 (arbitrary constant)
        const JWT_INIT_LOCK_ID: i64 = 1234567890;

        // Try to acquire advisory lock with a short timeout
        // This ensures only one pod initializes the secret at a time
        let lock_acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(JWT_INIT_LOCK_ID)
            .fetch_one(self.jwt_secret_repository.pool())
            .await
            .unwrap_or(false);

        if !lock_acquired {
            // Another pod is initializing, wait briefly and check again
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        let result = match self.jwt_secret_repository.get_active_secret().await? {
            Some(secret) => {
                info!("Found existing active JWT secret");
                Ok(secret.secret)
            }
            None => {
                warn!("No active JWT secret found, generating new one");
                let secret = self.jwt_secret_repository.generate_new_secret(None).await?;
                info!("Generated new JWT secret for application startup");
                Ok(secret.secret)
            }
        };

        // Release advisory lock if we acquired it
        if lock_acquired {
            let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(JWT_INIT_LOCK_ID)
                .execute(self.jwt_secret_repository.pool())
                .await;
        }

        result
    }

    async fn get_current_secret(&self) -> Result<String, Error> {
        Ok(self.get_signing_key().await?.secret)
    }

    async fn get_signing_key(&self) -> Result<SigningKey, Error> {
        match self.jwt_secret_repository.get_active_secret().await? {
            Some(secret) => Ok(SigningKey::from(secret)),
            None => {
                error!("No active JWT secret found and application should have one");
                Err(Error::InvalidInput(
                    "No active JWT secret available".to_string(),
                ))
            }
        }
    }

    async fn get_verification_secret(&self, kid: Option<Uuid>) -> Result<Option<String>, Error> {
        let secret = match kid {
            None => self.jwt_secret_repository.get_active_secret().await?,
            Some(kid) => self
                .jwt_secret_repository
                .get_secret_by_id(kid)
                .await?
                .filter(|secret| secret_verifies_tokens(secret, Utc::now(), self.rotation_grace)),
        };
        Ok(secret.map(|secret| secret.secret))
    }

    async fn generate_new_secret(&self, created_by: Option<Uuid>) -> Result<JwtSecret, Error> {
        let secret = self
            .jwt_secret_repository
            .generate_new_secret(created_by)
            .await?;
        info!("Generated new JWT secret by user: {:?}", created_by);
        Ok(secret)
    }

    async fn verify_secret(&self, secret: &str) -> Result<bool, Error> {
        match self.jwt_secret_repository.get_active_secret().await? {
            Some(active_secret) => Ok(active_secret.secret == secret),
            None => Ok(false),
        }
    }

    async fn get_all_secrets(&self) -> Result<Vec<JwtSecret>, Error> {
        self.jwt_secret_repository.get_all_secrets().await
    }

    async fn deactivate_all_secrets(&self) -> Result<(), Error> {
        let mut tx = self
            .jwt_secret_repository
            .pool()
            .begin()
            .await
            .map_err(Error::DatabaseError)?;
        crate::logic::jwt_secret_tx::deactivate_all_secrets_in_tx(&mut tx).await?;
        tx.commit().await.map_err(Error::DatabaseError)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn JwtSecretLogic> {
        Box::new(Self {
            jwt_secret_repository: self.jwt_secret_repository.clone(),
            rotation_grace: self.rotation_grace,
        })
    }
}

pub fn jwt_secret_logic(pool: PgPool, auth: AuthConfig) -> Box<dyn JwtSecretLogic> {
    let repository = jwt_secret_repository(pool);
    Box::new(JwtSecretLogicImpl::new(repository, auth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::jwt_secret::MockJwtSecretRepository;
    use chrono::Utc;
    use mockall::predicate::*;

    fn create_test_secret() -> JwtSecret {
        JwtSecret {
            id: Uuid::new_v4(),
            secret: "test_secret_123".to_string(),
            is_active: true,
            created_at: Utc::now(),
            created_by: None,
            expires_at: None,
            deactivated_at: None,
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn test_initialize_secret_with_existing() {
        // Use real repository for this test since it needs pool() for advisory locks
        let pool = crate::database::init_pg_pool().await;
        let repository = crate::database::jwt_secret::jwt_secret_repository(pool.clone());

        // Get or create an active secret (simulates existing secret scenario)
        let logic = JwtSecretLogicImpl::new(repository, AuthConfig::default());

        // First initialization should succeed and return a secret
        let result1 = logic.initialize_secret().await.unwrap();
        assert!(!result1.is_empty());

        // Second initialization should return the same existing secret
        let result2 = logic.initialize_secret().await.unwrap();
        assert_eq!(result1, result2, "Should return the same existing secret");
    }

    #[tokio::test]
    async fn test_initialize_secret_without_existing() {
        // This test verifies that initialize_secret works correctly
        // In practice, the first call will create a secret if none exists
        let pool = crate::database::init_pg_pool().await;
        let repository = crate::database::jwt_secret::jwt_secret_repository(pool);

        let logic = JwtSecretLogicImpl::new(repository, AuthConfig::default());

        // Initialize should always succeed and return a valid secret
        let result = logic.initialize_secret().await.unwrap();

        // Should have a valid secret (either existing or newly created)
        assert!(!result.is_empty());
        assert!(
            result.len() >= 32,
            "Secret should be at least 32 chars long"
        );

        // Verify we can get the current secret
        let current_secret = logic.get_current_secret().await.unwrap();
        assert_eq!(current_secret, result);
    }

    #[tokio::test]
    async fn test_get_current_secret_success() {
        let mut mock_repo = MockJwtSecretRepository::new();
        let test_secret = create_test_secret();
        let expected_secret = test_secret.secret.clone();

        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(move || Ok(Some(test_secret.clone())));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        let result = logic.get_current_secret().await.unwrap();

        assert_eq!(result, expected_secret);
    }

    #[tokio::test]
    async fn test_get_current_secret_none_available() {
        let mut mock_repo = MockJwtSecretRepository::new();

        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(|| Ok(None));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        let result = logic.get_current_secret().await;

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InvalidInput(msg) => {
                assert_eq!(msg, "No active JWT secret available");
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_generate_new_secret() {
        let mut mock_repo = MockJwtSecretRepository::new();
        let test_secret = create_test_secret();
        let user_id = Some(Uuid::new_v4());

        mock_repo
            .expect_generate_new_secret()
            .with(eq(user_id))
            .times(1)
            .returning(move |_| Ok(test_secret.clone()));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        let result = logic.generate_new_secret(user_id).await.unwrap();

        assert_eq!(result.secret, "test_secret_123");
        assert!(result.is_active);
    }

    #[tokio::test]
    async fn test_verify_secret_correct() {
        let mut mock_repo = MockJwtSecretRepository::new();
        let test_secret = create_test_secret();

        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(move || Ok(Some(test_secret.clone())));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        let result = logic.verify_secret("test_secret_123").await.unwrap();

        assert!(result);
    }

    #[tokio::test]
    async fn test_verify_secret_incorrect() {
        let mut mock_repo = MockJwtSecretRepository::new();
        let test_secret = create_test_secret();

        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(move || Ok(Some(test_secret.clone())));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        let result = logic.verify_secret("wrong_secret").await.unwrap();

        assert!(!result);
    }

    #[tokio::test]
    async fn test_verify_secret_no_active_secret() {
        let mut mock_repo = MockJwtSecretRepository::new();

        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(|| Ok(None));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        let result = logic.verify_secret("any_secret").await.unwrap();

        assert!(!result);
    }

    fn rotated_out_secret(deactivated_ago: Duration) -> JwtSecret {
        JwtSecret {
            is_active: false,
            deactivated_at: Some(Utc::now() - deactivated_ago),
            ..create_test_secret()
        }
    }

    /// A logic whose repository knows exactly `secret` (by id) and `active`.
    fn logic_with_secrets(
        active: Option<JwtSecret>,
        secret: Option<JwtSecret>,
        auth: AuthConfig,
    ) -> JwtSecretLogicImpl {
        let mut mock_repo = MockJwtSecretRepository::new();
        mock_repo
            .expect_get_active_secret()
            .returning(move || Ok(active.clone()));
        mock_repo
            .expect_get_secret_by_id()
            .returning(move |id| Ok(secret.clone().filter(|secret| secret.id == id)));
        JwtSecretLogicImpl::new(Box::new(mock_repo), auth)
    }

    #[test]
    fn active_secret_verifies_and_revoked_secret_never_does() {
        let grace = Duration::minutes(30);
        let now = Utc::now();
        let active = create_test_secret();
        assert!(secret_verifies_tokens(&active, now, grace));

        let revoked = JwtSecret {
            revoked_at: Some(now),
            ..active.clone()
        };
        assert!(!secret_verifies_tokens(&revoked, now, grace));

        let revoked_within_grace = JwtSecret {
            is_active: false,
            deactivated_at: Some(now - Duration::minutes(1)),
            revoked_at: Some(now),
            ..active.clone()
        };
        assert!(!secret_verifies_tokens(&revoked_within_grace, now, grace));

        // Deactivated before rotation timestamps existed: no grace.
        let legacy_inactive = JwtSecret {
            is_active: false,
            ..active
        };
        assert!(!secret_verifies_tokens(&legacy_inactive, now, grace));
    }

    #[test]
    fn rotated_out_secret_verifies_only_within_the_grace_window() {
        let grace = Duration::minutes(30);
        let now = Utc::now();
        let deactivated_at = now - Duration::minutes(10);
        let secret = JwtSecret {
            is_active: false,
            deactivated_at: Some(deactivated_at),
            ..create_test_secret()
        };
        assert!(secret_verifies_tokens(&secret, now, grace));
        assert!(secret_verifies_tokens(
            &secret,
            deactivated_at + grace - Duration::seconds(1),
            grace
        ));
        assert!(!secret_verifies_tokens(
            &secret,
            deactivated_at + grace,
            grace
        ));
        assert!(!secret_verifies_tokens(
            &secret,
            deactivated_at + Duration::hours(2),
            grace
        ));
    }

    #[tokio::test]
    async fn verification_secret_for_a_kid_follows_the_configured_grace() {
        let auth = AuthConfig {
            access_token_ttl_minutes: 15,
            refresh_token_ttl_days: 7,
        };

        let within = rotated_out_secret(Duration::minutes(14));
        let logic = logic_with_secrets(Some(create_test_secret()), Some(within.clone()), auth);
        assert_eq!(
            logic
                .get_verification_secret(Some(within.id))
                .await
                .unwrap(),
            Some(within.secret.clone())
        );

        let past = rotated_out_secret(Duration::minutes(16));
        let logic = logic_with_secrets(Some(create_test_secret()), Some(past.clone()), auth);
        assert_eq!(
            logic.get_verification_secret(Some(past.id)).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn verification_secret_for_the_active_kid_and_unknown_kid() {
        let active = create_test_secret();
        let logic = logic_with_secrets(
            Some(active.clone()),
            Some(active.clone()),
            AuthConfig::default(),
        );
        assert_eq!(
            logic
                .get_verification_secret(Some(active.id))
                .await
                .unwrap(),
            Some(active.secret.clone())
        );
        assert_eq!(
            logic
                .get_verification_secret(Some(Uuid::new_v4()))
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn token_without_kid_is_verified_with_the_active_secret_only() {
        let mut mock_repo = MockJwtSecretRepository::new();
        let active = create_test_secret();
        let expected = active.secret.clone();
        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(move || Ok(Some(active.clone())));
        // A rotated-out secret is never consulted for a token without kid.
        mock_repo.expect_get_secret_by_id().never();
        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        assert_eq!(
            logic.get_verification_secret(None).await.unwrap(),
            Some(expected)
        );

        let mut mock_repo = MockJwtSecretRepository::new();
        mock_repo.expect_get_active_secret().returning(|| Ok(None));
        mock_repo.expect_get_secret_by_id().never();
        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo), AuthConfig::default());
        assert_eq!(logic.get_verification_secret(None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn signing_key_carries_the_active_secret_id() {
        let active = create_test_secret();
        let expected = SigningKey {
            kid: active.id,
            secret: active.secret.clone(),
        };
        let logic = logic_with_secrets(Some(active), None, AuthConfig::default());
        assert_eq!(logic.get_signing_key().await.unwrap(), expected);
    }
}
