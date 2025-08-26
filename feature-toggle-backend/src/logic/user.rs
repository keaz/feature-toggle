use crate::database::user::{CreateUser, UpdateUser, UserRepository};
use crate::Error;
use argon2::{password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString}, Argon2};
use async_graphql::ID;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct GqlUser {
    pub id: ID,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

#[async_trait::async_trait]
pub trait UserLogic: Send + Sync {
    async fn get_user_by_id(&self, id: ID) -> Result<GqlUser, Error>;
    async fn get_user_by_username(&self, username: String) -> Result<GqlUser, Error>;
    async fn register_user(&self, input: RegisterUserInput) -> Result<GqlUser, Error>;
    async fn authenticate_user(&self, username: String, password: String) -> Result<GqlUser, Error>;
    async fn update_user(&self, id: ID, input: UpdateGqlUserInput) -> Result<GqlUser, Error>;
    fn clone_box(&self) -> Box<dyn UserLogic>;
}

impl Clone for Box<dyn UserLogic> {
    fn clone(&self) -> Box<dyn UserLogic> {
        self.clone_box()
    }
}

pub fn user_logic(repository: Box<dyn UserRepository>) -> Box<dyn UserLogic> {
    Box::new(UserLogicImpl { repository })
}

#[derive(Clone)]
struct UserLogicImpl {
    repository: Box<dyn UserRepository>,
}

#[derive(Clone, Debug)]
pub struct RegisterUserInput {
    pub username: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub is_admin: bool,
}

#[derive(Clone, Debug)]
pub struct UpdateGqlUserInput {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub is_admin: Option<bool>,
    pub enabled: Option<bool>,
}

#[async_trait::async_trait]
impl UserLogic for UserLogicImpl {
    async fn get_user_by_id(&self, id: ID) -> Result<GqlUser, Error> {
        let id = Uuid::try_from(id).unwrap();
        let u = self.repository.get_user_by_id(id).await?;
        Ok(GqlUser {
            id: ID::from(u.id),
            username: u.username,
            first_name: u.first_name,
            last_name: u.last_name,
            email: u.email,
            is_admin: u.is_admin,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
        })
    }

    async fn get_user_by_username(&self, username: String) -> Result<GqlUser, Error> {
        let u = self.repository.get_user_by_username(&username).await?;
        Ok(GqlUser {
            id: ID::from(u.id),
            username: u.username,
            first_name: u.first_name,
            last_name: u.last_name,
            email: u.email,
            is_admin: u.is_admin,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
        })
    }

    async fn register_user(&self, input: RegisterUserInput) -> Result<GqlUser, Error> {
        if input.username.is_empty() || input.password.is_empty() {
            return Err(Error::InvalidInput("Username and password are required".to_string()));
        }

        if self.repository.user_exists_by_username(&input.username).await? {
            return Err(Error::RecordAlreadyExists("username".to_string()));
        }

        if self.repository.user_exists_by_email(&input.email, None).await? {
            return Err(Error::RecordAlreadyExists("email".to_string()));
        }

        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(input.password.as_bytes(), &salt)
            .map_err(|_| Error::InvalidInput("Failed to hash password".to_string()))?
            .to_string();

        let created = self.repository.create_user(CreateUser{
            username: input.username,
            password_hash,
            first_name: input.first_name,
            last_name: input.last_name,
            email: input.email,
            is_admin: input.is_admin,
        }).await?;

        Ok(GqlUser {
            id: ID::from(created.id),
            username: created.username,
            first_name: created.first_name,
            last_name: created.last_name,
            email: created.email,
            is_admin: created.is_admin,
            created_at: created.created_at,
            updated_at: created.updated_at,
            last_login: created.last_login,
        })
    }

    async fn authenticate_user(&self, username: String, password: String) -> Result<GqlUser, Error> {
        let u = self.repository.get_user_by_username(&username).await?;
        let parsed_hash = PasswordHash::new(&u.password_hash)
            .map_err(|_| Error::InvalidInput("Stored password hash is invalid".to_string()))?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| Error::InvalidInput("Invalid username or password".to_string()))?;
        let now = Utc::now();
        let _ = self.repository.update_last_login(u.id, now).await?;
        let u = self.repository.get_user_by_id(u.id).await?; // reload to get updated last_login
        Ok(GqlUser {
            id: ID::from(u.id),
            username: u.username,
            first_name: u.first_name,
            last_name: u.last_name,
            email: u.email,
            is_admin: u.is_admin,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
        })
    }

    async fn update_user(&self, id: ID, input: UpdateGqlUserInput) -> Result<GqlUser, Error> {
        let id = Uuid::try_from(id).unwrap();

        // If updating email, validate uniqueness (allow unchanged or same owner)
        if let Some(ref new_email) = input.email && self.repository.user_exists_by_email(new_email, Some(id)).await?{
            return Err(Error::RecordAlreadyExists("email".to_string()));
        }

        let updated = self.repository.update_user(UpdateUser {
            id,
            first_name: input.first_name,
            last_name: input.last_name,
            email: input.email,
            is_admin: input.is_admin,
            enabled: input.enabled,
        }).await?;

        Ok(GqlUser {
            id: ID::from(updated.id),
            username: updated.username,
            first_name: updated.first_name,
            last_name: updated.last_name,
            email: updated.email,
            is_admin: updated.is_admin,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
            last_login: updated.last_login,
        })
    }

    fn clone_box(&self) -> Box<dyn UserLogic> { Box::new(self.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::user::{MockUserRepository, User};
    use chrono::Utc;
    use mockall::predicate::*;

    fn sample_user() -> User {
        User {
            id: Uuid::new_v4(),
            username: "jdoe".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$C+z5Yq+YcD1m0M1aQ3sYKA$2GgO7d4r8i5x5KQX1W0b3cVdQd1C8Wk2ZsJp6a9Xg2Q".to_string(),
            first_name: "John".to_string(),
            last_name: "Doe".to_string(),
            email: "john@example.com".to_string(),
            is_admin: false,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
        }
    }

    #[tokio::test]
    async fn test_get_user_by_id_maps_fields() {
        let u = sample_user();
        let id = u.id;
        let mut mock = MockUserRepository::new();
        mock.expect_get_user_by_id()
            .with(eq(id))
            .returning(move |_| Ok(u.clone()));
        let logic = user_logic(Box::new(mock));
        let gql = logic.get_user_by_id(ID::from(id)).await.unwrap();
        assert_eq!(gql.username, "jdoe");
        assert_eq!(gql.email, "john@example.com");
        assert_eq!(Uuid::try_from(gql.id.clone()).unwrap(), id);
    }

    #[tokio::test]
    async fn test_get_user_by_username_maps_fields() {
        let u = sample_user();
        let mut mock = MockUserRepository::new();
        mock.expect_get_user_by_username()
            .with(eq("jdoe"))
            .returning(move |_| Ok(u.clone()));
        let logic = user_logic(Box::new(mock));
        let gql = logic.get_user_by_username("jdoe".to_string()).await.unwrap();
        assert_eq!(gql.first_name, "John");
        assert_eq!(gql.last_name, "Doe");
    }

    #[tokio::test]
    async fn test_register_user_validates_and_creates() {
        // Prepare repository expectations
        let mut mock = MockUserRepository::new();
        mock.expect_user_exists_by_username()
            .with(eq("newuser"))
            .returning(|_| Ok(false));
        mock.expect_user_exists_by_email()
            .with(eq("new@example.com"), eq(None))
            .returning(|_, _| Ok(false));

        // For create_user, we don't know the actual password hash because logic generates it, so accept any
        mock.expect_create_user()
            .returning(|input| {
                Ok(User {
                    id: Uuid::new_v4(),
                    username: input.username,
                    password_hash: input.password_hash,
                    first_name: input.first_name,
                    last_name: input.last_name,
                    email: input.email,
                    is_admin: input.is_admin,
                    enabled: true,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    last_login: None,
                })
            });

        let logic = user_logic(Box::new(mock));
        let res = logic
            .register_user(RegisterUserInput {
                username: "newuser".to_string(),
                password: "S3cret!".to_string(),
                first_name: "New".to_string(),
                last_name: "User".to_string(),
                email: "new@example.com".to_string(),
                is_admin: true,
            })
            .await
            .unwrap();

        assert_eq!(res.username, "newuser");
        assert!(res.is_admin);
    }

    #[tokio::test]
    async fn test_register_user_rejects_empty_credentials() {
        let mock = MockUserRepository::new();
        let logic = user_logic(Box::new(mock));
        let err = logic
            .register_user(RegisterUserInput {
                username: "".to_string(),
                password: "".to_string(),
                first_name: "A".to_string(),
                last_name: "B".to_string(),
                email: "a@b.c".to_string(),
                is_admin: false,
            })
            .await
            .err()
            .unwrap();
        match err { Error::InvalidInput(msg) => assert!(msg.contains("Username and password")), _ => panic!("wrong error") }
    }

    #[tokio::test]
    async fn test_register_user_duplicate_username_or_email() {
        let mut mock = MockUserRepository::new();
        // First call: username exists
        mock.expect_user_exists_by_username()
            .returning(|_| Ok(true));
        let logic = user_logic(Box::new(mock));
        let err1 = logic
            .register_user(RegisterUserInput {
                username: "exists".to_string(),
                password: "pw".to_string(),
                first_name: "A".to_string(),
                last_name: "B".to_string(),
                email: "x@y.z".to_string(),
                is_admin: false,
            })
            .await
            .err()
            .unwrap();
        match err1 { Error::RecordAlreadyExists(field) => assert_eq!(field, "username"), _ => panic!("wrong error") }

        // Second scenario: username ok but email exists
        let mut mock2 = MockUserRepository::new();
        mock2.expect_user_exists_by_username().returning(|_| Ok(false));
        mock2
            .expect_user_exists_by_email()
            .returning(|_, _| Ok(true));
        let logic2 = user_logic(Box::new(mock2));
        let err2 = logic2
            .register_user(RegisterUserInput {
                username: "ok".to_string(),
                password: "pw".to_string(),
                first_name: "A".to_string(),
                last_name: "B".to_string(),
                email: "dup@e.com".to_string(),
                is_admin: false,
            })
            .await
            .err()
            .unwrap();
        match err2 { Error::RecordAlreadyExists(field) => assert_eq!(field, "email"), _ => panic!("wrong error") }
    }

    #[tokio::test]
    async fn test_authenticate_user_success_updates_last_login() {
        // Build a real argon2 password hash from a known password for verification
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default().hash_password("topsecret".as_bytes(), &salt).unwrap().to_string();
        let mut u = sample_user();
        u.password_hash = hash.clone();
        let id = u.id;

        let mut mock = MockUserRepository::new();
        let u_clone = u.clone();
        mock.expect_get_user_by_username().returning(move |_| Ok(u_clone.clone()));
        // Expect update_last_login to be called
        mock.expect_update_last_login()
            .with(eq(id), function(|_| true))
            .returning(|_, _| Ok(()));
        // After update, logic reloads by id
        let mut u_after = u.clone();
        u_after.last_login = Some(Utc::now());
        mock.expect_get_user_by_id().returning(move |_| Ok(u_after.clone()));

        let logic = user_logic(Box::new(mock));
        let res = logic
            .authenticate_user("jdoe".to_string(), "topsecret".to_string())
            .await
            .unwrap();
        assert!(res.last_login.is_some());
    }

    #[tokio::test]
    async fn test_authenticate_user_wrong_password() {
        let mut u = sample_user();
        // set a hash for password "abc"
        let salt = SaltString::generate(&mut OsRng);
        u.password_hash = Argon2::default().hash_password("abc".as_bytes(), &salt).unwrap().to_string();
        let mut mock = MockUserRepository::new();
        mock.expect_get_user_by_username().returning(move |_| Ok(u.clone()));
        let logic = user_logic(Box::new(mock));
        let err = logic
            .authenticate_user("jdoe".to_string(), "wrong".to_string())
            .await
            .err()
            .unwrap();
        match err { Error::InvalidInput(msg) => assert!(msg.contains("Invalid username or password")), _ => panic!("wrong error") }
    }

    #[tokio::test]
    async fn test_update_user_checks_email_uniqueness_and_updates() {
        let u = sample_user();
        let id = u.id;
        let mut mock = MockUserRepository::new();
        // Email uniqueness: repository returns false (no conflict)
        mock.expect_user_exists_by_email()
            .with(eq("new@example.com"), eq(Some(id)))
            .returning(|_, _| Ok(false));
        // update_user returns updated record
        mock.expect_update_user().returning(move |input| {
            assert_eq!(input.id, id);
            Ok(User {
                id,
                username: "jdoe".to_string(),
                password_hash: "hash".to_string(),
                first_name: input.first_name.unwrap_or("John".to_string()),
                last_name: input.last_name.unwrap_or("Doe".to_string()),
                email: input.email.unwrap_or("john@example.com".to_string()),
                is_admin: input.is_admin.unwrap_or(false),
                enabled: input.enabled.unwrap_or(true),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                last_login: None,
            })
        });
        let logic = user_logic(Box::new(mock));
        let res = logic
            .update_user(ID::from(id), UpdateGqlUserInput {
                first_name: Some("Jane".to_string()),
                last_name: None,
                email: Some("new@example.com".to_string()),
                is_admin: Some(true),
                enabled: Some(true),
            })
            .await
            .unwrap();
        assert_eq!(res.first_name, "Jane");
        assert_eq!(res.email, "new@example.com");
        assert!(res.is_admin);
    }

    #[tokio::test]
    async fn test_update_user_email_conflict() {
        let u = sample_user();
        let id = u.id;
        let mut mock = MockUserRepository::new();
        mock.expect_user_exists_by_email()
            .with(eq("dup@example.com"), eq(Some(id)))
            .returning(|_, _| Ok(true));
        let logic = user_logic(Box::new(mock));
        let err = logic
            .update_user(ID::from(id), UpdateGqlUserInput {
                first_name: None,
                last_name: None,
                email: Some("dup@example.com".to_string()),
                is_admin: None,
                enabled: None,
            })
            .await
            .err()
            .unwrap();
        match err { Error::RecordAlreadyExists(field) => assert_eq!(field, "email"), _ => panic!("wrong error") }
    }
}
