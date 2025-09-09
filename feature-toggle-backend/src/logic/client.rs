use crate::database::client::{ClientRepository, CreateClient, UpdateClient};
use crate::database::entity::ClientType as EntityClientType;
use crate::graphql::schema::{
    Client as GqlClient, ClientType as GqlClientType, CreateClientInput, UpdateClientInput,
};
use crate::Error;
use async_graphql::ID;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait ClientLogic: Send + Sync {
    fn clone_box(&self) -> Box<dyn ClientLogic>;
    fn map_entity_to_graphql_type(&self, t: EntityClientType) -> GqlClientType;
    fn map_graphql_to_entity_type(&self, t: GqlClientType) -> EntityClientType;
    async fn get_client_by_id(&self, id: ID) -> Result<GqlClient, Error>;
    async fn get_clients(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<GqlClientType>,
    ) -> Result<Vec<GqlClient>, Error>;
    async fn create_client(
        &self,
        team_id: ID,
        input: CreateClientInput,
    ) -> Result<GqlClient, Error>;
    async fn update_client(&self, id: ID, input: UpdateClientInput) -> Result<GqlClient, Error>;
    async fn delete_client(&self, id: ID) -> Result<(), Error>;
}

impl Clone for Box<dyn ClientLogic> {
    fn clone(&self) -> Box<dyn ClientLogic> {
        self.clone_box()
    }
}

pub fn client_logic(repository: Box<dyn ClientRepository>) -> Box<dyn ClientLogic> {
    Box::new(ClientLogicImpl { repository })
}

#[derive(Clone)]
struct ClientLogicImpl {
    repository: Box<dyn ClientRepository>,
}

#[async_trait::async_trait]
impl ClientLogic for ClientLogicImpl {
    fn clone_box(&self) -> Box<dyn ClientLogic> {
        Box::new(self.clone())
    }

    fn map_entity_to_graphql_type(&self, t: EntityClientType) -> GqlClientType {
        match t {
            EntityClientType::Web => GqlClientType::Web,
            EntityClientType::Backend => GqlClientType::Backend,
        }
    }

    fn map_graphql_to_entity_type(&self, t: GqlClientType) -> EntityClientType {
        match t {
            GqlClientType::Web => EntityClientType::Web,
            GqlClientType::Backend => EntityClientType::Backend,
        }
    }

    async fn get_client_by_id(&self, id: ID) -> Result<GqlClient, Error> {
        let id =
            Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let c = self.repository.get_client_by_id(id).await?;
        Ok(GqlClient {
            id: ID::from(c.id.to_string()),
            team_id: ID::from(c.team_id.to_string()),
            name: c.name,
            description: c.description,
            enabled: c.enabled,
            client_type: self.map_entity_to_graphql_type(c.client_type),
            api_key: c.api_key,
            web_origins: c.web_origins.unwrap_or_default(),
        })
    }

    async fn get_clients(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<GqlClientType>,
    ) -> Result<Vec<GqlClient>, Error> {
        let team_id = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let ct = client_type.map(|t| self.map_graphql_to_entity_type(t));
        let list = self
            .repository
            .get_clients(team_id, name, enabled, ct)
            .await?;
        Ok(list
            .into_iter()
            .map(|c| GqlClient {
                id: ID::from(c.id.to_string()),
                team_id: ID::from(c.team_id.to_string()),
                name: c.name,
                description: c.description,
                enabled: c.enabled,
                client_type: self.map_entity_to_graphql_type(c.client_type),
                api_key: c.api_key,
                web_origins: c.web_origins.unwrap_or_default(),
            })
            .collect())
    }

    async fn create_client(
        &self,
        team_id: ID,
        input: CreateClientInput,
    ) -> Result<GqlClient, Error> {
        let team_id = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        // Validation rules
        match input.client_type {
            GqlClientType::Web => {
                if input
                    .web_origins
                    .as_ref()
                    .map(|v| v.is_empty())
                    .unwrap_or(true)
                {
                    return Err(Error::InvalidInput(
                        "Web client must specify at least one web origin".into(),
                    ));
                }
            }
            GqlClientType::Backend => {
                if let Some(origins) = &input.web_origins
                    && !origins.is_empty()
                {
                    return Err(Error::InvalidInput(
                        "Backend client cannot have web origins".into(),
                    ));
                }
            }
        }

        let create = CreateClient {
            name: input.name,
            description: input.description,
            enabled: input.enabled.unwrap_or(true),
            client_type: self.map_graphql_to_entity_type(input.client_type),
            web_origins: input.web_origins,
        };
        let c = self.repository.create_client(team_id, create).await?;
        Ok(GqlClient {
            id: ID::from(c.id.to_string()),
            team_id: ID::from(c.team_id.to_string()),
            name: c.name,
            description: c.description,
            enabled: c.enabled,
            client_type: self.map_entity_to_graphql_type(c.client_type),
            api_key: c.api_key,
            web_origins: c.web_origins.unwrap_or_default(),
        })
    }

    async fn update_client(&self, id: ID, input: UpdateClientInput) -> Result<GqlClient, Error> {
        let id =
            Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))?;

        if let Some(ct) = input.client_type {
            match ct {
                GqlClientType::Web => {
                    if let Some(origins) = &input.web_origins && origins.is_empty() {
                        return Err(Error::InvalidInput(
                            "Web client must specify at least one web origin".into(),
                        ));
                    }
                }
                GqlClientType::Backend => {
                    if let Some(origins) = &input.web_origins && !origins.is_empty() {
                        return Err(Error::InvalidInput(
                            "Backend client cannot have web origins".into(),
                        ));
                    }
                }
            }
        }

        let update = UpdateClient {
            name: input.name,
            description: input.description,
            enabled: input.enabled,
            client_type: input
                .client_type
                .map(|t| self.map_graphql_to_entity_type(t)),
            web_origins: input.web_origins,
        };
        let c = self.repository.update_client(id, update).await?;
        Ok(GqlClient {
            id: ID::from(c.id.to_string()),
            team_id: ID::from(c.team_id.to_string()),
            name: c.name,
            description: c.description,
            enabled: c.enabled,
            client_type: self.map_entity_to_graphql_type(c.client_type),
            api_key: c.api_key,
            web_origins: c.web_origins.unwrap_or_default(),
        })
    }

    async fn delete_client(&self, id: ID) -> Result<(), Error> {
        let id =
            Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))?;
        self.repository.delete_client(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::client::MockClientRepository;
    use crate::database::entity::Client as EntityClient;

    #[test]
    fn test_type_mapping() {
        let (web_e, be_e) = (EntityClientType::Web, EntityClientType::Backend);
        let (web_g, be_g) = (GqlClientType::Web, GqlClientType::Backend);
        let logic = super::client_logic(Box::new(MockClientRepository::new()));
        assert!(matches!(
            logic.map_entity_to_graphql_type(web_e),
            GqlClientType::Web
        ));
        assert!(matches!(
            logic.map_entity_to_graphql_type(be_e),
            GqlClientType::Backend
        ));
        assert!(matches!(
            logic.map_graphql_to_entity_type(web_g),
            EntityClientType::Web
        ));
        assert!(matches!(
            logic.map_graphql_to_entity_type(be_g),
            EntityClientType::Backend
        ));
    }

    #[tokio::test]
    async fn test_create_client_web_requires_origins() {
        let logic = super::client_logic(Box::new(MockClientRepository::new()));
        let input = CreateClientInput {
            name: "WebC".into(),
            description: None,
            enabled: Some(true),
            client_type: GqlClientType::Web,
            web_origins: Some(vec![]),
        };
        let res = logic.create_client(ID::from(Uuid::new_v4()), input).await;
        assert!(matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Web client must")));
    }

    #[tokio::test]
    async fn test_create_client_backend_cannot_have_origins() {
        let logic = super::client_logic(Box::new(MockClientRepository::new()));
        let input = CreateClientInput {
            name: "BackendC".into(),
            description: None,
            enabled: Some(true),
            client_type: GqlClientType::Backend,
            web_origins: Some(vec!["https://x".into()]),
        };
        let res = logic.create_client(ID::from(Uuid::new_v4()), input).await;
        assert!(
            matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Backend client cannot"))
        );
    }

    #[tokio::test]
    async fn test_update_client_backend_with_origins_fails() {
        let logic = super::client_logic(Box::new(MockClientRepository::new()));
        let input = UpdateClientInput {
            name: None,
            description: None,
            enabled: None,
            client_type: Some(GqlClientType::Backend),
            web_origins: Some(vec!["https://x".into()]),
        };
        let res = logic.update_client(ID::from(Uuid::new_v4()), input).await;
        assert!(
            matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Backend client cannot"))
        );
    }

    #[tokio::test]
    async fn test_create_client_calls_repository() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();
        let team_id_str = team_id.to_string();
        repo.expect_create_client()
            .withf(move |tid, ci| {
                tid.to_string() == team_id_str && matches!(ci.client_type, EntityClientType::Web)
            })
            .times(1)
            .returning(|tid, _| {
                Ok(EntityClient {
                    id: Uuid::new_v4(),
                    team_id: tid,
                    name: "n".into(),
                    description: None,
                    enabled: true,
                    client_type: EntityClientType::Web,
                    api_key: "K".into(),
                    web_origins: Some(vec!["https://a".into()]),
                })
            });
        let logic = super::client_logic(Box::new(repo));
        let input = CreateClientInput {
            name: "n".into(),
            description: None,
            enabled: Some(true),
            client_type: GqlClientType::Web,
            web_origins: Some(vec!["https://a".into()]),
        };
        let out = logic.create_client(ID::from(team_id), input).await.unwrap();
        assert_eq!(out.client_type, GqlClientType::Web);
        assert_eq!(out.web_origins.len(), 1);
    }
}
