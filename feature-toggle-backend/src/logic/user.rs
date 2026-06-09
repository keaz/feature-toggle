use crate::Error;
use crate::database::user::{CreateUser, UpdateUser, UserRepository};
use crate::model::ID;
use crate::model::Team;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Utc};
use mockall::automock;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ApiUser {
    pub id: ID,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub mobile_number: Option<String>,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
    pub is_temporary_password: bool,
}

#[automock]
#[async_trait::async_trait]
pub trait UserLogic: Send + Sync {
    async fn get_user_by_id(&self, id: ID) -> Result<ApiUser, Error>;
    async fn get_user_by_username(&self, username: String) -> Result<ApiUser, Error>;
    async fn register_user(
        &self,
        input: RegisterUserInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ApiUser, Error>;
    async fn authenticate_user(&self, username: String, password: String)
    -> Result<ApiUser, Error>;
    async fn update_user(
        &self,
        id: ID,
        input: UpdateUserInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ApiUser, Error>;
    async fn reset_password(
        &self,
        id: ID,
        current_password: String,
        new_password: String,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error>;
    async fn set_temporary_password(
        &self,
        user_id: ID,
        temporary_password: String,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error>;
    async fn assign_user_teams(
        &self,
        id: ID,
        team_ids: Vec<ID>,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<bool, Error>;
    async fn get_user_teams(&self, id: ID) -> Result<Vec<Team>, Error>;
    async fn search_users(
        &self,
        team_id: Option<ID>,
        name: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<ApiUser>, i64), Error>;
    async fn admin_exists(&self) -> Result<bool, Error>;
    fn clone_box(&self) -> Box<dyn UserLogic>;
}

impl Clone for Box<dyn UserLogic> {
    fn clone(&self) -> Box<dyn UserLogic> {
        self.clone_box()
    }
}

pub fn user_logic(
    repository: Box<dyn UserRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
) -> Box<dyn UserLogic> {
    user_logic_with_notifications(repository, activity_log_repository, None)
}

pub fn user_logic_with_notifications(
    repository: Box<dyn UserRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
    notification_logic: Option<Box<dyn crate::logic::notification::NotificationLogic>>,
) -> Box<dyn UserLogic> {
    Box::new(UserLogicImpl {
        repository,
        activity_log_repository,
        notification_logic,
    })
}

struct UserLogicImpl {
    repository: Box<dyn UserRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
    notification_logic: Option<Box<dyn crate::logic::notification::NotificationLogic>>,
}

impl Clone for UserLogicImpl {
    fn clone(&self) -> Self {
        Self {
            repository: self.repository.clone_box(),
            activity_log_repository: self.activity_log_repository.clone_box(),
            notification_logic: self
                .notification_logic
                .as_ref()
                .map(|logic| logic.clone_box()),
        }
    }
}

impl UserLogicImpl {
    fn dispatch_notification(&self, event: crate::logic::notification::NotificationEvent) {
        if let Some(logic) = &self.notification_logic {
            crate::logic::notification::spawn_notification_dispatch(logic.clone_box(), event);
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegisterUserInput {
    pub username: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub mobile_number: Option<String>,
    pub is_admin: bool,
    pub is_temporary_password: bool,
}

#[derive(Clone, Debug)]
pub struct UpdateUserInput {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub mobile_number: Option<String>,
    pub is_admin: Option<bool>,
    pub enabled: Option<bool>,
}

fn user_display_name(first_name: &str, last_name: &str, username: &str, fallback: &str) -> String {
    let full_name = format!("{} {}", first_name.trim(), last_name.trim())
        .trim()
        .to_string();
    if !full_name.is_empty() {
        return full_name;
    }

    let trimmed_username = username.trim();
    if !trimmed_username.is_empty() {
        return trimmed_username.to_string();
    }

    fallback.to_string()
}

#[async_trait::async_trait]
impl UserLogic for UserLogicImpl {
    async fn get_user_by_id(&self, id: ID) -> Result<ApiUser, Error> {
        let id =
            Uuid::try_from(id).map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;
        let u = self.repository.get_user_by_id(id).await?;
        Ok(ApiUser {
            id: ID::from(u.id),
            username: u.username,
            first_name: u.first_name,
            last_name: u.last_name,
            email: u.email,
            mobile_number: u.mobile_number,
            is_admin: u.is_admin,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
            is_temporary_password: u.is_temporary_password,
        })
    }

    async fn get_user_by_username(&self, username: String) -> Result<ApiUser, Error> {
        let u = self.repository.get_user_by_username(&username).await?;
        Ok(ApiUser {
            id: ID::from(u.id),
            username: u.username,
            first_name: u.first_name,
            last_name: u.last_name,
            email: u.email,
            mobile_number: u.mobile_number,
            is_admin: u.is_admin,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
            is_temporary_password: u.is_temporary_password,
        })
    }

    async fn register_user(
        &self,
        input: RegisterUserInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ApiUser, Error> {
        if input.username.is_empty() || input.password.is_empty() {
            return Err(Error::InvalidInput(
                "Username and password are required".to_string(),
            ));
        }

        if self
            .repository
            .user_exists_by_username(&input.username)
            .await?
        {
            return Err(Error::RecordAlreadyExists("username".to_string()));
        }

        if self
            .repository
            .user_exists_by_email(&input.email, None)
            .await?
        {
            return Err(Error::RecordAlreadyExists("email".to_string()));
        }

        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(input.password.as_bytes(), &salt)
            .map_err(|_| Error::InvalidInput("Failed to hash password".to_string()))?
            .to_string();

        let created = self
            .repository
            .create_user(CreateUser {
                username: input.username.clone(),
                password_hash,
                first_name: input.first_name.clone(),
                last_name: input.last_name.clone(),
                email: input.email,
                mobile_number: input.mobile_number.clone(),
                is_admin: input.is_admin,
                is_temporary_password: input.is_temporary_password,
            })
            .await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_user_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::USER_CREATED,
            &created.id.to_string(),
            actor_id,
            actor_name,
            format!("Created user '{}'", created.username),
            Some(serde_json::json!({
                "user_id": created.id.to_string(),
                "username": created.username.clone(),
                "is_admin": created.is_admin,
            })),
        )
        .await;

        Ok(ApiUser {
            id: ID::from(created.id),
            username: created.username,
            first_name: created.first_name,
            last_name: created.last_name,
            email: created.email,
            mobile_number: created.mobile_number,
            is_admin: created.is_admin,
            created_at: created.created_at,
            updated_at: created.updated_at,
            last_login: created.last_login,
            is_temporary_password: created.is_temporary_password,
        })
    }

    async fn authenticate_user(
        &self,
        username: String,
        password: String,
    ) -> Result<ApiUser, Error> {
        let u = match self.repository.get_user_by_username(&username).await {
            Ok(user) => user,
            Err(err) => match err {
                // Keep login errors non-enumerable and user-friendly.
                Error::NotFound(_)
                | Error::DatabaseError(sqlx::Error::RowNotFound)
                | Error::InvalidInput(_) => {
                    return Err(Error::Unauthorized(
                        "Invalid username or password".to_string(),
                    ));
                }
                other => return Err(other),
            },
        };
        let parsed_hash = PasswordHash::new(&u.password_hash)
            .map_err(|_| Error::InvalidInput("Stored password hash is invalid".to_string()))?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| Error::Unauthorized("Invalid username or password".to_string()))?;
        let now = Utc::now();
        let _ = self.repository.update_last_login(u.id, now).await?;
        let u = self.repository.get_user_by_id(u.id).await?; // reload to get updated last_login
        Ok(ApiUser {
            id: ID::from(u.id),
            username: u.username,
            first_name: u.first_name,
            last_name: u.last_name,
            email: u.email,
            mobile_number: u.mobile_number,
            is_admin: u.is_admin,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
            is_temporary_password: u.is_temporary_password,
        })
    }

    async fn update_user(
        &self,
        id: ID,
        input: UpdateUserInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ApiUser, Error> {
        let id =
            Uuid::try_from(id).map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;

        // If updating email, validate uniqueness (allow unchanged or same owner)
        if let Some(ref new_email) = input.email
            && self
                .repository
                .user_exists_by_email(new_email, Some(id))
                .await?
        {
            return Err(Error::RecordAlreadyExists("email".to_string()));
        }

        let updated = self
            .repository
            .update_user(UpdateUser {
                id,
                first_name: input.first_name,
                last_name: input.last_name,
                email: input.email,
                mobile_number: input.mobile_number,
                is_admin: input.is_admin,
                enabled: input.enabled,
            })
            .await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_user_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::USER_UPDATED,
            &updated.id.to_string(),
            actor_id,
            actor_name,
            format!("Updated user '{}'", updated.username),
            Some(serde_json::json!({
                "user_id": updated.id.to_string(),
                "username": updated.username.clone(),
            })),
        )
        .await;

        Ok(ApiUser {
            id: ID::from(updated.id),
            username: updated.username,
            first_name: updated.first_name,
            last_name: updated.last_name,
            email: updated.email,
            mobile_number: updated.mobile_number,
            is_admin: updated.is_admin,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
            last_login: updated.last_login,
            is_temporary_password: updated.is_temporary_password,
        })
    }

    async fn reset_password(
        &self,
        id: ID,
        current_password: String,
        new_password: String,
        _actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error> {
        let user_id =
            Uuid::try_from(id).map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;

        // Get current user to verify current password
        let user = self.repository.get_user_by_id(user_id).await?;

        // Verify current password
        let parsed_hash = PasswordHash::new(&user.password_hash)
            .map_err(|_| Error::InvalidInput("Stored password hash is invalid".to_string()))?;
        Argon2::default()
            .verify_password(current_password.as_bytes(), &parsed_hash)
            .map_err(|_| Error::InvalidInput("Current password is incorrect".to_string()))?;

        // Check if new password is same as current password
        if Argon2::default()
            .verify_password(new_password.as_bytes(), &parsed_hash)
            .is_ok()
        {
            return Err(Error::InvalidInput(
                "New password must be different from current password".to_string(),
            ));
        }

        // Hash new password
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let new_password_hash = argon2
            .hash_password(new_password.as_bytes(), &salt)
            .map_err(|_| Error::InvalidInput("Failed to hash new password".to_string()))?
            .to_string();

        // Update password and reset temporary flag
        self.repository
            .update_password(user_id, new_password_hash, false)
            .await?;

        // Note: For reset_password, the actor IS the user themselves (self-service password change)
        // So we use the user's own ID and username for the actor fields
        let _ = crate::utils::activity_logger::log_user_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::USER_PASSWORD_CHANGED,
            &user_id.to_string(),
            Some(user_id),
            Some(user.username.clone()),
            format!("User '{}' changed their password", user.username),
            Some(serde_json::json!({
                "user_id": user_id.to_string(),
                "username": user.username,
            })),
        )
        .await;

        Ok(())
    }

    async fn set_temporary_password(
        &self,
        user_id: ID,
        temporary_password: String,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error> {
        let user_uuid = Uuid::try_from(user_id)
            .map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;

        // Verify user exists
        let _user = self.repository.get_user_by_id(user_uuid).await?;

        // Hash the new temporary password
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(temporary_password.as_bytes(), &salt)
            .map_err(|_| Error::InvalidInput("Failed to hash temporary password".to_string()))?
            .to_string();

        // Update password and set temporary flag to true
        self.repository
            .update_password(user_uuid, password_hash, true)
            .await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_user_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::USER_PASSWORD_CHANGED,
            &user_uuid.to_string(),
            actor_id,
            actor_name,
            format!("Temporary password set for user '{}'", _user.username),
            Some(serde_json::json!({
                "user_id": user_uuid.to_string(),
                "username": _user.username,
                "temporary": true,
            })),
        )
        .await;

        Ok(())
    }

    async fn assign_user_teams(
        &self,
        id: ID,
        team_ids: Vec<ID>,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<bool, Error> {
        let user_id = Uuid::try_from(id.clone())
            .map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;
        let team_ids_uuid: Result<Vec<Uuid>, _> = team_ids
            .iter()
            .map(|id| Uuid::try_from(id.clone()))
            .collect();
        let team_ids_uuid =
            team_ids_uuid.map_err(|e| Error::InvalidInput(format!("Invalid team id: {e}")))?;
        self.repository
            .set_user_teams(user_id, team_ids_uuid.clone())
            .await?;

        let assigned_user_name = self
            .repository
            .get_user_by_id(user_id)
            .await
            .map(|user| {
                user_display_name(
                    &user.first_name,
                    &user.last_name,
                    &user.username,
                    &user_id.to_string(),
                )
            })
            .unwrap_or_else(|_| user_id.to_string());

        let team_name_by_id: HashMap<Uuid, String> = self
            .repository
            .get_user_teams(user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|team| (team.id, team.name))
            .collect();

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity for each team assignment (ignore errors to not fail the operation)
        for team_id in &team_ids_uuid {
            let team_name = team_name_by_id
                .get(team_id)
                .cloned()
                .unwrap_or_else(|| team_id.to_string());
            let added_by = actor_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| name.to_string());
            let message = if let Some(added_by) = added_by.as_deref() {
                format!("User {assigned_user_name} was added to team '{team_name}' by {added_by}.")
            } else {
                format!("User {assigned_user_name} was added to team '{team_name}'.")
            };

            let _ = crate::utils::activity_logger::log_team_activity(
                &self.activity_log_repository,
                crate::utils::activity_logger::activity_types::USER_ADDED_TO_TEAM,
                &team_id.to_string(),
                actor_id,
                actor_name.clone(),
                format!("User '{}' added to team", id),
                Some(serde_json::json!({
                    "user_id": id.to_string(),
                    "team_id": team_id.to_string(),
                })),
            )
            .await;

            self.dispatch_notification(crate::logic::notification::NotificationEvent {
                notification_type: crate::logic::notification::NOTIFICATION_TYPE_USER_ADDED_TO_TEAM
                    .to_string(),
                team_id: Some(*team_id),
                actor_id,
                recipient_user_ids: None,
                subject: format!("User added to team: {team_name}"),
                message,
                metadata: Some(serde_json::json!({
                    "team_id": team_id.to_string(),
                    "team_name": team_name.clone(),
                    "user_id": user_id.to_string(),
                    "user_display_name": assigned_user_name.clone(),
                    "added_by": added_by,
                })),
            });
        }

        Ok(true)
    }

    async fn get_user_teams(&self, id: ID) -> Result<Vec<Team>, Error> {
        let user_id =
            Uuid::try_from(id).map_err(|e| Error::InvalidInput(format!("Invalid user id: {e}")))?;
        let teams = self.repository.get_user_teams(user_id).await?;
        Ok(teams
            .into_iter()
            .map(|t| Team {
                id: ID::from(t.id),
                name: t.name,
                description: t.description,
            })
            .collect())
    }

    async fn search_users(
        &self,
        team_id: Option<ID>,
        name: Option<String>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<ApiUser>, i64), Error> {
        let team_uuid = team_id
            .map(Uuid::try_from)
            .transpose()
            .map_err(|e| Error::InvalidInput(format!("Invalid team id: {e}")))?;
        let (items, total) = self
            .repository
            .search_users(team_uuid, name, page_number, page_size)
            .await?;
        let mapped = items
            .into_iter()
            .map(|u| ApiUser {
                id: ID::from(u.id),
                username: u.username,
                first_name: u.first_name,
                last_name: u.last_name,
                email: u.email,
                mobile_number: u.mobile_number,
                is_admin: u.is_admin,
                created_at: u.created_at,
                updated_at: u.updated_at,
                last_login: u.last_login,
                is_temporary_password: u.is_temporary_password,
            })
            .collect();
        Ok((mapped, total))
    }

    async fn admin_exists(&self) -> Result<bool, Error> {
        self.repository.admin_exists().await
    }

    fn clone_box(&self) -> Box<dyn UserLogic> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::MockActivityLogRepository;
    use crate::database::user::{MockUserRepository, User};
    use chrono::Utc;
    use mockall::predicate::*;

    fn create_mock_activity_log() -> Box<dyn crate::database::activity_log::ActivityLogRepository> {
        let mut mock = MockActivityLogRepository::new();
        mock.expect_create_activity().returning(|_| {
            Ok(crate::database::activity_log::ActivityLogRow {
                id: uuid::Uuid::new_v4(),
                activity_type: "test".to_string(),
                entity_type: "test".to_string(),
                entity_id: "test".to_string(),
                actor_id: None,
                actor_name: None,
                description: "test".to_string(),
                metadata: None,
                created_at: chrono::Utc::now(),
            })
        });
        mock.expect_clone_box()
            .returning(|| create_mock_activity_log());
        Box::new(mock)
    }

    fn sample_user() -> User {
        User {
            id: Uuid::new_v4(),
            username: "jdoe".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$C+z5Yq+YcD1m0M1aQ3sYKA$2GgO7d4r8i5x5KQX1W0b3cVdQd1C8Wk2ZsJp6a9Xg2Q".to_string(),
            first_name: "John".to_string(),
            last_name: "Doe".to_string(),
            email: "john@example.com".to_string(),
            mobile_number: None,
            is_admin: false,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login: None,
            is_temporary_password: false,
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
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let gql = logic.get_user_by_id(ID::from(id)).await.unwrap();
        assert_eq!(gql.username, "jdoe");
        assert_eq!(gql.email, "john@example.com");
        assert_eq!(Uuid::try_from(gql.id.clone()).unwrap(), id);
    }

    #[tokio::test]
    async fn test_get_user_by_id_rejects_invalid_uuid() {
        let mock = MockUserRepository::new();
        let logic = user_logic(Box::new(mock), create_mock_activity_log());

        let err = logic
            .get_user_by_id(ID::from("not-a-uuid"))
            .await
            .expect_err("invalid ids should not panic");

        assert!(matches!(err, Error::InvalidInput(message) if message.contains("Invalid user id")));
    }

    #[tokio::test]
    async fn test_get_user_by_username_maps_fields() {
        let u = sample_user();
        let mut mock = MockUserRepository::new();
        mock.expect_get_user_by_username()
            .with(eq("jdoe"))
            .returning(move |_| Ok(u.clone()));
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let gql = logic
            .get_user_by_username("jdoe".to_string())
            .await
            .unwrap();
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
        mock.expect_create_user().returning(|input| {
            Ok(User {
                id: Uuid::new_v4(),
                username: input.username,
                password_hash: input.password_hash,
                first_name: input.first_name,
                last_name: input.last_name,
                email: input.email,
                mobile_number: input.mobile_number,
                is_admin: input.is_admin,
                enabled: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                last_login: None,
                is_temporary_password: input.is_temporary_password,
            })
        });

        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let res = logic
            .register_user(
                RegisterUserInput {
                    username: "newuser".to_string(),
                    password: "S3cret!".to_string(),
                    first_name: "New".to_string(),
                    last_name: "User".to_string(),
                    email: "new@example.com".to_string(),
                    mobile_number: None,
                    is_admin: true,
                    is_temporary_password: false,
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(res.username, "newuser");
        assert!(res.is_admin);
    }

    #[tokio::test]
    async fn test_register_user_rejects_empty_credentials() {
        let mock = MockUserRepository::new();
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let err = logic
            .register_user(
                RegisterUserInput {
                    username: "".to_string(),
                    password: "".to_string(),
                    first_name: "A".to_string(),
                    last_name: "B".to_string(),
                    email: "a@b.c".to_string(),
                    mobile_number: None,
                    is_admin: false,
                    is_temporary_password: false,
                },
                None,
            )
            .await
            .err()
            .unwrap();
        match err {
            Error::InvalidInput(msg) => assert!(msg.contains("Username and password")),
            _ => panic!("wrong error"),
        }
    }

    #[tokio::test]
    async fn test_register_user_duplicate_username_or_email() {
        let mut mock = MockUserRepository::new();
        // First call: username exists
        mock.expect_user_exists_by_username()
            .returning(|_| Ok(true));
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let err1 = logic
            .register_user(
                RegisterUserInput {
                    username: "exists".to_string(),
                    password: "pw".to_string(),
                    first_name: "A".to_string(),
                    last_name: "B".to_string(),
                    email: "x@y.z".to_string(),
                    mobile_number: None,
                    is_admin: false,
                    is_temporary_password: false,
                },
                None,
            )
            .await
            .err()
            .unwrap();
        match err1 {
            Error::RecordAlreadyExists(field) => assert_eq!(field, "username"),
            _ => panic!("wrong error"),
        }

        // Second scenario: username ok but email exists
        let mut mock2 = MockUserRepository::new();
        mock2
            .expect_user_exists_by_username()
            .returning(|_| Ok(false));
        mock2
            .expect_user_exists_by_email()
            .returning(|_, _| Ok(true));
        let logic2 = user_logic(Box::new(mock2), create_mock_activity_log());
        let err2 = logic2
            .register_user(
                RegisterUserInput {
                    username: "ok".to_string(),
                    password: "pw".to_string(),
                    first_name: "A".to_string(),
                    last_name: "B".to_string(),
                    email: "dup@e.com".to_string(),
                    mobile_number: None,
                    is_admin: false,
                    is_temporary_password: false,
                },
                None,
            )
            .await
            .err()
            .unwrap();
        match err2 {
            Error::RecordAlreadyExists(field) => assert_eq!(field, "email"),
            _ => panic!("wrong error"),
        }
    }

    #[tokio::test]
    async fn test_authenticate_user_success_updates_last_login() {
        // Build a real argon2 password hash from a known password for verification
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password("topsecret".as_bytes(), &salt)
            .unwrap()
            .to_string();
        let mut u = sample_user();
        u.password_hash = hash.clone();
        let id = u.id;

        let mut mock = MockUserRepository::new();
        let u_clone = u.clone();
        mock.expect_get_user_by_username()
            .returning(move |_| Ok(u_clone.clone()));
        // Expect update_last_login to be called
        mock.expect_update_last_login()
            .with(eq(id), function(|_| true))
            .returning(|_, _| Ok(()));
        // After update, logic reloads by id
        let mut u_after = u.clone();
        u_after.last_login = Some(Utc::now());
        mock.expect_get_user_by_id()
            .returning(move |_| Ok(u_after.clone()));

        let logic = user_logic(Box::new(mock), create_mock_activity_log());
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
        u.password_hash = Argon2::default()
            .hash_password("abc".as_bytes(), &salt)
            .unwrap()
            .to_string();
        let mut mock = MockUserRepository::new();
        mock.expect_get_user_by_username()
            .returning(move |_| Ok(u.clone()));
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let err = logic
            .authenticate_user("jdoe".to_string(), "wrong".to_string())
            .await
            .err()
            .unwrap();
        match err {
            Error::Unauthorized(msg) => assert!(msg.contains("Invalid username or password")),
            _ => panic!("wrong error"),
        }
    }

    #[tokio::test]
    async fn test_authenticate_user_missing_username_is_unauthorized() {
        let mut mock = MockUserRepository::new();
        mock.expect_get_user_by_username()
            .returning(|_| Err(Error::DatabaseError(sqlx::Error::RowNotFound)));

        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let err = logic
            .authenticate_user("missing-user".to_string(), "password".to_string())
            .await
            .err()
            .unwrap();

        match err {
            Error::Unauthorized(msg) => assert!(msg.contains("Invalid username or password")),
            _ => panic!("wrong error"),
        }
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
                mobile_number: input.mobile_number,
                is_admin: input.is_admin.unwrap_or(false),
                enabled: input.enabled.unwrap_or(true),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                last_login: None,
                is_temporary_password: false,
            })
        });
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let res = logic
            .update_user(
                ID::from(id),
                UpdateUserInput {
                    first_name: Some("Jane".to_string()),
                    last_name: None,
                    email: Some("new@example.com".to_string()),
                    mobile_number: None,
                    is_admin: Some(true),
                    enabled: Some(true),
                },
                None,
            )
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
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let err = logic
            .update_user(
                ID::from(id),
                UpdateUserInput {
                    first_name: None,
                    last_name: None,
                    email: Some("dup@example.com".to_string()),
                    mobile_number: None,
                    is_admin: None,
                    enabled: None,
                },
                None,
            )
            .await
            .err()
            .unwrap();
        match err {
            Error::RecordAlreadyExists(field) => assert_eq!(field, "email"),
            _ => panic!("wrong error"),
        }
    }

    #[tokio::test]
    async fn test_update_user_rejects_invalid_uuid() {
        let mock = MockUserRepository::new();
        let logic = user_logic(Box::new(mock), create_mock_activity_log());

        let err = logic
            .update_user(
                ID::from("not-a-uuid"),
                UpdateUserInput {
                    first_name: Some("Jane".to_string()),
                    last_name: None,
                    email: None,
                    mobile_number: None,
                    is_admin: None,
                    enabled: None,
                },
                None,
            )
            .await
            .expect_err("invalid ids should not panic");

        assert!(matches!(err, Error::InvalidInput(message) if message.contains("Invalid user id")));
    }

    #[tokio::test]
    async fn test_assign_user_teams_delegates_to_repo() {
        let id = Uuid::new_v4();
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        let mut mock = MockUserRepository::new();
        mock.expect_set_user_teams()
            .with(
                eq(id),
                function(move |vec: &Vec<Uuid>| {
                    vec.len() == 2 && vec.contains(&t1) && vec.contains(&t2)
                }),
            )
            .return_once(|_, _| Ok(()));
        mock.expect_get_user_by_id()
            .with(eq(id))
            .times(1)
            .returning(move |_| {
                Ok(User {
                    id,
                    username: "jdoe".to_string(),
                    password_hash: "hash".to_string(),
                    first_name: "Jane".to_string(),
                    last_name: "Doe".to_string(),
                    email: "jane@example.com".to_string(),
                    mobile_number: None,
                    is_admin: false,
                    enabled: true,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    last_login: None,
                    is_temporary_password: false,
                })
            });
        mock.expect_get_user_teams()
            .with(eq(id))
            .times(1)
            .returning(move |_| {
                Ok(vec![
                    crate::database::entity::Team {
                        id: t1,
                        name: "E-Commerce".to_string(),
                        description: "Team one".to_string(),
                    },
                    crate::database::entity::Team {
                        id: t2,
                        name: "Checkout".to_string(),
                        description: "Team two".to_string(),
                    },
                ])
            });
        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let ok = logic
            .assign_user_teams(ID::from(id), vec![ID::from(t1), ID::from(t2)], None)
            .await
            .unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn test_set_temporary_password_updates_user_with_temp_flag() {
        let user_id = Uuid::new_v4();
        let user = sample_user();

        let mut mock = MockUserRepository::new();
        // Expect get_user_by_id to verify user exists
        mock.expect_get_user_by_id()
            .with(eq(user_id))
            .returning(move |_| Ok(user.clone()));
        // Expect update_password to be called with temporary flag = true
        mock.expect_update_password()
            .with(eq(user_id), function(|_| true), eq(true))
            .returning(|_, _, _| Ok(()));

        let logic = user_logic(Box::new(mock), create_mock_activity_log());
        let result = logic
            .set_temporary_password(ID::from(user_id), "temp123".to_string(), None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_search_users_rejects_invalid_team_uuid() {
        let mock = MockUserRepository::new();
        let logic = user_logic(Box::new(mock), create_mock_activity_log());

        let err = logic
            .search_users(Some(ID::from("not-a-uuid")), None, 1, 10)
            .await
            .expect_err("invalid ids should not panic");

        assert!(matches!(err, Error::InvalidInput(message) if message.contains("Invalid team id")));
    }
}
