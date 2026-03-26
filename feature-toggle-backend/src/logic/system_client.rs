use crate::Error;
use crate::database::system_client::{
    CreateSystemClient as DbCreateSystemClient, SystemClientRepository,
    UpdateSystemClient as DbUpdateSystemClient,
};
use crate::database::system_client_token::SystemClientTokenRepository;
use crate::logic::jwt_secret::JwtSecretLogic;
use crate::model::{CreateSystemClientInput, ID, SystemClient, UpdateSystemClientInput};
use chrono::Utc;
use uuid::Uuid;

#[cfg(test)]
use mockall::automock;

#[derive(Clone, Debug)]
pub struct SystemClientTokenResult {
    pub system_client: SystemClient,
    pub token: String,
}

#[cfg_attr(test, automock)]
#[async_trait::async_trait]
pub trait SystemClientLogic: Send + Sync {
    async fn list_system_clients(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<SystemClient>, i64), Error>;

    async fn get_system_client_by_id(&self, id: ID) -> Result<SystemClient, Error>;

    async fn create_system_client(
        &self,
        team_id: ID,
        input: CreateSystemClientInput,
    ) -> Result<SystemClientTokenResult, Error>;

    async fn update_system_client(
        &self,
        id: ID,
        input: UpdateSystemClientInput,
    ) -> Result<SystemClient, Error>;

    async fn regenerate_token(&self, id: ID) -> Result<SystemClientTokenResult, Error>;

    fn clone_box(&self) -> Box<dyn SystemClientLogic>;
}

impl Clone for Box<dyn SystemClientLogic> {
    fn clone(&self) -> Box<dyn SystemClientLogic> {
        self.clone_box()
    }
}

pub fn system_client_logic(
    repository: Box<dyn SystemClientRepository>,
    token_repository: Box<dyn SystemClientTokenRepository>,
    jwt_secret_logic: Box<dyn JwtSecretLogic>,
) -> Box<dyn SystemClientLogic> {
    Box::new(SystemClientLogicImpl {
        repository,
        token_repository,
        jwt_secret_logic,
    })
}

#[derive(Clone)]
struct SystemClientLogicImpl {
    repository: Box<dyn SystemClientRepository>,
    token_repository: Box<dyn SystemClientTokenRepository>,
    jwt_secret_logic: Box<dyn JwtSecretLogic>,
}

impl SystemClientLogicImpl {
    fn parse_uuid(id: ID, field: &str) -> Result<Uuid, Error> {
        Uuid::try_from(id).map_err(|e| Error::InvalidInput(format!("Invalid {field}: {e}")))
    }

    fn map_entity(client: crate::database::entity::SystemClient) -> SystemClient {
        SystemClient {
            id: ID::from(client.id),
            team_id: ID::from(client.team_id),
            name: client.name,
            description: client.description,
            enabled: client.enabled,
            expires_at: client.expires_at,
            created_at: client.created_at,
            updated_at: client.updated_at,
            last_used_at: client.last_used_at,
        }
    }

    fn validate_expiry(expires_at: chrono::DateTime<Utc>) -> Result<(), Error> {
        if expires_at <= Utc::now() {
            return Err(Error::InvalidInput(
                "System client expiry must be in the future".to_string(),
            ));
        }
        Ok(())
    }

    async fn issue_token(
        &self,
        client: &crate::database::entity::SystemClient,
    ) -> Result<String, Error> {
        let secret = self
            .jwt_secret_logic
            .get_current_secret()
            .await
            .map_err(|e| Error::InvalidInput(format!("Failed to get JWT secret: {e}")))?;

        let token = crate::middleware::jwt_guard::create_system_client_jwt_token(
            client.id,
            client.team_id,
            &client.name,
            client.expires_at,
            &secret,
        )
        .map_err(|e| Error::InvalidInput(format!("Failed to create token: {e}")))?;

        let token_hash = crate::middleware::jwt_guard::hash_token(&token);
        self.token_repository
            .store_token(client.id, token_hash, client.expires_at)
            .await?;

        Ok(token)
    }
}

#[async_trait::async_trait]
impl SystemClientLogic for SystemClientLogicImpl {
    async fn list_system_clients(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<SystemClient>, i64), Error> {
        let team_uuid = Self::parse_uuid(team_id, "team id")?;

        let (items, total) = self
            .repository
            .list_system_clients(team_uuid, name, enabled, offset, limit)
            .await?;

        Ok((items.into_iter().map(Self::map_entity).collect(), total))
    }

    async fn get_system_client_by_id(&self, id: ID) -> Result<SystemClient, Error> {
        let client_id = Self::parse_uuid(id, "system client id")?;
        let item = self.repository.get_system_client_by_id(client_id).await?;
        Ok(Self::map_entity(item))
    }

    async fn create_system_client(
        &self,
        team_id: ID,
        input: CreateSystemClientInput,
    ) -> Result<SystemClientTokenResult, Error> {
        let team_uuid = Self::parse_uuid(team_id, "team id")?;
        Self::validate_expiry(input.expires_at)?;

        let created = self
            .repository
            .create_system_client(
                team_uuid,
                DbCreateSystemClient {
                    name: input.name,
                    description: input.description,
                    enabled: input.enabled.unwrap_or(true),
                    expires_at: input.expires_at,
                },
            )
            .await?;

        let token = self.issue_token(&created).await?;

        Ok(SystemClientTokenResult {
            system_client: Self::map_entity(created),
            token,
        })
    }

    async fn update_system_client(
        &self,
        id: ID,
        input: UpdateSystemClientInput,
    ) -> Result<SystemClient, Error> {
        let client_id = Self::parse_uuid(id, "system client id")?;

        if let Some(expires_at) = input.expires_at {
            Self::validate_expiry(expires_at)?;
        }

        let updated = self
            .repository
            .update_system_client(
                client_id,
                DbUpdateSystemClient {
                    name: input.name,
                    description: input.description,
                    enabled: input.enabled,
                    expires_at: input.expires_at,
                },
            )
            .await?;

        Ok(Self::map_entity(updated))
    }

    async fn regenerate_token(&self, id: ID) -> Result<SystemClientTokenResult, Error> {
        let client_id = Self::parse_uuid(id, "system client id")?;
        let client = self.repository.get_system_client_by_id(client_id).await?;

        if !client.enabled {
            return Err(Error::InvalidInput(
                "Cannot generate token for disabled system client".to_string(),
            ));
        }

        Self::validate_expiry(client.expires_at)?;

        self.token_repository
            .revoke_all_tokens_for_client(client.id)
            .await?;

        let token = self.issue_token(&client).await?;

        Ok(SystemClientTokenResult {
            system_client: Self::map_entity(client),
            token,
        })
    }

    fn clone_box(&self) -> Box<dyn SystemClientLogic> {
        Box::new(self.clone())
    }
}
