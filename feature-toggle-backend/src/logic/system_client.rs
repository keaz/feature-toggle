use crate::Error;
use crate::database::system_client::{
    CreateSystemClient as DbCreateSystemClient, SystemClientRepository,
    UpdateSystemClient as DbUpdateSystemClient,
};
use crate::database::system_client_token::SystemClientTokenRepository;
use crate::logic::jwt_secret::JwtSecretLogic;
use crate::model::{
    CreateSystemClientInput, CreateSystemClientTokenInput, ID, SystemClient, SystemClientToken,
    UpdateSystemClientInput,
};
use chrono::Utc;
use std::collections::BTreeSet;
use uuid::Uuid;

pub const SCOPE_EVALUATE: &str = "evaluate";
pub const SCOPE_METRICS_WRITE: &str = "metrics:write";
pub const SCOPE_ADMIN_READ: &str = "admin:read";
pub const SCOPE_FLAG_WRITE: &str = "flag:write";

pub fn default_system_client_scopes() -> Vec<String> {
    vec![
        SCOPE_EVALUATE.to_string(),
        SCOPE_METRICS_WRITE.to_string(),
        SCOPE_ADMIN_READ.to_string(),
        SCOPE_FLAG_WRITE.to_string(),
    ]
}

#[cfg(test)]
use mockall::automock;

#[derive(Clone, Debug)]
pub struct SystemClientTokenResult {
    pub system_client: SystemClient,
    pub token_meta: SystemClientToken,
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

    async fn list_tokens(&self, id: ID) -> Result<Vec<SystemClientToken>, Error>;

    async fn create_token(
        &self,
        id: ID,
        input: CreateSystemClientTokenInput,
    ) -> Result<SystemClientTokenResult, Error>;

    async fn revoke_token(&self, id: ID) -> Result<bool, Error>;

    async fn regenerate_token(
        &self,
        id: ID,
        input: CreateSystemClientTokenInput,
    ) -> Result<SystemClientTokenResult, Error>;

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

    fn map_token(token: crate::database::entity::SystemClientToken) -> SystemClientToken {
        SystemClientToken {
            id: ID::from(token.id),
            system_client_id: ID::from(token.system_client_id),
            name: token.name,
            scopes: token.scopes,
            expires_at: token.expires_at,
            created_at: token.created_at,
            revoked_at: token.revoked_at,
            is_revoked: token.is_revoked,
            last_used_at: token.last_used_at,
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

    fn normalize_scopes(scopes: Vec<String>) -> Result<Vec<String>, Error> {
        let mut normalized = BTreeSet::new();
        for scope in scopes {
            let trimmed = scope.trim();
            if trimmed.is_empty() {
                continue;
            }
            match trimmed {
                SCOPE_EVALUATE | SCOPE_METRICS_WRITE | SCOPE_ADMIN_READ | SCOPE_FLAG_WRITE => {
                    normalized.insert(trimmed.to_string());
                }
                _ => {
                    return Err(Error::InvalidInput(format!(
                        "Unsupported system client scope: {trimmed}"
                    )));
                }
            }
        }

        if normalized.is_empty() {
            return Err(Error::InvalidInput(
                "At least one system client scope is required".to_string(),
            ));
        }

        Ok(normalized.into_iter().collect())
    }

    fn normalize_token_name(name: Option<String>) -> String {
        name.map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    async fn issue_token(
        &self,
        client: &crate::database::entity::SystemClient,
        name: String,
        scopes: Vec<String>,
        expires_at: chrono::DateTime<Utc>,
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
            expires_at,
            scopes.clone(),
            &secret,
        )
        .map_err(|e| Error::InvalidInput(format!("Failed to create token: {e}")))?;

        let token_hash = crate::middleware::jwt_guard::hash_token(&token);
        self.token_repository
            .store_token(client.id, token_hash, name, scopes, expires_at)
            .await?;

        Ok(token)
    }

    async fn issue_token_result(
        &self,
        client: crate::database::entity::SystemClient,
        input: CreateSystemClientTokenInput,
    ) -> Result<SystemClientTokenResult, Error> {
        if !client.enabled {
            return Err(Error::InvalidInput(
                "Cannot generate token for disabled system client".to_string(),
            ));
        }

        let expires_at = input.expires_at.unwrap_or(client.expires_at);
        Self::validate_expiry(expires_at)?;

        let scopes = Self::normalize_scopes(input.scopes)?;
        let name = Self::normalize_token_name(input.name);
        let token = self.issue_token(&client, name, scopes, expires_at).await?;
        let token_hash = crate::middleware::jwt_guard::hash_token(&token);
        let token_meta = self
            .token_repository
            .get_token_by_hash(&token_hash)
            .await?
            .ok_or_else(|| Error::InvalidInput("token metadata not found".to_string()))?;

        Ok(SystemClientTokenResult {
            system_client: Self::map_entity(client),
            token_meta: Self::map_token(token_meta),
            token,
        })
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

        self.issue_token_result(
            created,
            CreateSystemClientTokenInput {
                name: input.token_name,
                scopes: input.scopes,
                expires_at: None,
            },
        )
        .await
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

    async fn list_tokens(&self, id: ID) -> Result<Vec<SystemClientToken>, Error> {
        let client_id = Self::parse_uuid(id, "system client id")?;
        let client = self.repository.get_system_client_by_id(client_id).await?;
        let tokens = self.token_repository.list_tokens(client.id).await?;
        Ok(tokens.into_iter().map(Self::map_token).collect())
    }

    async fn create_token(
        &self,
        id: ID,
        input: CreateSystemClientTokenInput,
    ) -> Result<SystemClientTokenResult, Error> {
        let client_id = Self::parse_uuid(id, "system client id")?;
        let client = self.repository.get_system_client_by_id(client_id).await?;
        self.issue_token_result(client, input).await
    }

    async fn revoke_token(&self, id: ID) -> Result<bool, Error> {
        let token_id = Self::parse_uuid(id, "system client token id")?;
        self.token_repository.revoke_token_by_id(token_id).await
    }

    async fn regenerate_token(
        &self,
        id: ID,
        input: CreateSystemClientTokenInput,
    ) -> Result<SystemClientTokenResult, Error> {
        let client_id = Self::parse_uuid(id, "system client id")?;
        let client = self.repository.get_system_client_by_id(client_id).await?;
        self.token_repository
            .revoke_all_tokens_for_client(client.id)
            .await?;
        self.issue_token_result(client, input).await
    }

    fn clone_box(&self) -> Box<dyn SystemClientLogic> {
        Box::new(self.clone())
    }
}
