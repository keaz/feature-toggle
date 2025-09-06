use crate::Error;
use crate::database::jwt_token::{JwtToken, JwtTokenRepository};
use crate::logic::jwt_secret::JwtSecretLogic;
use crate::logic::role::RoleLogic;
use crate::logic::user::{GqlUser, UserLogic};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct LoginResult {
    pub user: GqlUser,
    pub token: String,
    pub is_temporary: bool,
}

#[async_trait::async_trait]
pub trait JwtTokenLogic: Send + Sync {
    async fn login_user(&self, username: String, password: String) -> Result<LoginResult, Error>;
    async fn logout_user(&self, user_id: Uuid) -> Result<u64, Error>;
    async fn store_token(
        &self,
        user_id: Uuid,
        token_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<JwtToken, Error>;
    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error>;
    async fn revoke_all_user_tokens(&self, user_id: Uuid) -> Result<u64, Error>;
    async fn cleanup_expired_tokens(&self) -> Result<u64, Error>;
    async fn get_user_active_tokens(&self, user_id: Uuid) -> Result<Vec<JwtToken>, Error>;
    fn clone_box(&self) -> Box<dyn JwtTokenLogic>;
}

impl Clone for Box<dyn JwtTokenLogic> {
    fn clone(&self) -> Box<dyn JwtTokenLogic> {
        self.clone_box()
    }
}

pub fn jwt_token_logic(
    repository: Box<dyn JwtTokenRepository>,
    user_logic: Box<dyn UserLogic>,
    role_logic: Box<dyn RoleLogic>,
    jwt_secret_logic: Box<dyn JwtSecretLogic>,
) -> Box<dyn JwtTokenLogic> {
    Box::new(JwtTokenLogicImpl {
        repository,
        user_logic,
        role_logic,
        jwt_secret_logic,
    })
}

#[derive(Clone)]
struct JwtTokenLogicImpl {
    repository: Box<dyn JwtTokenRepository>,
    user_logic: Box<dyn UserLogic>,
    role_logic: Box<dyn RoleLogic>,
    jwt_secret_logic: Box<dyn JwtSecretLogic>,
}

#[async_trait::async_trait]
impl JwtTokenLogic for JwtTokenLogicImpl {
    async fn login_user(&self, username: String, password: String) -> Result<LoginResult, Error> {
        // Authenticate user
        let user = self
            .user_logic
            .authenticate_user(username, password)
            .await?;

        // Fetch user roles
        let user_id = Uuid::try_from(user.id.clone())
            .map_err(|e| Error::InvalidInput(format!("Invalid user ID: {}", e)))?;
        let roles = self.role_logic.get_user_roles(user.id.clone()).await?;
        let role_names: Vec<String> = roles.into_iter().map(|r| r.name).collect();

        // Get current JWT secret from database
        let jwt_secret = self
            .jwt_secret_logic
            .get_current_secret()
            .await
            .map_err(|e| Error::InvalidInput(format!("Failed to get JWT secret: {}", e)))?;

        // Generate JWT token
        let token = crate::middleware::jwt_guard::create_jwt_token(
            user_id,
            &user.username,
            user.is_admin,
            role_names,
            &jwt_secret,
        )
        .map_err(|e| Error::InvalidInput(format!("Failed to create token: {}", e)))?;

        // Store token hash in database
        let token_hash = crate::middleware::jwt_guard::hash_token(&token);
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(24);

        self.repository
            .store_token(user_id, token_hash, expires_at)
            .await?;

        let is_temporary = user.is_temporary_password;
        Ok(LoginResult {
            user,
            token,
            is_temporary,
        })
    }

    async fn logout_user(&self, user_id: Uuid) -> Result<u64, Error> {
        self.repository.revoke_all_user_tokens(user_id).await
    }

    async fn store_token(
        &self,
        user_id: Uuid,
        token_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<JwtToken, Error> {
        self.repository
            .store_token(user_id, token_hash, expires_at)
            .await
    }

    async fn is_token_valid(&self, token_hash: &str) -> Result<bool, Error> {
        self.repository.is_token_valid(token_hash).await
    }

    async fn revoke_token(&self, token_hash: &str) -> Result<bool, Error> {
        self.repository.revoke_token(token_hash).await
    }

    async fn revoke_all_user_tokens(&self, user_id: Uuid) -> Result<u64, Error> {
        self.repository.revoke_all_user_tokens(user_id).await
    }

    async fn cleanup_expired_tokens(&self) -> Result<u64, Error> {
        self.repository.cleanup_expired_tokens().await
    }

    async fn get_user_active_tokens(&self, user_id: Uuid) -> Result<Vec<JwtToken>, Error> {
        self.repository.get_user_active_tokens(user_id).await
    }

    fn clone_box(&self) -> Box<dyn JwtTokenLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::jwt_token::MockJwtTokenRepository;
    use crate::logic::jwt_secret::MockJwtSecretLogic;
    use crate::logic::role::MockRoleLogic;
    use crate::logic::user::GqlUser;
    use crate::logic::user::MockUserLogic;
    use async_graphql::ID;
    use chrono::Utc;
    use mockall::predicate::*;
    use uuid::Uuid;

    fn sample_gql_user() -> GqlUser {
        GqlUser {
            id: ID::from(Uuid::new_v4()),
            username: "testuser".to_string(),
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            email: "test@example.com".to_string(),
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
            is_temporary_password: false,
        }
    }

    #[tokio::test]
    async fn test_login_user_success() {
        let user = sample_gql_user();
        let user_uuid = Uuid::try_from(user.id.clone()).unwrap();

        let mut mock_user_logic = MockUserLogic::new();
        mock_user_logic
            .expect_authenticate_user()
            .with(eq("testuser".to_string()), eq("password".to_string()))
            .returning(move |_, _| Ok(user.clone()));

        let mut mock_role_logic = MockRoleLogic::new();
        mock_role_logic
            .expect_get_user_roles()
            .returning(|_| Ok(vec![]));

        let mut mock_jwt_secret_logic = MockJwtSecretLogic::new();
        mock_jwt_secret_logic
            .expect_get_current_secret()
            .returning(|| Ok("secret".to_string()));

        let mut mock_repo = MockJwtTokenRepository::new();
        mock_repo
            .expect_store_token()
            .returning(|user_id, token_hash, expires_at| {
                Ok(JwtToken {
                    id: Uuid::new_v4(),
                    user_id,
                    token_hash,
                    expires_at,
                    created_at: Utc::now(),
                    revoked_at: None,
                    is_revoked: false,
                })
            });

        let logic = jwt_token_logic(
            Box::new(mock_repo),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
        );

        let result = logic
            .login_user("testuser".to_string(), "password".to_string())
            .await
            .unwrap();

        assert_eq!(result.user.username, "testuser");
        assert!(!result.token.is_empty());
        assert_eq!(result.is_temporary, false); // user.is_temporary_password is false
    }

    #[tokio::test]
    async fn test_logout_user_success() {
        let user_id = Uuid::new_v4();

        let mut mock_repo = MockJwtTokenRepository::new();
        mock_repo
            .expect_revoke_all_user_tokens()
            .with(eq(user_id))
            .returning(|_| Ok(2));

        let mock_user_logic = MockUserLogic::new();
        let mock_role_logic = MockRoleLogic::new();
        let mock_jwt_secret_logic = MockJwtSecretLogic::new();

        let logic = jwt_token_logic(
            Box::new(mock_repo),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
        );

        let result = logic.logout_user(user_id).await.unwrap();
        assert_eq!(result, 2);
    }

    #[tokio::test]
    async fn test_login_user_with_temporary_password() {
        let mut user = sample_gql_user();
        user.is_temporary_password = true; // Set as temporary password

        let mut mock_user_logic = MockUserLogic::new();
        mock_user_logic
            .expect_authenticate_user()
            .with(eq("testuser".to_string()), eq("temppassword".to_string()))
            .returning(move |_, _| Ok(user.clone()));

        let mut mock_role_logic = MockRoleLogic::new();
        mock_role_logic
            .expect_get_user_roles()
            .returning(|_| Ok(vec![]));

        let mut mock_jwt_secret_logic = MockJwtSecretLogic::new();
        mock_jwt_secret_logic
            .expect_get_current_secret()
            .returning(|| Ok("secret".to_string()));

        let mut mock_repo = MockJwtTokenRepository::new();
        mock_repo
            .expect_store_token()
            .returning(|user_id, token_hash, expires_at| {
                Ok(JwtToken {
                    id: Uuid::new_v4(),
                    user_id,
                    token_hash,
                    expires_at,
                    created_at: Utc::now(),
                    revoked_at: None,
                    is_revoked: false,
                })
            });

        let logic = jwt_token_logic(
            Box::new(mock_repo),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
        );

        let result = logic
            .login_user("testuser".to_string(), "temppassword".to_string())
            .await
            .unwrap();

        assert_eq!(result.user.username, "testuser");
        assert!(!result.token.is_empty());
        assert_eq!(result.is_temporary, true); // Should reflect the temporary password status
    }
}
