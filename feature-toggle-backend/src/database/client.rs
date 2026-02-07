use crate::database::entity::{Client, ClientType};
use crate::database::{Error, handle_error};
use mockall::automock;
use rand::{Rng, distr::Alphanumeric};
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row};
use std::collections::HashMap;
use uuid::Uuid;

pub struct CreateClient {
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub client_type: ClientType,
    pub web_origins: Option<Vec<String>>, // Only for Web
    pub environment_id: Uuid,
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
    async fn get_clients_paginated(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Client>, i64), Error>;
    async fn get_clients_with_offset(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Client>, i64), Error>;
    async fn create_client(&self, team_id: Uuid, input: CreateClient) -> Result<Client, Error>;
    async fn update_client(&self, id: Uuid, input: UpdateClient) -> Result<Client, Error>;
    async fn delete_client(&self, id: Uuid) -> Result<(), Error>;

    // Count clients (for dashboard metrics)
    async fn count_clients(
        &self,
        team_id: Option<Uuid>,
        enabled: Option<bool>,
    ) -> Result<i64, Error>;

    fn clone_box(&self) -> Box<dyn ClientRepository>;
}

impl Clone for Box<dyn ClientRepository> {
    fn clone(&self) -> Box<dyn ClientRepository> {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait ClientRepositoryTx: ClientRepository {
    async fn create_client_tx(
        &self,
        conn: &mut PgConnection,
        team_id: Uuid,
        input: CreateClient,
    ) -> Result<Client, Error>;
    async fn update_client_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateClient,
    ) -> Result<Client, Error>;
    async fn delete_client_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error>;
}

pub fn client_repository(pool: PgPool) -> Box<dyn ClientRepository> {
    Box::new(ClientRepositoryImpl::new(pool))
}

/// Returns a repository that also implements ClientRepositoryTx for transaction support.
pub fn client_repository_tx(pool: PgPool) -> ClientRepositoryImpl {
    ClientRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct ClientRepositoryImpl {
    pool: PgPool,
}

#[derive(Debug, Clone)]
struct ClientBaseRow {
    id: Uuid,
    team_id: Uuid,
    environment_id: Uuid,
    name: String,
    description: Option<String>,
    enabled: bool,
    client_type: ClientType,
    api_key: String,
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

    async fn load_web_origins_batch(
        &self,
        client_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<String>>, Error> {
        if client_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let result = sqlx::query!(
            r#"SELECT client_id, origin
               FROM client_web_origins
               WHERE client_id = ANY($1)
               ORDER BY client_id, origin"#,
            client_ids
        )
        .fetch_all(&self.pool)
        .await;
        let rows = handle_error(None, result)?;
        let mut origins_by_client: HashMap<Uuid, Vec<String>> = HashMap::new();
        for row in rows {
            origins_by_client
                .entry(row.client_id)
                .or_default()
                .push(row.origin);
        }
        Ok(origins_by_client)
    }

    fn parse_client_base_row(row: &sqlx::postgres::PgRow) -> ClientBaseRow {
        let client_type_str: String = row.get("client_type");
        ClientBaseRow {
            id: row.get("id"),
            team_id: row.get("team_id"),
            environment_id: row.get("environment_id"),
            name: row.get("name"),
            description: row.get("description"),
            enabled: row.get::<bool, _>("enabled"),
            client_type: Self::from_type_str(&client_type_str),
            api_key: row.get("api_key"),
        }
    }

    async fn map_client_rows(&self, base_rows: Vec<ClientBaseRow>) -> Result<Vec<Client>, Error> {
        let web_client_ids: Vec<Uuid> = base_rows
            .iter()
            .filter(|row| matches!(row.client_type, ClientType::Web))
            .map(|row| row.id)
            .collect();
        let web_origins_by_client = self.load_web_origins_batch(&web_client_ids).await?;

        Ok(base_rows
            .into_iter()
            .map(|row| {
                let web_origins = if matches!(row.client_type, ClientType::Web) {
                    Some(
                        web_origins_by_client
                            .get(&row.id)
                            .cloned()
                            .unwrap_or_default(),
                    )
                } else {
                    None
                };

                Client {
                    id: row.id,
                    team_id: row.team_id,
                    environment_id: row.environment_id,
                    name: row.name,
                    description: row.description,
                    enabled: row.enabled,
                    client_type: row.client_type,
                    api_key: row.api_key,
                    web_origins,
                }
            })
            .collect())
    }

    async fn count_filtered_clients(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
    ) -> Result<i64, Error> {
        let mut count_qb =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM clients WHERE team_id = ");
        count_qb.push_bind(team_id);

        if let Some(filter_name) = name {
            count_qb
                .push(" AND name ILIKE ")
                .push_bind(format!("%{}%", filter_name));
        }
        if let Some(enabled_value) = enabled {
            count_qb.push(" AND enabled = ").push_bind(enabled_value);
        }
        if let Some(ct) = client_type {
            count_qb
                .push(" AND client_type = ")
                .push_bind(Self::to_type_str(&ct));
        }

        let count: i64 = count_qb
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?;
        Ok(count)
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
            r#"SELECT id, team_id, environment_id, name, description, enabled, client_type, api_key
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
            environment_id: row.environment_id,
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
            "SELECT id, team_id, environment_id, name, description, enabled, client_type, api_key FROM clients WHERE team_id = ",
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
        let base_rows = rows
            .iter()
            .map(Self::parse_client_base_row)
            .collect::<Vec<_>>();
        let clients = self.map_client_rows(base_rows).await?;
        Ok(clients)
    }

    async fn get_clients_paginated(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
        page_number: i32,
        page_size: i32,
    ) -> Result<(Vec<Client>, i64), Error> {
        if page_size <= 0 {
            let total_count = self
                .count_filtered_clients(team_id, name.clone(), enabled, client_type.clone())
                .await?;
            return Ok((Vec::new(), total_count));
        }

        let page = if page_number < 1 { 1 } else { page_number } as i64;
        let size = page_size as i64;
        let offset = (page - 1) * size;
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT id, team_id, environment_id, name, description, enabled, client_type, api_key, COUNT(*) OVER() as total_count FROM clients WHERE team_id = ",
        );
        qb.push_bind(team_id);

        let name_for_query = name.clone();
        let client_type_for_query = client_type.clone();

        if let Some(filter_name) = name_for_query {
            qb.push(" AND name ILIKE ")
                .push_bind(format!("%{}%", filter_name));
        }
        if let Some(enabled_value) = enabled {
            qb.push(" AND enabled = ").push_bind(enabled_value);
        }
        if let Some(ct) = client_type_for_query {
            qb.push(" AND client_type = ")
                .push_bind(Self::to_type_str(&ct));
        }
        qb.push(" ORDER BY name");
        qb.push(" LIMIT ").push_bind(size);
        qb.push(" OFFSET ").push_bind(offset);

        let rows = qb.build().fetch_all(&self.pool).await;
        let rows = handle_error(None, rows)?;
        let total_count = if let Some(row) = rows.first() {
            row.get::<i64, _>("total_count")
        } else {
            self.count_filtered_clients(team_id, name, enabled, client_type)
                .await?
        };
        let base_rows = rows
            .iter()
            .map(Self::parse_client_base_row)
            .collect::<Vec<_>>();
        let clients = self.map_client_rows(base_rows).await?;
        Ok((clients, total_count))
    }

    async fn get_clients_with_offset(
        &self,
        team_id: Uuid,
        name: Option<String>,
        enabled: Option<bool>,
        client_type: Option<ClientType>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<Client>, i64), Error> {
        let offset = offset.max(0);
        let limit = limit.max(1);

        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT id, team_id, environment_id, name, description, enabled, client_type, api_key, COUNT(*) OVER() as total_count FROM clients WHERE team_id = ",
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
        qb.push(" LIMIT ").push_bind(limit);
        qb.push(" OFFSET ").push_bind(offset);

        let rows = qb.build().fetch_all(&self.pool).await;
        let rows = handle_error(None, rows)?;
        let total_count = rows
            .first()
            .map(|row| row.get::<i64, _>("total_count"))
            .unwrap_or(0);
        let base_rows = rows
            .iter()
            .map(Self::parse_client_base_row)
            .collect::<Vec<_>>();
        let clients = self.map_client_rows(base_rows).await?;

        Ok((clients, total_count))
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
            r#"INSERT INTO clients (id, team_id, environment_id, name, description, enabled, client_type, api_key)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               RETURNING id, team_id, environment_id, name, description, enabled, client_type, api_key"#,
            id,
            team_id,
            input.environment_id,
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
        if matches!(input.client_type, ClientType::Web)
            && let Some(origins) = input.web_origins.clone() {
                for origin in origins {
                    sqlx::query!(
                        r#"INSERT INTO client_web_origins (id, client_id, origin) VALUES ($1, $2, $3)"#,
                        Uuid::new_v4(),
                        row.id,
                        origin
                    )
                    .execute(&self.pool)
                    .await
                    .map_err(Error::DatabaseError)?;
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
            environment_id: row.environment_id,
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
               RETURNING id, team_id, environment_id, name, description, enabled, client_type, api_key"#,
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
            sqlx::query!("DELETE FROM client_web_origins WHERE client_id = $1", id)
                .execute(&self.pool)
                .await
                .map_err(Error::DatabaseError)?;
            for origin in origins {
                sqlx::query!(
                    r#"INSERT INTO client_web_origins (id, client_id, origin) VALUES ($1, $2, $3)"#,
                    Uuid::new_v4(),
                    id,
                    origin
                )
                .execute(&self.pool)
                .await
                .map_err(Error::DatabaseError)?;
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
            environment_id: row.environment_id,
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

    async fn count_clients(
        &self,
        team_id: Option<Uuid>,
        enabled: Option<bool>,
    ) -> Result<i64, Error> {
        // Build query dynamically based on filters
        let count = match (team_id, enabled) {
            (Some(team_id), Some(enabled)) => sqlx::query_scalar!(
                r#"
                    SELECT COUNT(*) as "count!"
                    FROM clients
                    WHERE team_id = $1 AND enabled = $2
                    "#,
                team_id,
                enabled
            )
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?,
            (Some(team_id), None) => sqlx::query_scalar!(
                r#"
                    SELECT COUNT(*) as "count!"
                    FROM clients
                    WHERE team_id = $1
                    "#,
                team_id
            )
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?,
            (None, Some(enabled)) => sqlx::query_scalar!(
                r#"
                    SELECT COUNT(*) as "count!"
                    FROM clients
                    WHERE enabled = $1
                    "#,
                enabled
            )
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?,
            (None, None) => sqlx::query_scalar!(
                r#"
                    SELECT COUNT(*) as "count!"
                    FROM clients
                    "#
            )
            .fetch_one(&self.pool)
            .await
            .map_err(Error::DatabaseError)?,
        };

        Ok(count)
    }

    fn clone_box(&self) -> Box<dyn ClientRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl ClientRepositoryTx for ClientRepositoryImpl {
    async fn create_client_tx(
        &self,
        conn: &mut PgConnection,
        team_id: Uuid,
        input: CreateClient,
    ) -> Result<Client, Error> {
        // Ensure unique name per team (read from pool)
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

        // Generate API key (uses pool for read)
        let api_key = self.generate_unique_api_key().await?;
        let id = Uuid::new_v4();
        let client_type_str = Self::to_type_str(&input.client_type);

        let result = sqlx::query!(
            r#"INSERT INTO clients (id, team_id, environment_id, name, description, enabled, client_type, api_key)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
               RETURNING id, team_id, environment_id, name, description, enabled, client_type, api_key"#,
            id,
            team_id,
            input.environment_id,
            input.name,
            input.description,
            input.enabled,
            client_type_str,
            api_key
        )
        .fetch_one(&mut *conn)
        .await;
        let row = handle_error(None, result)?;

        // Insert web origins if needed (within transaction)
        if matches!(input.client_type, ClientType::Web)
            && let Some(origins) = input.web_origins.clone() {
                for origin in origins {
                    sqlx::query!(
                        r#"INSERT INTO client_web_origins (id, client_id, origin) VALUES ($1, $2, $3)"#,
                        Uuid::new_v4(),
                        row.id,
                        origin
                    )
                    .execute(&mut *conn)
                    .await
                    .map_err(Error::DatabaseError)?;
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
            environment_id: row.environment_id,
            name: row.name,
            description: row.description,
            enabled: row.enabled,
            client_type: Self::from_type_str(&row.client_type),
            api_key: row.api_key,
            web_origins,
        })
    }

    async fn update_client_tx(
        &self,
        conn: &mut PgConnection,
        id: Uuid,
        input: UpdateClient,
    ) -> Result<Client, Error> {
        let existing = self.get_client_by_id(id).await?;
        let updated_type = input
            .client_type
            .clone()
            .unwrap_or(existing.client_type.clone());
        let client_type_str = Self::to_type_str(&updated_type);

        let result = sqlx::query!(
            r#"UPDATE clients SET name = $1, description = $2, enabled = $3, client_type = $4 WHERE id = $5
               RETURNING id, team_id, environment_id, name, description, enabled, client_type, api_key"#,
            input.name.clone().unwrap_or(existing.name),
            input.description.clone().or(existing.description),
            input.enabled.unwrap_or(existing.enabled),
            client_type_str,
            id
        )
        .fetch_one(&mut *conn)
        .await;
        let row = handle_error(Some(id), result)?;

        // Update web origins: replace set if provided (within transaction)
        if let Some(origins) = input.web_origins.clone() {
            // wipe then insert
            sqlx::query!("DELETE FROM client_web_origins WHERE client_id = $1", id)
                .execute(&mut *conn)
                .await
                .map_err(Error::DatabaseError)?;
            for origin in origins {
                sqlx::query!(
                    r#"INSERT INTO client_web_origins (id, client_id, origin) VALUES ($1, $2, $3)"#,
                    Uuid::new_v4(),
                    id,
                    origin
                )
                .execute(&mut *conn)
                .await
                .map_err(Error::DatabaseError)?;
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
            environment_id: row.environment_id,
            name: row.name,
            description: row.description,
            enabled: row.enabled,
            client_type,
            api_key: row.api_key,
            web_origins,
        })
    }

    async fn delete_client_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        // ensure exists
        self.get_client_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM clients WHERE id = $1", id)
            .execute(&mut *conn)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }
}
