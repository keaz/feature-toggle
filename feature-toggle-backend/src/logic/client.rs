use crate::Error;
use crate::database::client::{ClientRepository, CreateClient, UpdateClient};
use crate::database::entity::ClientType as EntityClientType;
use crate::model::{
    Client as ModelClient, ClientType as ModelClientType, CreateClientInput, UpdateClientInput,
};
use crate::model::ID;
use uuid::Uuid;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
#[async_trait::async_trait]
pub trait ClientLogic: Send + Sync {
    fn clone_box(&self) -> Box<dyn ClientLogic>;
    fn map_entity_to_api_type(&self, t: EntityClientType) -> ModelClientType;
    fn map_api_to_entity_type(&self, t: ModelClientType) -> EntityClientType;
    async fn get_client_by_id(&self, id: ID) -> Result<ModelClient, Error>;
    async fn get_clients(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ModelClientType>,
    ) -> Result<Vec<ModelClient>, Error>;
    async fn get_clients_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ModelClientType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<ModelClient>, i64), Error>;
    async fn get_clients_with_offset(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ModelClientType>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ModelClient>, i64), Error>;
    async fn create_client(
        &self,
        team_id: ID,
        input: CreateClientInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ModelClient, Error>;
    async fn update_client(
        &self,
        id: ID,
        input: UpdateClientInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ModelClient, Error>;
    async fn delete_client(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error>;

    // Count clients
    async fn count_clients(&self, team_id: Option<ID>, enabled: Option<bool>)
    -> Result<i64, Error>;
}

impl Clone for Box<dyn ClientLogic> {
    fn clone(&self) -> Box<dyn ClientLogic> {
        self.clone_box()
    }
}

pub fn client_logic(
    repository: Box<dyn ClientRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
) -> Box<dyn ClientLogic> {
    Box::new(ClientLogicImpl {
        repository,
        activity_log_repository,
    })
}

struct ClientLogicImpl {
    repository: Box<dyn ClientRepository>,
    activity_log_repository: Box<dyn crate::database::activity_log::ActivityLogRepository>,
}

impl Clone for ClientLogicImpl {
    fn clone(&self) -> Self {
        Self {
            repository: self.repository.clone_box(),
            activity_log_repository: self.activity_log_repository.clone_box(),
        }
    }
}

#[async_trait::async_trait]
impl ClientLogic for ClientLogicImpl {
    fn clone_box(&self) -> Box<dyn ClientLogic> {
        Box::new(self.clone())
    }

    fn map_entity_to_api_type(&self, t: EntityClientType) -> ModelClientType {
        match t {
            EntityClientType::Web => ModelClientType::Web,
            EntityClientType::Backend => ModelClientType::Backend,
        }
    }

    fn map_api_to_entity_type(&self, t: ModelClientType) -> EntityClientType {
        match t {
            ModelClientType::Web => EntityClientType::Web,
            ModelClientType::Backend => EntityClientType::Backend,
        }
    }

    async fn get_client_by_id(&self, id: ID) -> Result<ModelClient, Error> {
        let id =
            Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))?;
        let c = self.repository.get_client_by_id(id).await?;
        Ok(ModelClient {
            id: ID::from(c.id.to_string()),
            team_id: ID::from(c.team_id.to_string()),
            environment_id: ID::from(c.environment_id.to_string()),
            name: c.name,
            description: c.description,
            enabled: c.enabled,
            client_type: self.map_entity_to_api_type(c.client_type),
            api_key: c.api_key,
            web_origins: c.web_origins.unwrap_or_default(),
        })
    }

    async fn get_clients(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ModelClientType>,
    ) -> Result<Vec<ModelClient>, Error> {
        let team_id = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let ct = client_type.map(|t| self.map_api_to_entity_type(t));
        let list = self
            .repository
            .get_clients(team_id, name, enabled, ct)
            .await?;
        Ok(list
            .into_iter()
            .map(|c| ModelClient {
                id: ID::from(c.id.to_string()),
                team_id: ID::from(c.team_id.to_string()),
                environment_id: ID::from(c.environment_id.to_string()),
                name: c.name,
                description: c.description,
                enabled: c.enabled,
                client_type: self.map_entity_to_api_type(c.client_type),
                api_key: c.api_key,
                web_origins: c.web_origins.unwrap_or_default(),
            })
            .collect())
    }

    async fn get_clients_paginated(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ModelClientType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<ModelClient>, i64), Error> {
        let team_id = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let ct = client_type.map(|t| self.map_api_to_entity_type(t));
        let (list, total) = self
            .repository
            .get_clients_paginated(team_id, name, enabled, ct, page_number, page_size)
            .await?;
        let clients = list
            .into_iter()
            .map(|c| ModelClient {
                id: ID::from(c.id.to_string()),
                team_id: ID::from(c.team_id.to_string()),
                environment_id: ID::from(c.environment_id.to_string()),
                name: c.name,
                description: c.description,
                enabled: c.enabled,
                client_type: self.map_entity_to_api_type(c.client_type),
                api_key: c.api_key,
                web_origins: c.web_origins.unwrap_or_default(),
            })
            .collect();
        Ok((clients, total))
    }

    async fn get_clients_with_offset(
        &self,
        team_id: ID,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ModelClientType>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<ModelClient>, i64), Error> {
        let team_id = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let ct = client_type.map(|t| self.map_api_to_entity_type(t));
        let (list, total) = self
            .repository
            .get_clients_with_offset(team_id, name, enabled, ct, offset, limit)
            .await?;
        let clients = list
            .into_iter()
            .map(|c| ModelClient {
                id: ID::from(c.id.to_string()),
                team_id: ID::from(c.team_id.to_string()),
                environment_id: ID::from(c.environment_id.to_string()),
                name: c.name,
                description: c.description,
                enabled: c.enabled,
                client_type: self.map_entity_to_api_type(c.client_type),
                api_key: c.api_key,
                web_origins: c.web_origins.unwrap_or_default(),
            })
            .collect();
        Ok((clients, total))
    }

    async fn create_client(
        &self,
        team_id: ID,
        input: CreateClientInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ModelClient, Error> {
        let team_id = Uuid::parse_str(&team_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let environment_id = Uuid::parse_str(&input.environment_id.to_string())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        // Validation rules
        match input.client_type {
            ModelClientType::Web => {
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
            ModelClientType::Backend => {
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
            name: input.name.clone(),
            description: input.description,
            enabled: input.enabled.unwrap_or(true),
            client_type: self.map_api_to_entity_type(input.client_type),
            web_origins: input.web_origins,
            environment_id,
        };
        let c = self.repository.create_client(team_id, create).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_client_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::CLIENT_CREATED,
            &c.id.to_string(),
            actor_id,
            actor_name,
            format!("Created client '{}'", c.name),
            Some(serde_json::json!({
                "client_id": c.id.to_string(),
                "client_name": c.name.clone(),
                "team_id": c.team_id.to_string(),
                "enabled": c.enabled,
            })),
        )
        .await;

        Ok(ModelClient {
            id: ID::from(c.id.to_string()),
            team_id: ID::from(c.team_id.to_string()),
            environment_id: ID::from(c.environment_id.to_string()),
            name: c.name,
            description: c.description,
            enabled: c.enabled,
            client_type: self.map_entity_to_api_type(c.client_type),
            api_key: c.api_key,
            web_origins: c.web_origins.unwrap_or_default(),
        })
    }

    async fn update_client(
        &self,
        id: ID,
        input: UpdateClientInput,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<ModelClient, Error> {
        let id =
            Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))?;

        if let Some(ct) = input.client_type {
            match ct {
                ModelClientType::Web => {
                    if let Some(origins) = &input.web_origins
                        && origins.is_empty()
                    {
                        return Err(Error::InvalidInput(
                            "Web client must specify at least one web origin".into(),
                        ));
                    }
                }
                ModelClientType::Backend => {
                    if let Some(origins) = &input.web_origins
                        && !origins.is_empty()
                    {
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
                .map(|t| self.map_api_to_entity_type(t)),
            web_origins: input.web_origins,
        };
        let c = self.repository.update_client(id, update).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let activity_type = if let Some(enabled) = input.enabled {
            if enabled {
                crate::utils::activity_logger::activity_types::CLIENT_ENABLED
            } else {
                crate::utils::activity_logger::activity_types::CLIENT_DISABLED
            }
        } else {
            crate::utils::activity_logger::activity_types::CLIENT_UPDATED
        };

        let _ = crate::utils::activity_logger::log_client_activity(
            &self.activity_log_repository,
            activity_type,
            &c.id.to_string(),
            actor_id,
            actor_name,
            format!("Updated client '{}'", c.name),
            Some(serde_json::json!({
                "client_id": c.id.to_string(),
                "client_name": c.name.clone(),
                "enabled": c.enabled,
            })),
        )
        .await;

        Ok(ModelClient {
            id: ID::from(c.id.to_string()),
            team_id: ID::from(c.team_id.to_string()),
            environment_id: ID::from(c.environment_id.to_string()),
            name: c.name,
            description: c.description,
            enabled: c.enabled,
            client_type: self.map_entity_to_api_type(c.client_type),
            api_key: c.api_key,
            web_origins: c.web_origins.unwrap_or_default(),
        })
    }

    async fn delete_client(
        &self,
        id: ID,
        actor: Option<crate::logic::ActorContext>,
    ) -> Result<(), Error> {
        let id =
            Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))?;

        // Get client name before deletion for activity log
        let client = self.repository.get_client_by_id(id).await?;

        self.repository.delete_client(id).await?;

        // Extract actor information
        let (actor_id, actor_name) = actor
            .as_ref()
            .map(|a| a.as_option())
            .unwrap_or((None, None));

        // Log activity (ignore errors to not fail the operation)
        let _ = crate::utils::activity_logger::log_client_activity(
            &self.activity_log_repository,
            crate::utils::activity_logger::activity_types::CLIENT_DELETED,
            &id.to_string(),
            actor_id,
            actor_name,
            format!("Deleted client '{}'", client.name),
            Some(serde_json::json!({
                "client_id": id.to_string(),
                "client_name": client.name,
            })),
        )
        .await;

        Ok(())
    }

    async fn count_clients(
        &self,
        team_id: Option<ID>,
        enabled: Option<bool>,
    ) -> Result<i64, Error> {
        let team_uuid = team_id
            .map(|id| {
                Uuid::parse_str(&id.to_string()).map_err(|e| Error::InvalidInput(e.to_string()))
            })
            .transpose()?;
        self.repository.count_clients(team_uuid, enabled).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::{ActivityLogRepository, MockActivityLogRepository};
    use crate::database::client::MockClientRepository;
    use crate::database::entity::Client as EntityClient;

    fn create_mock_activity_log() -> Box<dyn ActivityLogRepository> {
        let mut mock = MockActivityLogRepository::new();
        mock.expect_create_activity().returning(|_| {
            Ok(crate::database::activity_log::ActivityLogRow {
                id: uuid::Uuid::new_v4(),
                activity_type: "TEST".to_string(),
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

    #[test]
    fn test_type_mapping() {
        let (web_e, be_e) = (EntityClientType::Web, EntityClientType::Backend);
        let (web_m, be_m) = (ModelClientType::Web, ModelClientType::Backend);
        let logic = super::client_logic(
            Box::new(MockClientRepository::new()),
            create_mock_activity_log(),
        );
        assert!(matches!(
            logic.map_entity_to_api_type(web_e),
            ModelClientType::Web
        ));
        assert!(matches!(
            logic.map_entity_to_api_type(be_e),
            ModelClientType::Backend
        ));
        assert!(matches!(
            logic.map_api_to_entity_type(web_m),
            EntityClientType::Web
        ));
        assert!(matches!(
            logic.map_api_to_entity_type(be_m),
            EntityClientType::Backend
        ));
    }

    #[tokio::test]
    async fn test_create_client_web_requires_origins() {
        let logic = super::client_logic(
            Box::new(MockClientRepository::new()),
            create_mock_activity_log(),
        );
        let input = CreateClientInput {
            name: "WebC".into(),
            description: None,
            enabled: Some(true),
            client_type: ModelClientType::Web,
            web_origins: Some(vec![]),
            environment_id: ID::from(Uuid::new_v4()),
        };
        let res = logic
            .create_client(ID::from(Uuid::new_v4()), input, None)
            .await;
        assert!(matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Web client must")));
    }

    #[tokio::test]
    async fn test_create_client_backend_cannot_have_origins() {
        let logic = super::client_logic(
            Box::new(MockClientRepository::new()),
            create_mock_activity_log(),
        );
        let input = CreateClientInput {
            name: "BackendC".into(),
            description: None,
            enabled: Some(true),
            client_type: ModelClientType::Backend,
            web_origins: Some(vec!["https://x".into()]),
            environment_id: ID::from(Uuid::new_v4()),
        };
        let res = logic
            .create_client(ID::from(Uuid::new_v4()), input, None)
            .await;
        assert!(
            matches!(res, Err(Error::InvalidInput(msg)) if msg.contains("Backend client cannot"))
        );
    }

    #[tokio::test]
    async fn test_update_client_backend_with_origins_fails() {
        let logic = super::client_logic(
            Box::new(MockClientRepository::new()),
            create_mock_activity_log(),
        );
        let input = UpdateClientInput {
            name: None,
            description: None,
            enabled: None,
            client_type: Some(ModelClientType::Backend),
            web_origins: Some(vec!["https://x".into()]),
        };
        let res = logic
            .update_client(ID::from(Uuid::new_v4()), input, None)
            .await;
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
                tid.to_string() == team_id_str
                    && matches!(ci.client_type, EntityClientType::Web)
                    && ci.environment_id != Uuid::nil()
            })
            .times(1)
            .returning(|tid, _| {
                Ok(EntityClient {
                    id: Uuid::new_v4(),
                    team_id: tid,
                    environment_id: Uuid::new_v4(),
                    name: "n".into(),
                    description: None,
                    enabled: true,
                    client_type: EntityClientType::Web,
                    api_key: "K".into(),
                    web_origins: Some(vec!["https://a".into()]),
                })
            });
        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let input = CreateClientInput {
            name: "n".into(),
            description: None,
            enabled: Some(true),
            client_type: ModelClientType::Web,
            web_origins: Some(vec!["https://a".into()]),
            environment_id: ID::from(Uuid::new_v4()),
        };
        let out = logic
            .create_client(ID::from(team_id), input, None)
            .await
            .unwrap();
        assert_eq!(out.client_type, ModelClientType::Web);
        assert_eq!(out.web_origins.len(), 1);
    }

    #[tokio::test]
    async fn test_get_clients_paginated_success() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();
        let client1_id = Uuid::new_v4();
        let client2_id = Uuid::new_v4();

        let expected_clients = vec![
            EntityClient {
                id: client1_id,
                team_id,
                environment_id: Uuid::new_v4(),
                name: "Client 1".into(),
                description: Some("First client".into()),
                enabled: true,
                client_type: EntityClientType::Web,
                api_key: "api_key_1".into(),
                web_origins: Some(vec!["https://example1.com".into()]),
            },
            EntityClient {
                id: client2_id,
                team_id,
                environment_id: Uuid::new_v4(),
                name: "Client 2".into(),
                description: Some("Second client".into()),
                enabled: false,
                client_type: EntityClientType::Backend,
                api_key: "api_key_2".into(),
                web_origins: None,
            },
        ];

        repo.expect_get_clients_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(None::<EntityClientType>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(move |_, _, _, _, _, _| Ok((expected_clients.clone(), 25)));

        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let (clients, total) = logic
            .get_clients_paginated(ID::from(team_id), None, None, None, 1, 10)
            .await
            .unwrap();

        assert_eq!(clients.len(), 2);
        assert_eq!(total, 25);
        assert_eq!(clients[0].name, "Client 1");
        assert_eq!(clients[0].client_type, ModelClientType::Web);
        assert_eq!(clients[0].web_origins.len(), 1);
        assert_eq!(clients[1].name, "Client 2");
        assert_eq!(clients[1].client_type, ModelClientType::Backend);
        assert_eq!(clients[1].web_origins.len(), 0);
    }

    #[tokio::test]
    async fn test_get_clients_paginated_with_filters() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();

        repo.expect_get_clients_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(Some("Test".to_string())),
                mockall::predicate::eq(Some(true)),
                mockall::predicate::eq(Some(EntityClientType::Web)),
                mockall::predicate::eq(2),
                mockall::predicate::eq(5),
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok((vec![], 0)));

        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let (clients, total) = logic
            .get_clients_paginated(
                ID::from(team_id),
                Some("Test".to_string()),
                Some(true),
                Some(ModelClientType::Web),
                2,
                5,
            )
            .await
            .unwrap();

        assert_eq!(clients.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_clients_paginated_invalid_team_id() {
        let repo = MockClientRepository::new();
        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());

        let result = logic
            .get_clients_paginated(ID::from("invalid-uuid"), None, None, None, 1, 10)
            .await;

        assert!(matches!(result, Err(Error::InvalidInput(_))));
    }

    #[tokio::test]
    async fn test_get_clients_paginated_edge_cases() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();

        // Test with page_number = 0 (passed through as-is)
        repo.expect_get_clients_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(None::<EntityClientType>),
                mockall::predicate::eq(0), // Passed through as-is
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok((vec![], 0)));

        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let (clients, total) = logic
            .get_clients_paginated(
                ID::from(team_id),
                None,
                None,
                None,
                0, // Edge case: page 0
                10,
            )
            .await
            .unwrap();

        assert_eq!(clients.len(), 0);
        assert_eq!(total, 0);
    }
    #[tokio::test]
    async fn test_get_clients_paginated_negative_values() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();

        // Test with negative page_number (passed through as-is)
        repo.expect_get_clients_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(None::<EntityClientType>),
                mockall::predicate::eq(-1), // Passed through as-is
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok((vec![], 0)));

        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let (clients, total) = logic
            .get_clients_paginated(
                ID::from(team_id),
                None,
                None,
                None,
                -1, // Edge case: negative page
                10,
            )
            .await
            .unwrap();

        assert_eq!(clients.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_clients_paginated_large_page_size() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();

        repo.expect_get_clients_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(None::<EntityClientType>),
                mockall::predicate::eq(1),
                mockall::predicate::eq(i32::MAX), // Very large page size
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok((vec![], 0)));

        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let (clients, total) = logic
            .get_clients_paginated(
                ID::from(team_id),
                None,
                None,
                None,
                1,
                i32::MAX, // Edge case: maximum page size
            )
            .await
            .unwrap();

        assert_eq!(clients.len(), 0);
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_clients_paginated_empty_results_large_page() {
        let mut repo = MockClientRepository::new();
        let team_id = Uuid::new_v4();

        // Simulate requesting a page far beyond available data
        repo.expect_get_clients_paginated()
            .with(
                mockall::predicate::eq(team_id),
                mockall::predicate::eq(None::<String>),
                mockall::predicate::eq(None::<bool>),
                mockall::predicate::eq(None::<EntityClientType>),
                mockall::predicate::eq(999),
                mockall::predicate::eq(10),
            )
            .times(1)
            .returning(|_, _, _, _, _, _| Ok((vec![], 100))); // Total is 100, but page 999 has no results

        let logic = super::client_logic(Box::new(repo), create_mock_activity_log());
        let (clients, total) = logic
            .get_clients_paginated(
                ID::from(team_id),
                None,
                None,
                None,
                999, // Page far beyond data
                10,
            )
            .await
            .unwrap();

        assert_eq!(clients.len(), 0);
        assert_eq!(total, 100); // Total should still be accurate
    }
}
