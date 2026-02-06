use actix_web::{get, patch, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::model::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::model::{
    Client as ModelClient, ClientType as ModelClientType, CreateClientInput, UpdateClientInput,
};
use crate::database::activity_log::ActivityLogRepository;
use crate::database::client::client_repository_tx;
use crate::logic::client::ClientLogic;
use crate::logic::ActorContext;
use crate::rest::error::RestError;
use crate::rest::pagination::{normalize_pagination, PageMeta, PaginationQuery};
use crate::JwtUser;

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientType {
    Web,
    Backend,
}

impl From<ModelClientType> for ClientType {
    fn from(value: ModelClientType) -> Self {
        match value {
            ModelClientType::Web => ClientType::Web,
            ModelClientType::Backend => ClientType::Backend,
        }
    }
}

impl From<ClientType> for ModelClientType {
    fn from(value: ClientType) -> Self {
        match value {
            ClientType::Web => ModelClientType::Web,
            ClientType::Backend => ModelClientType::Backend,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientListQuery {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: Option<ClientType>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientResponse {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub client_type: ClientType,
    pub api_key: String,
    pub web_origins: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ClientsResponse {
    pub items: Vec<ClientResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateClientRequest {
    pub name: String,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: ClientType,
    pub web_origins: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateClientRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub enabled: Option<bool>,
    pub client_type: Option<ClientType>,
    pub web_origins: Option<Vec<String>>,
}

impl From<ModelClient> for ClientResponse {
    fn from(client: ModelClient) -> Self {
        Self {
            id: client.id.to_string(),
            team_id: client.team_id.to_string(),
            name: client.name,
            description: client.description,
            enabled: client.enabled,
            client_type: ClientType::from(client.client_type),
            api_key: client.api_key,
            web_origins: client.web_origins,
        }
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

fn validate_client_name(name: &str) -> Result<(), RestError> {
    let trimmed = name.trim();
    if trimmed.len() < 3 || trimmed.len() > 100 {
        return Err(RestError::invalid_input(
            "Client name must be between 3 and 100 characters",
        ));
    }
    Ok(())
}

fn validate_client_description(description: &Option<String>) -> Result<(), RestError> {
    if let Some(desc) = description {
        if desc.len() > 500 {
            return Err(RestError::invalid_input(
                "Client description must be 500 characters or fewer",
            ));
        }
    }
    Ok(())
}

fn validate_web_origins_for_create(
    client_type: ClientType,
    web_origins: &Option<Vec<String>>,
) -> Result<(), RestError> {
    match client_type {
        ClientType::Web => {
            if web_origins.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
                return Err(RestError::invalid_input(
                    "Web client must specify at least one web origin",
                ));
            }
        }
        ClientType::Backend => {
            if let Some(origins) = web_origins
                && !origins.is_empty()
            {
                return Err(RestError::invalid_input(
                    "Backend client cannot have web origins",
                ));
            }
        }
    }
    Ok(())
}

fn validate_web_origins_for_update(
    client_type: Option<ClientType>,
    web_origins: &Option<Vec<String>>,
) -> Result<(), RestError> {
    if let Some(ct) = client_type {
        match ct {
            ClientType::Web => {
                if let Some(origins) = web_origins
                    && origins.is_empty()
                {
                    return Err(RestError::invalid_input(
                        "Web client must specify at least one web origin",
                    ));
                }
            }
            ClientType::Backend => {
                if let Some(origins) = web_origins
                    && !origins.is_empty()
                {
                    return Err(RestError::invalid_input(
                        "Backend client cannot have web origins",
                    ));
                }
            }
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/clients",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("name" = Option<String>, Query, description = "Filter by client name"),
        ("enabled" = Option<bool>, Query, description = "Filter by enabled status"),
        ("clientType" = Option<ClientType>, Query, description = "Filter by client type"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Clients list", body = ClientsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Clients"
)]
#[get("/teams/{team_id}/clients")]
pub(crate) async fn list_clients(
    logic: web::Data<Box<dyn ClientLogic>>,
    team_id: web::Path<String>,
    query: web::Query<ClientListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let name = query
        .name
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let client_type = query.client_type.map(ModelClientType::from);

    let (items, total) = logic
        .get_clients_with_offset(ID::from(team_uuid), name, query.enabled, client_type, offset, limit)
        .await
        .map_err(RestError::from)?;

    let response = ClientsResponse {
        items: items.into_iter().map(ClientResponse::from).collect(),
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    };

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    get,
    path = "/api/v1/clients/{id}",
    params(
        ("id" = String, Path, description = "Client ID")
    ),
    responses(
        (status = 200, description = "Client", body = ClientResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Clients"
)]
#[get("/clients/{id}")]
pub(crate) async fn get_client(
    logic: web::Data<Box<dyn ClientLogic>>,
    client_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let client_uuid = parse_uuid(&client_id, "client id")?;
    let client = logic
        .get_client_by_id(ID::from(client_uuid))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(ClientResponse::from(client)))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/clients",
    request_body = CreateClientRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Client created", body = ClientResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Clients"
)]
#[post("/teams/{team_id}/clients")]
pub(crate) async fn create_client(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<CreateClientRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let actor = actor_from_request(&req);

    validate_client_name(&payload.name)?;
    validate_client_description(&payload.description)?;
    validate_web_origins_for_create(payload.client_type, &payload.web_origins)?;

    let input = CreateClientInput {
        name: payload.name.clone(),
        description: payload.description.clone(),
        enabled: payload.enabled,
        client_type: payload.client_type.into(),
        web_origins: payload.web_origins.clone(),
    };

    let repo = client_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::client_tx::create_client_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(team_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(client) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Created().json(ClientResponse::from(client)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/clients/{id}",
    request_body = UpdateClientRequest,
    params(
        ("id" = String, Path, description = "Client ID")
    ),
    responses(
        (status = 200, description = "Client updated", body = ClientResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Clients"
)]
#[patch("/clients/{id}")]
pub(crate) async fn update_client(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    client_id: web::Path<String>,
    payload: web::Json<UpdateClientRequest>,
) -> Result<impl Responder, RestError> {
    let client_uuid = parse_uuid(&client_id, "client id")?;
    let actor = actor_from_request(&req);

    if let Some(name) = payload.name.as_deref() {
        validate_client_name(name)?;
    }
    validate_client_description(&payload.description)?;
    validate_web_origins_for_update(payload.client_type, &payload.web_origins)?;

    let input = UpdateClientInput {
        name: payload.name.clone(),
        description: payload.description.clone(),
        enabled: payload.enabled,
        client_type: payload.client_type.map(ModelClientType::from),
        web_origins: payload.web_origins.clone(),
    };

    let repo = client_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::client_tx::update_client_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(client_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(client) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            Ok(HttpResponse::Ok().json(ClientResponse::from(client)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_clients)
        .service(get_client)
        .service(create_client)
        .service(update_client);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};

    use crate::database::activity_log::PgActivityLogRepository;
    use crate::logic::client::MockClientLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_client(client_id: Uuid, team_id: Uuid) -> ModelClient {
        ModelClient {
            id: ID::from(client_id),
            team_id: ID::from(team_id),
            name: "Web Client".to_string(),
            description: Some("Test".to_string()),
            enabled: true,
            client_type: ModelClientType::Web,
            api_key: "api_key".to_string(),
            web_origins: vec!["https://example.com".to_string()],
        }
    }

    async fn test_pool() -> sqlx::PgPool {
        let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
        PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("Failed to connect to database")
    }

    async fn insert_team(pool: &sqlx::PgPool) -> Uuid {
        let team_id = Uuid::new_v4();
        let name = format!("client-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "client test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_client(pool: &sqlx::PgPool, team_id: Uuid, name: &str) -> Uuid {
        let client_id = Uuid::new_v4();
        let api_key = format!("api-{}", client_id);
        sqlx::query!(
            r#"INSERT INTO clients (id, team_id, name, description, enabled, client_type, api_key)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            client_id,
            team_id,
            name,
            Some("Test".to_string()),
            true,
            "Web",
            api_key
        )
        .execute(pool)
        .await
        .expect("Failed to insert client");
        client_id
    }

    #[actix_web::test]
    async fn list_clients_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let client_id = Uuid::new_v4();
        let client = sample_client(client_id, team_id);
        let client_clone = client.clone();

        let mut mock_logic = MockClientLogic::new();
        mock_logic
            .expect_get_clients_with_offset()
            .withf(move |id, name, enabled, client_type, offset, limit| {
                id.to_string() == team_id.to_string()
                    && name.as_deref() == Some("client")
                    && *enabled == Some(true)
                    && matches!(client_type, Some(ModelClientType::Web))
                    && *offset == 0
                    && *limit == 10
            })
            .times(1)
            .returning(move |_, _, _, _, _, _| Ok((vec![client_clone.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn ClientLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!(
            "/api/v1/teams/{team_id}/clients?offset=0&limit=10&name=client&enabled=true&clientType=WEB"
        );
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], client_id.to_string());
        assert_eq!(json["meta"]["offset"], 0);
        assert_eq!(json["meta"]["limit"], 10);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn get_client_returns_client() {
        let team_id = Uuid::new_v4();
        let client_id = Uuid::new_v4();
        let client = sample_client(client_id, team_id);
        let client_clone = client.clone();

        let mut mock_logic = MockClientLogic::new();
        mock_logic
            .expect_get_client_by_id()
            .withf(move |id| id.to_string() == client_id.to_string())
            .times(1)
            .returning(move |_| Ok(client_clone.clone()));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn ClientLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/clients/{client_id}");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], client_id.to_string());
        assert_eq!(json["clientType"], "WEB");
    }

    #[actix_web::test]
    async fn create_client_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/clients");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateClientRequest {
                name: "Web Client".to_string(),
                description: Some("Test".to_string()),
                enabled: Some(true),
                client_type: ClientType::Web,
                web_origins: Some(vec!["https://example.com".to_string()]),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[actix_web::test]
    async fn create_client_missing_web_origins_returns_bad_request() {
        let team_id = Uuid::new_v4();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/feature_toggle")
            .unwrap();
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/clients");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateClientRequest {
                name: "Web Client".to_string(),
                description: None,
                enabled: Some(true),
                client_type: ClientType::Web,
                web_origins: Some(vec![]),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "invalid_input");
        assert_eq!(json["message"], "Web client must specify at least one web origin");
    }

    #[actix_web::test]
    async fn create_client_conflict_returns_conflict() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let _client_id = insert_client(&pool, team_id, "Web Client").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/clients");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateClientRequest {
                name: "Web Client".to_string(),
                description: None,
                enabled: Some(true),
                client_type: ClientType::Web,
                web_origins: Some(vec!["https://example.com".to_string()]),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
    }

    #[actix_web::test]
    async fn update_client_returns_updated() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let client_id = insert_client(&pool, team_id, "Web Client").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/clients/{client_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateClientRequest {
                name: Some("Updated Client".to_string()),
                description: None,
                enabled: Some(false),
                client_type: None,
                web_origins: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn update_client_backend_with_origins_returns_bad_request() {
        let client_id = Uuid::new_v4();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/feature_toggle")
            .unwrap();
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/clients/{client_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateClientRequest {
                name: None,
                description: None,
                enabled: None,
                client_type: Some(ClientType::Backend),
                web_origins: Some(vec!["https://example.com".to_string()]),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "invalid_input");
        assert_eq!(json["message"], "Backend client cannot have web origins");
    }
}
