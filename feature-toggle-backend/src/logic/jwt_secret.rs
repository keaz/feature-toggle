use crate::Error;
use crate::database::entity::JwtSecret;
use crate::database::jwt_secret::{JwtSecretRepository, jwt_secret_repository};
use log::{error, info, warn};
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

#[automock]
#[async_trait::async_trait]
pub trait JwtSecretLogic: Send + Sync {
    /// Initialize JWT secret on application startup
    /// Returns the active secret, creating one if none exists
    async fn initialize_secret(&self) -> Result<String, Error>;

    /// Get the current active JWT secret for signing/verification
    async fn get_current_secret(&self) -> Result<String, Error>;

    /// Generate a new JWT secret (admin operation)
    async fn generate_new_secret(&self, created_by: Option<Uuid>) -> Result<JwtSecret, Error>;

    /// Verify if a secret is currently active
    async fn verify_secret(&self, secret: &str) -> Result<bool, Error>;

    /// Get all secrets for admin purposes
    async fn get_all_secrets(&self) -> Result<Vec<JwtSecret>, Error>;

    /// Emergency deactivate all secrets (will require new secret generation)
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
}

impl JwtSecretLogicImpl {
    pub fn new(jwt_secret_repository: Box<dyn JwtSecretRepository>) -> Self {
        Self {
            jwt_secret_repository,
        }
    }
}

#[async_trait::async_trait]
impl JwtSecretLogic for JwtSecretLogicImpl {
    async fn initialize_secret(&self) -> Result<String, Error> {
        match self.jwt_secret_repository.get_active_secret().await? {
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
        }
    }

    async fn get_current_secret(&self) -> Result<String, Error> {
        match self.jwt_secret_repository.get_active_secret().await? {
            Some(secret) => Ok(secret.secret),
            None => {
                error!("No active JWT secret found and application should have one");
                Err(Error::InvalidInput(
                    "No active JWT secret available".to_string(),
                ))
            }
        }
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
        warn!("Deactivating all JWT secrets - this will invalidate all existing tokens");
        self.jwt_secret_repository.deactivate_all_secrets().await
    }

    fn clone_box(&self) -> Box<dyn JwtSecretLogic> {
        Box::new(Self {
            jwt_secret_repository: self.jwt_secret_repository.clone(),
        })
    }
}

pub fn jwt_secret_logic(pool: PgPool) -> Box<dyn JwtSecretLogic> {
    let repository = jwt_secret_repository(pool);
    Box::new(JwtSecretLogicImpl::new(repository))
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
        }
    }

    #[tokio::test]
    async fn test_initialize_secret_with_existing() {
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
        let result = logic.initialize_secret().await.unwrap();

        assert_eq!(result, expected_secret);
    }

    #[tokio::test]
    async fn test_initialize_secret_without_existing() {
        let mut mock_repo = MockJwtSecretRepository::new();
        let test_secret = create_test_secret();
        let expected_secret = test_secret.secret.clone();

        mock_repo
            .expect_get_active_secret()
            .times(1)
            .returning(|| Ok(None));

        mock_repo
            .expect_generate_new_secret()
            .with(eq(None))
            .times(1)
            .returning(move |_| Ok(test_secret.clone()));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
        let result = logic.initialize_secret().await.unwrap();

        assert_eq!(result, expected_secret);
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
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

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
        let result = logic.verify_secret("any_secret").await.unwrap();

        assert!(!result);
    }

    #[tokio::test]
    async fn test_deactivate_all_secrets() {
        let mut mock_repo = MockJwtSecretRepository::new();

        mock_repo
            .expect_deactivate_all_secrets()
            .times(1)
            .returning(|| Ok(()));

        mock_repo
            .expect_clone_box()
            .returning(|| Box::new(MockJwtSecretRepository::new()));

        let logic = JwtSecretLogicImpl::new(Box::new(mock_repo));
        let result = logic.deactivate_all_secrets().await;

        assert!(result.is_ok());
    }
}
