use crate::model::ID;
use actix_web::{HttpResponse, Responder, delete, get, patch, post, web};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::context::context_repository_tx;
use crate::logic::context::ContextLogic;
use crate::model::{
    Context as ModelContext, ContextEntry as ModelContextEntry, CreateContextInput,
    UpdateContextInput,
};
use crate::rest::error::RestError;
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};

#[derive(Debug, Deserialize, ToSchema)]
pub struct ContextListQuery {
    pub key: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextEntryResponse {
    pub id: String,
    pub value: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextResponse {
    pub id: String,
    pub team_id: String,
    pub key: String,
    pub entries: Vec<ContextEntryResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContextsResponse {
    pub items: Vec<ContextResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextRequest {
    pub key: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContextRequest {
    pub key: Option<String>,
    pub entries: Option<Vec<String>>,
}

impl From<ModelContextEntry> for ContextEntryResponse {
    fn from(entry: ModelContextEntry) -> Self {
        Self {
            id: entry.id.to_string(),
            value: entry.value,
        }
    }
}

impl From<ModelContext> for ContextResponse {
    fn from(context: ModelContext) -> Self {
        Self {
            id: context.id.to_string(),
            team_id: context.team_id.to_string(),
            key: context.key,
            entries: context
                .entries
                .into_iter()
                .map(ContextEntryResponse::from)
                .collect(),
        }
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn validate_context_key(key: &str) -> Result<(), RestError> {
    if key.trim().is_empty() {
        return Err(RestError::invalid_input("Context key cannot be empty"));
    }
    Ok(())
}

fn validate_context_entries(entries: &[String]) -> Result<(), RestError> {
    let mut set = std::collections::HashSet::new();
    for value in entries {
        if !set.insert(value) {
            return Err(RestError::invalid_input("Duplicate context entry"));
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/contexts",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("key" = Option<String>, Query, description = "Filter by context key"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Contexts list", body = ContextsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Contexts"
)]
#[get("/teams/{team_id}/contexts")]
pub(crate) async fn list_contexts(
    logic: web::Data<Box<dyn ContextLogic>>,
    team_id: web::Path<String>,
    query: web::Query<ContextListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let key = query
        .key
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let (items, total) = logic
        .get_contexts_with_offset(ID::from(team_uuid), key, offset, limit)
        .await
        .map_err(RestError::from)?;

    let response = ContextsResponse {
        items: items.into_iter().map(ContextResponse::from).collect(),
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
    path = "/api/v1/contexts/{id}",
    params(
        ("id" = String, Path, description = "Context ID")
    ),
    responses(
        (status = 200, description = "Context", body = ContextResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Contexts"
)]
#[get("/contexts/{id}")]
pub(crate) async fn get_context(
    logic: web::Data<Box<dyn ContextLogic>>,
    ctx_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let ctx_uuid = parse_uuid(&ctx_id, "context id")?;
    let context = logic
        .get_context_by_id(ID::from(ctx_uuid))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(ContextResponse::from(context)))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/contexts",
    request_body = CreateContextRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Context created", body = ContextResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Contexts"
)]
#[post("/teams/{team_id}/contexts")]
pub(crate) async fn create_context(
    db_pool: web::Data<sqlx::PgPool>,
    team_id: web::Path<String>,
    payload: web::Json<CreateContextRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    validate_context_key(&payload.key)?;
    validate_context_entries(&payload.entries)?;

    let input = CreateContextInput {
        key: payload.key.clone(),
        entries: payload.entries.clone(),
    };

    let repo = context_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result =
        crate::logic::context_tx::create_context_in_tx(&mut tx, &repo, ID::from(team_uuid), input)
            .await;

    match result {
        Ok(context) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::Created().json(ContextResponse::from(context)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/contexts/{id}",
    request_body = UpdateContextRequest,
    params(
        ("id" = String, Path, description = "Context ID")
    ),
    responses(
        (status = 200, description = "Context updated", body = ContextResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Contexts"
)]
#[patch("/contexts/{id}")]
pub(crate) async fn update_context(
    db_pool: web::Data<sqlx::PgPool>,
    ctx_id: web::Path<String>,
    payload: web::Json<UpdateContextRequest>,
) -> Result<impl Responder, RestError> {
    let ctx_uuid = parse_uuid(&ctx_id, "context id")?;

    if let Some(key) = payload.key.as_deref() {
        validate_context_key(key)?;
    }

    if let Some(entries) = &payload.entries {
        validate_context_entries(entries)?;
    }

    let input = UpdateContextInput {
        key: payload.key.clone(),
        entries: payload.entries.clone(),
    };

    let repo = context_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result =
        crate::logic::context_tx::update_context_in_tx(&mut tx, &repo, ID::from(ctx_uuid), input)
            .await;

    match result {
        Ok(context) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::Ok().json(ContextResponse::from(context)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/contexts/{id}",
    params(
        ("id" = String, Path, description = "Context ID")
    ),
    responses(
        (status = 204, description = "Context deleted"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Contexts"
)]
#[delete("/contexts/{id}")]
pub(crate) async fn delete_context(
    db_pool: web::Data<sqlx::PgPool>,
    ctx_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let ctx_uuid = parse_uuid(&ctx_id, "context id")?;
    let repo = context_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result =
        crate::logic::context_tx::delete_context_in_tx(&mut tx, &repo, ID::from(ctx_uuid)).await;

    match result {
        Ok(_) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_contexts)
        .service(get_context)
        .service(create_context)
        .service(update_context)
        .service(delete_context);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, http::StatusCode, test};

    use crate::logic::context::MockContextLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_context(ctx_id: Uuid, team_id: Uuid) -> ModelContext {
        ModelContext {
            id: ID::from(ctx_id),
            team_id: ID::from(team_id),
            key: "country".to_string(),
            entries: vec![ModelContextEntry {
                id: ID::from(Uuid::new_v4()),
                value: "US".to_string(),
            }],
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
        let name = format!("context-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "context test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_context(pool: &sqlx::PgPool, team_id: Uuid, key: &str) -> Uuid {
        let ctx_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO contexts (id, team_id, key) VALUES ($1, $2, $3)"#,
            ctx_id,
            team_id,
            key
        )
        .execute(pool)
        .await
        .expect("Failed to insert context");

        let entry_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO context_entries (id, context_id, value) VALUES ($1, $2, $3)"#,
            entry_id,
            ctx_id,
            "US"
        )
        .execute(pool)
        .await
        .expect("Failed to insert context entry");
        ctx_id
    }

    #[actix_web::test]
    async fn list_contexts_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let ctx_id = Uuid::new_v4();
        let context = sample_context(ctx_id, team_id);
        let context_clone = context.clone();

        let mut mock_logic = MockContextLogic::new();
        mock_logic
            .expect_get_contexts_with_offset()
            .withf(move |id, key, offset, limit| {
                id.to_string() == team_id.to_string()
                    && key.as_deref() == Some("country")
                    && *offset == 5
                    && *limit == 10
            })
            .times(1)
            .returning(move |_, _, _, _| Ok((vec![context_clone.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Box::new(mock_logic) as Box<dyn ContextLogic>))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/contexts?offset=5&limit=10&key=country");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], ctx_id.to_string());
        assert_eq!(json["meta"]["offset"], 5);
        assert_eq!(json["meta"]["limit"], 10);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn get_context_returns_context() {
        let team_id = Uuid::new_v4();
        let ctx_id = Uuid::new_v4();
        let context = sample_context(ctx_id, team_id);
        let context_clone = context.clone();

        let mut mock_logic = MockContextLogic::new();
        mock_logic
            .expect_get_context_by_id()
            .withf(move |id| id.to_string() == ctx_id.to_string())
            .times(1)
            .returning(move |_| Ok(context_clone.clone()));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Box::new(mock_logic) as Box<dyn ContextLogic>))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/contexts/{ctx_id}");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], ctx_id.to_string());
        assert_eq!(json["key"], "country");
    }

    #[actix_web::test]
    async fn create_context_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/contexts");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateContextRequest {
                key: "country".to_string(),
                entries: vec!["US".to_string()],
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["teamId"], team_id.to_string());
        assert_eq!(json["key"], "country");
    }

    #[actix_web::test]
    async fn create_context_duplicate_entries_returns_bad_request() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/contexts");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateContextRequest {
                key: "country".to_string(),
                entries: vec!["US".to_string(), "US".to_string()],
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "invalid_input");
        assert_eq!(json["message"], "Duplicate context entry");
    }

    #[actix_web::test]
    async fn update_context_returns_updated() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let ctx_id = insert_context(&pool, team_id, "country").await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/contexts/{ctx_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateContextRequest {
                key: Some("user.tier".to_string()),
                entries: Some(vec!["pro".to_string()]),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], ctx_id.to_string());
        assert_eq!(json["key"], "user.tier");
    }

    #[actix_web::test]
    async fn delete_context_returns_no_content() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let ctx_id = insert_context(&pool, team_id, "country").await;
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/contexts/{ctx_id}");
        let req = test::TestRequest::delete().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
}
