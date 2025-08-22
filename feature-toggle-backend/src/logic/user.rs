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
