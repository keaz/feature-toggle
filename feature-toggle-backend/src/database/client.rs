use crate::database::entity::{Client, ClientType};
use crate::database::{Error, handle_error};
use mockall::automock;
use rand::{Rng, distr::Alphanumeric};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

pub struct CreateClient {
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub client_type: ClientType,
    pub web_origins: Option<Vec<String>>, // Only for Web
}

pub struct UpdateClient {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: Option<ClientType>,
    pub web_origins: Option<Vec<String>>, // Only for Web
}

#[automock]
#[async_trait::async_trait]
pub trait ClientRepository: Send + Sync {
    async fn get_client_by_id(&self, id: Uuid) -> Result<Client, Error>;
    async fn get_clients(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
    ) -> Result<Vec<Client>, Error>;
    async fn create_client(&self, team_id: Uuid, input: CreateClient) -> Result<Client, Error>;
    async fn update_client(&self, id: Uuid, input: UpdateClient) -> Result<Client, Error>;
    async fn delete_client(&self, id: Uuid) -> Result<(), Error>;
    fn clone_box(&self) -> Box<dyn ClientRepository>;
}

impl Clone for Box<dyn ClientRepository> {
    fn clone(&self) -> Box<dyn ClientRepository> {
        self.clone_box()
    }
}

pub fn client_repository(pool: PgPool) -> Box<dyn ClientRepository> {
    Box::new(ClientRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct ClientRepositoryImpl {
    pool: PgPool,
}

impl ClientRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn to_type_str(t: &ClientType) -> &'static str {
        match t {
            ClientType::Web => "Web",
            ClientType::Backend => "Backend",
        }
    }

    fn from_type_str(s: &str) -> ClientType {
        match s {
            "Web" => ClientType::Web,
            _ => ClientType::Backend,
        }
    }

    async fn load_web_origins(&self, client_id: Uuid) -> Result<Vec<String>, Error> {
        let result = sqlx::query!(
            r#"SELECT origin FROM client_web_origins WHERE client_id = $1 ORDER BY origin"#,
            client_id
        )
        .fetch_all(&self.pool)
        .await;
        let rows = handle_error(Some(client_id), result)?;
        Ok(rows.into_iter().map(|r| r.origin).collect())
    }

    async fn is_api_key_unique(&self, api_key: &str) -> Result<bool, Error> {
        let result = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM clients WHERE api_key = $1) AS exists"#,
            api_key
        )
        .fetch_one(&self.pool)
        .await;
        let exists: Option<bool> = handle_error(None, result)?;
        Ok(!exists.unwrap_or(false))
    }

    async fn generate_unique_api_key(&self) -> Result<String, Error> {
        // 48-length URL-safe key
        for _ in 0..10 {
            let api_key: String = rand::rng()
                .sample_iter(&Alphanumeric)
                .take(48)
                .map(char::from)
                .collect();
            if self.is_api_key_unique(&api_key).await? {
                return Ok(api_key);
            }
        }
        Err(Error::InvalidInput(
            "Failed to generate unique API key".into(),
        ))
    }
}

#[async_trait::async_trait]
impl ClientRepository for ClientRepositoryImpl {
    async fn get_client_by_id(&self, id: Uuid) -> Result<Client, Error> {
        let result = sqlx::query!(
            r#"SELECT id, team_id, name, description, enabled, client_type, api_key
               FROM clients WHERE id = $1"#,
            id
        )
        .fetch_one(&self.pool)
        .await;

        let row = handle_error(Some(id), result)?;
        let client_type = Self::from_type_str(&row.client_type);
        let web_origins = if matches!(client_type, ClientType::Web) {
            Some(self.load_web_origins(row.id).await?)
        } else {
            None
        };

        Ok(Client {
            id: row.id,
            team_id: row.team_id,
            name: row.name,
            description: row.description,
            enabled: row.enabled,
            client_type,
            api_key: row.api_key,
            web_origins,
        })
    }

    async fn get_clients(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
    ) -> Result<Vec<Client>, Error> {
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT id, team_id, name, description, enabled, client_type, api_key FROM clients WHERE team_id = ",
        );
        qb.push_bind(team_id);

        if let Some(filter_name) = name {
            qb.push(" AND name ILIKE ")
                .push_bind(format!("%{}%", filter_name));
        }
        if let Some(enabled_value) = enabled {
            qb.push(" AND enabled = ").push_bind(enabled_value);
        }
        if let Some(ct) = client_type {
            qb.push(" AND client_type = ")
                .push_bind(Self::to_type_str(&ct));
        }
        qb.push(" ORDER BY name");

        let rows = qb.build().fetch_all(&self.pool).await;

        let rows = handle_error(None, rows)?;

        // Map and fetch origins for web clients in a second pass
        let mut clients: Vec<Client> = Vec::with_capacity(rows.len());
        for row in rows {
            // row is PgRow; extract columns
            let id: Uuid = row.get("id");
            let team_id: Uuid = row.get("team_id");
            let name: String = row.get("name");
            let description: Option<String> = row.get("description");
            let enabled: bool = row.get::<bool, _>("enabled");
            let client_type_str: String = row.get("client_type");
            let api_key: String = row.get("api_key");
            let client_type = Self::from_type_str(&client_type_str);
            let web_origins = if matches!(client_type, ClientType::Web) {
                Some(self.load_web_origins(id).await?)
            } else {
                None
            };
            clients.push(Client {
                id,
                team_id,
                name,
                description,
                enabled,
                client_type,
                api_key,
                web_origins,
            });
        }
        Ok(clients)
    }

    async fn create_client(&self, team_id: Uuid, input: CreateClient) -> Result<Client, Error> {
        // Ensure unique name per team
        let existing = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM clients WHERE team_id = $1 AND name = $2) AS exists"#,
            team_id,
            input.name
        )
        .fetch_one(&self.pool)
        .await;
        let exists: Option<bool> = handle_error(None, existing)?;
        if exists.unwrap_or_default() {
            return Err(Error::RecordAlreadyExists(
                "Client with same name in team".into(),
            ));
        }

        // Generate API key
        let api_key = self.generate_unique_api_key().await?;
        let id = Uuid::new_v4();
        let client_type_str = Self::to_type_str(&input.client_type);

        let result = sqlx::query!(
            r#"INSERT INTO clients (id, team_id, name, description, enabled, client_type, api_key)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING id, team_id, name, description, enabled, client_type, api_key"#,
            id,
            team_id,
            input.name,
            input.description,
            input.enabled,
            client_type_str,
            api_key
        )
        .fetch_one(&self.pool)
        .await;
        let row = handle_error(None, result)?;

        // Insert web origins if needed
        if matches!(input.client_type, ClientType::Web) {
            if let Some(origins) = input.web_origins.clone() {
                for origin in origins {
                    let _ = sqlx::query!(
                        r#"INSERT INTO client_web_origins (id, client_id, origin) VALUES ($1, $2, $3)"#,
                        Uuid::new_v4(),
                        row.id,
                        origin
                    )
                    .execute(&self.pool)
                    .await;
                }
            }
        }

        let web_origins = if matches!(input.client_type, ClientType::Web) {
            Some(self.load_web_origins(row.id).await?)
        } else {
            None
        };

        Ok(Client {
            id: row.id,
            team_id: row.team_id,
            name: row.name,
            description: row.description,
            enabled: row.enabled,
            client_type: Self::from_type_str(&row.client_type),
            api_key: row.api_key,
            web_origins,
        })
    }

    async fn update_client(&self, id: Uuid, input: UpdateClient) -> Result<Client, Error> {
        let existing = self.get_client_by_id(id).await?;
        let updated_type = input
            .client_type
            .clone()
            .unwrap_or(existing.client_type.clone());
        let client_type_str = Self::to_type_str(&updated_type);

        let result = sqlx::query!(
            r#"UPDATE clients SET name = $1, description = $2, enabled = $3, client_type = $4 WHERE id = $5
               RETURNING id, team_id, name, description, enabled, client_type, api_key"#,
            input.name.clone().unwrap_or(existing.name),
            input.description.clone().or(existing.description),
            input.enabled.unwrap_or(existing.enabled),
            client_type_str,
            id
        )
        .fetch_one(&self.pool)
        .await;
        let row = handle_error(Some(id), result)?;

        // Update web origins: replace set if provided
        if let Some(origins) = input.web_origins.clone() {
            // wipe then insert
            let _ = sqlx::query!("DELETE FROM client_web_origins WHERE client_id = $1", id)
                .execute(&self.pool)
                .await;
            for origin in origins {
                let _ = sqlx::query!(
                    r#"INSERT INTO client_web_origins (id, client_id, origin) VALUES ($1, $2, $3)"#,
                    Uuid::new_v4(),
                    id,
                    origin
                )
                .execute(&self.pool)
                .await;
            }
        }

        let client_type = Self::from_type_str(&row.client_type);
        let web_origins = if matches!(client_type, ClientType::Web) {
            Some(self.load_web_origins(row.id).await?)
        } else {
            None
        };

        Ok(Client {
            id: row.id,
            team_id: row.team_id,
            name: row.name,
            description: row.description,
            enabled: row.enabled,
            client_type,
            api_key: row.api_key,
            web_origins,
        })
    }

    async fn delete_client(&self, id: Uuid) -> Result<(), Error> {
        // ensure exists
        self.get_client_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM clients WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn ClientRepository> {
        Box::new(self.clone())
    }
}
