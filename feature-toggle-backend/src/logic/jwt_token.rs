use crate::Error;
use crate::config::AuthConfig;
use crate::database::jwt_token::{JwtToken, JwtTokenRepository, NewSession};
use crate::database::refresh_token::RefreshTokenRepository;
use crate::logic::jwt_secret::{JwtSecretLogic, SigningKey};
use crate::logic::role::RoleLogic;
use crate::logic::user::{ApiUser, UserLogic};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Tokens issued for a session, by login or by a refresh. `Debug` redacts the tokens.
#[derive(Clone)]
pub struct LoginResult {
    pub user: ApiUser,
    pub token: String,
    pub is_temporary: bool,
    /// Opaque refresh token; only its hash is stored.
    pub refresh_token: String,
    /// Access token lifetime in seconds.
    pub expires_in: i64,
}

impl std::fmt::Debug for LoginResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginResult")
            .field("user", &self.user)
            .field("token", &"<redacted>")
            .field("is_temporary", &self.is_temporary)
            .field("refresh_token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// A freshly signed access token together with the values to persist for it.
pub(crate) struct IssuedAccessToken {
    pub token: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
}

/// Signs a user access token whose `exp` is now + the configured lifetime.
pub(crate) fn issue_access_token(
    user_id: Uuid,
    username: &str,
    is_admin: bool,
    roles: Vec<String>,
    signing_key: &SigningKey,
    auth: &AuthConfig,
) -> Result<IssuedAccessToken, Error> {
    let expires_at = Utc::now() + auth.access_token_ttl();
    let token = crate::middleware::jwt_guard::create_jwt_token(
        user_id,
        username,
        is_admin,
        roles,
        signing_key,
        expires_at,
    )
    .map_err(|e| Error::InvalidInput(format!("Failed to create token: {}", e)))?;
    let token_hash = crate::middleware::jwt_guard::hash_token(&token);
    Ok(IssuedAccessToken {
        token,
        token_hash,
        expires_at,
    })
}

/// Generates an opaque refresh token (32 random bytes, base64url without
/// padding) and returns it with the SHA-256 hash that is stored instead.
pub(crate) fn generate_refresh_token() -> (String, String) {
    let bytes: [u8; 32] = rand::random();
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let token_hash = crate::middleware::jwt_guard::hash_token(&token);
    (token, token_hash)
}

#[async_trait::async_trait]
pub trait JwtTokenLogic: Send + Sync {
    /// Password login: [`JwtTokenLogic::authenticate`] then [`JwtTokenLogic::issue_session`].
    async fn login_user(&self, username: String, password: String) -> Result<LoginResult, Error>;
    /// Verifies username and password (and that the account is enabled) without
    /// issuing tokens.
    async fn authenticate(&self, username: String, password: String) -> Result<ApiUser, Error>;
    /// Starts a new session for an already authenticated user: signs an access
    /// token and stores it with a new refresh-token family (see `store_session_tx`).
    async fn issue_session(&self, user: ApiUser) -> Result<LoginResult, Error>;
    async fn logout_user(&self, user_id: Uuid) -> Result<u64, Error>;
    /// Revokes the whole family of `refresh_token` if it belongs to `user_id`.
    async fn revoke_refresh_token_family(
        &self,
        user_id: Uuid,
        refresh_token: &str,
    ) -> Result<u64, Error>;
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
    refresh_repository: Box<dyn RefreshTokenRepository>,
    user_logic: Box<dyn UserLogic>,
    role_logic: Box<dyn RoleLogic>,
    jwt_secret_logic: Box<dyn JwtSecretLogic>,
    auth: AuthConfig,
) -> Box<dyn JwtTokenLogic> {
    Box::new(JwtTokenLogicImpl {
        repository,
        refresh_repository,
        user_logic,
        role_logic,
        jwt_secret_logic,
        auth,
    })
}

#[derive(Clone)]
struct JwtTokenLogicImpl {
    repository: Box<dyn JwtTokenRepository>,
    refresh_repository: Box<dyn RefreshTokenRepository>,
    user_logic: Box<dyn UserLogic>,
    role_logic: Box<dyn RoleLogic>,
    jwt_secret_logic: Box<dyn JwtSecretLogic>,
    auth: AuthConfig,
}

#[async_trait::async_trait]
impl JwtTokenLogic for JwtTokenLogicImpl {
    async fn login_user(&self, username: String, password: String) -> Result<LoginResult, Error> {
        let user = self.authenticate(username, password).await?;
        self.issue_session(user).await
    }

    async fn authenticate(&self, username: String, password: String) -> Result<ApiUser, Error> {
        self.user_logic.authenticate_user(username, password).await
    }

    async fn issue_session(&self, user: ApiUser) -> Result<LoginResult, Error> {
        // Fetch user roles
        let user_id = Uuid::try_from(user.id.clone())
            .map_err(|e| Error::InvalidInput(format!("Invalid user ID: {}", e)))?;
        let roles = self.role_logic.get_user_roles(user.id.clone()).await?;
        let role_names: Vec<String> = roles.into_iter().map(|r| r.name).collect();

        // The access-token and refresh-token rows are stored in one transaction
        // that first locks the signing secret, so an emergency deactivation
        // cannot miss them (see `store_session_tx`). If the secret was rotated
        // or revoked since it was read, sign again with the current one, once.
        for _ in 0..2 {
            // Get the active JWT secret (and its id, the token's `kid`) from the database
            let signing_key = self
                .jwt_secret_logic
                .get_signing_key()
                .await
                .map_err(|e| Error::InvalidInput(format!("Failed to get JWT secret: {}", e)))?;

            let access = issue_access_token(
                user_id,
                &user.username,
                user.is_admin,
                role_names.clone(),
                &signing_key,
                &self.auth,
            )?;
            // Every login starts a new refresh-token family.
            let (refresh_token, refresh_token_hash) = generate_refresh_token();
            let stored = self
                .repository
                .store_session(NewSession {
                    user_id,
                    kid: signing_key.kid,
                    access_token_hash: access.token_hash,
                    access_expires_at: access.expires_at,
                    refresh_family_id: Uuid::new_v4(),
                    refresh_token_hash,
                    refresh_expires_at: Utc::now() + self.auth.refresh_token_ttl(),
                })
                .await?;
            if stored {
                let mut user = user;
                // `store_session` recorded the login.
                user.last_login = Some(Utc::now());
                let is_temporary = user.is_temporary_password;
                return Ok(LoginResult {
                    user,
                    token: access.token,
                    is_temporary,
                    refresh_token,
                    expires_in: self.auth.access_token_ttl().num_seconds(),
                });
            }
        }

        Err(Error::InvalidInput(
            "Failed to get JWT secret: the signing secret is no longer active".to_string(),
        ))
    }

    async fn logout_user(&self, user_id: Uuid) -> Result<u64, Error> {
        self.repository.revoke_all_user_tokens(user_id).await
    }

    async fn revoke_refresh_token_family(
        &self,
        user_id: Uuid,
        refresh_token: &str,
    ) -> Result<u64, Error> {
        let token_hash = crate::middleware::jwt_guard::hash_token(refresh_token);
        self.refresh_repository
            .revoke_family_by_hash(user_id, &token_hash)
            .await
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
    use crate::database::refresh_token::MockRefreshTokenRepository;
    use crate::logic::jwt_secret::MockJwtSecretLogic;
    use crate::logic::role::MockRoleLogic;
    use crate::logic::user::ApiUser;
    use crate::logic::user::MockUserLogic;
    use crate::model::ID;
    use chrono::Utc;
    use mockall::predicate::*;
    use uuid::Uuid;

    fn test_signing_key() -> SigningKey {
        SigningKey {
            kid: Uuid::from_u128(0x5ec2e7),
            secret: "secret".to_string(),
        }
    }

    fn sample_api_user() -> ApiUser {
        ApiUser {
            id: ID::from(Uuid::new_v4()),
            username: "testuser".to_string(),
            first_name: "Test".to_string(),
            last_name: "User".to_string(),
            email: "test@example.com".to_string(),
            mobile_number: None,
            is_admin: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
            is_temporary_password: false,
            auth_source: "local".to_string(),
        }
    }

    #[tokio::test]
    async fn test_login_user_success() {
        let user = sample_api_user();

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
            .expect_get_signing_key()
            .returning(|| Ok(test_signing_key()));

        let mut mock_repo = MockJwtTokenRepository::new();
        mock_repo.expect_store_session().returning(|_| Ok(true));

        let logic = jwt_token_logic(
            Box::new(mock_repo),
            Box::new(MockRefreshTokenRepository::new()),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
            AuthConfig::default(),
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
            Box::new(MockRefreshTokenRepository::new()),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
            AuthConfig::default(),
        );

        let result = logic.logout_user(user_id).await.unwrap();
        assert_eq!(result, 2);
    }

    #[tokio::test]
    async fn test_login_user_with_temporary_password() {
        let mut user = sample_api_user();
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
            .expect_get_signing_key()
            .returning(|| Ok(test_signing_key()));

        let mut mock_repo = MockJwtTokenRepository::new();
        mock_repo.expect_store_session().returning(|_| Ok(true));

        let logic = jwt_token_logic(
            Box::new(mock_repo),
            Box::new(MockRefreshTokenRepository::new()),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
            AuthConfig::default(),
        );

        let result = logic
            .login_user("testuser".to_string(), "temppassword".to_string())
            .await
            .unwrap();

        assert_eq!(result.user.username, "testuser");
        assert!(!result.token.is_empty());
        assert_eq!(result.is_temporary, true); // Should reflect the temporary password status
    }

    #[tokio::test]
    async fn login_issues_refresh_token_and_honours_configured_lifetimes() {
        let user = sample_api_user();
        let expected_user_id = Uuid::try_from(user.id.clone()).unwrap();

        let mut mock_user_logic = MockUserLogic::new();
        mock_user_logic
            .expect_authenticate_user()
            .returning(move |_, _| Ok(user.clone()));
        let mut mock_role_logic = MockRoleLogic::new();
        mock_role_logic
            .expect_get_user_roles()
            .returning(|_| Ok(vec![]));
        let mut mock_jwt_secret_logic = MockJwtSecretLogic::new();
        mock_jwt_secret_logic
            .expect_get_signing_key()
            .returning(|| Ok(test_signing_key()));

        let stored = std::sync::Arc::new(std::sync::Mutex::new(None));
        let stored_clone = stored.clone();
        let mut mock_repo = MockJwtTokenRepository::new();
        mock_repo
            .expect_store_session()
            .times(1)
            .returning(move |session| {
                *stored_clone.lock().unwrap() = Some(session);
                Ok(true)
            });

        let auth = AuthConfig {
            access_token_ttl_minutes: 5,
            refresh_token_ttl_days: 2,
        };
        let logic = jwt_token_logic(
            Box::new(mock_repo),
            Box::new(MockRefreshTokenRepository::new()),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(mock_jwt_secret_logic),
            auth,
        );

        let before = Utc::now();
        let result = logic
            .login_user("testuser".to_string(), "password".to_string())
            .await
            .unwrap();
        let after = Utc::now();

        assert_eq!(result.expires_in, 300);

        // Both rows are stored together, under the kid that signed the token.
        let session: NewSession = stored.lock().unwrap().clone().unwrap();
        assert_eq!(session.kid, test_signing_key().kid);
        assert_eq!(session.user_id, expected_user_id);

        // The access token's exp and the stored row's expires_at are both now + 5 minutes.
        let (access_hash, access_expires_at) =
            (session.access_token_hash.clone(), session.access_expires_at);
        assert_eq!(
            access_hash,
            crate::middleware::jwt_guard::hash_token(&result.token)
        );
        assert!(access_expires_at >= before + chrono::Duration::minutes(5));
        assert!(access_expires_at <= after + chrono::Duration::minutes(5));
        let claims =
            crate::middleware::jwt_guard::decode_verified_claims(&result.token, "secret").unwrap();
        assert_eq!(claims.exp as i64, access_expires_at.timestamp());
        assert_eq!(
            crate::middleware::jwt_guard::token_kid(&result.token).unwrap(),
            Some(test_signing_key().kid)
        );

        // The refresh token is opaque base64url of 32 bytes; only its hash is stored.
        assert_eq!(result.refresh_token.len(), 43);
        assert!(
            result
                .refresh_token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        let (refresh_hash, refresh_expires_at) = (
            session.refresh_token_hash.clone(),
            session.refresh_expires_at,
        );
        assert_eq!(
            refresh_hash,
            crate::middleware::jwt_guard::hash_token(&result.refresh_token)
        );
        assert_ne!(refresh_hash, result.refresh_token);
        assert!(refresh_expires_at >= before + chrono::Duration::days(2));
        assert!(refresh_expires_at <= after + chrono::Duration::days(2));
    }

    fn login_logic(
        repo: MockJwtTokenRepository,
        secret_logic: MockJwtSecretLogic,
    ) -> Box<dyn JwtTokenLogic> {
        let user = sample_api_user();
        let mut mock_user_logic = MockUserLogic::new();
        mock_user_logic
            .expect_authenticate_user()
            .returning(move |_, _| Ok(user.clone()));
        let mut mock_role_logic = MockRoleLogic::new();
        mock_role_logic
            .expect_get_user_roles()
            .returning(|_| Ok(vec![]));
        jwt_token_logic(
            Box::new(repo),
            Box::new(MockRefreshTokenRepository::new()),
            Box::new(mock_user_logic),
            Box::new(mock_role_logic),
            Box::new(secret_logic),
            AuthConfig::default(),
        )
    }

    #[tokio::test]
    async fn login_fails_when_the_signing_secret_is_revoked() {
        let mut secret_logic = MockJwtSecretLogic::new();
        secret_logic
            .expect_get_signing_key()
            .times(2)
            .returning(|| Ok(test_signing_key()));
        // The secret was revoked after it was read: nothing is stored.
        let mut repo = MockJwtTokenRepository::new();
        repo.expect_store_session()
            .times(2)
            .returning(|_| Ok(false));

        let err = login_logic(repo, secret_logic)
            .login_user("testuser".to_string(), "password".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidInput(msg) if msg.contains("JWT secret")));
    }

    #[tokio::test]
    async fn login_signs_again_when_the_signing_secret_was_rotated() {
        let old_key = test_signing_key();
        let new_key = SigningKey {
            kid: Uuid::from_u128(0x2e3),
            secret: "rotated".to_string(),
        };
        let keys = std::sync::Arc::new(std::sync::Mutex::new(vec![
            new_key.clone(),
            old_key.clone(),
        ]));
        let mut secret_logic = MockJwtSecretLogic::new();
        secret_logic
            .expect_get_signing_key()
            .times(2)
            .returning(move || Ok(keys.lock().unwrap().pop().unwrap()));
        let new_kid = new_key.kid;
        let mut repo = MockJwtTokenRepository::new();
        repo.expect_store_session()
            .times(2)
            .returning(move |session| Ok(session.kid == new_kid));

        let result = login_logic(repo, secret_logic)
            .login_user("testuser".to_string(), "password".to_string())
            .await
            .unwrap();
        assert_eq!(
            crate::middleware::jwt_guard::token_kid(&result.token).unwrap(),
            Some(new_key.kid)
        );
        assert!(
            crate::middleware::jwt_guard::decode_verified_claims(&result.token, "rotated").is_ok()
        );
    }

    #[test]
    fn generated_refresh_tokens_are_unique() {
        let (a, a_hash) = generate_refresh_token();
        let (b, b_hash) = generate_refresh_token();
        assert_ne!(a, b);
        assert_ne!(a_hash, b_hash);
        assert_eq!(a_hash.len(), 64);
    }

    #[tokio::test]
    async fn revoke_refresh_token_family_looks_up_by_hash_for_the_user() {
        let user_id = Uuid::new_v4();
        let expected_hash = crate::middleware::jwt_guard::hash_token("raw-refresh");
        let mut mock_refresh = MockRefreshTokenRepository::new();
        mock_refresh
            .expect_revoke_family_by_hash()
            .withf(move |uid, hash| *uid == user_id && hash == expected_hash)
            .times(1)
            .returning(|_, _| Ok(3));

        let logic = jwt_token_logic(
            Box::new(MockJwtTokenRepository::new()),
            Box::new(mock_refresh),
            Box::new(MockUserLogic::new()),
            Box::new(MockRoleLogic::new()),
            Box::new(MockJwtSecretLogic::new()),
            AuthConfig::default(),
        );

        assert_eq!(
            logic
                .revoke_refresh_token_family(user_id, "raw-refresh")
                .await
                .unwrap(),
            3
        );
    }
}
