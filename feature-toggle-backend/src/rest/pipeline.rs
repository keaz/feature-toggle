use actix_web::{get, patch, post, web, HttpMessage, HttpRequest, HttpResponse, Responder};
use crate::model::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::model::{
    CreatePipelineInput, CreateRelationshipInput, CreateStageInput, Pipeline, PipelineRelationship,
    PipelineStage, UpdatePipelineInput,
};
use crate::validation::{
    validate_duplicate_environment_and_index, validate_relationships_and_stages,
};
use crate::database::activity_log::ActivityLogRepository;
use crate::database::pipeline::{pipeline_repository_tx, PipelineRepository};
use crate::logic::pipeline::PipelineLogic;
use crate::logic::ActorContext;
use crate::rest::environment::EnvironmentResponse;
use crate::rest::error::RestError;
use crate::rest::pagination::{normalize_pagination, PageMeta, PaginationQuery};
use crate::JwtUser;

#[derive(Debug, Deserialize, ToSchema)]
pub struct PipelineListQuery {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineRelationshipResponse {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStageResponse {
    pub id: String,
    pub environment: EnvironmentResponse,
    pub order_index: i32,
    pub position: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PipelineResponse {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub team_id: String,
    pub stages: Vec<PipelineStageResponse>,
    pub relationships: Vec<PipelineRelationshipResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PipelinesResponse {
    pub items: Vec<PipelineResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateStageRequest {
    pub environment_id: String,
    pub order_index: i32,
    pub position: String,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRelationshipRequest {
    pub source_id: i32,
    pub target_id: i32,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatePipelineRequest {
    pub name: String,
    pub stages: Vec<CreateStageRequest>,
    pub relationships: Vec<CreateRelationshipRequest>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePipelineRequest {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub stages: Vec<CreateStageRequest>,
    pub relationships: Vec<CreateRelationshipRequest>,
}

impl From<PipelineRelationship> for PipelineRelationshipResponse {
    fn from(rel: PipelineRelationship) -> Self {
        Self {
            source_id: rel.source_id,
            target_id: rel.target_id,
        }
    }
}

impl From<PipelineStage> for PipelineStageResponse {
    fn from(stage: PipelineStage) -> Self {
        Self {
            id: stage.id.to_string(),
            environment: EnvironmentResponse::from(stage.environment),
            order_index: stage.order_index,
            position: stage.position,
        }
    }
}

impl From<Pipeline> for PipelineResponse {
    fn from(pipeline: Pipeline) -> Self {
        Self {
            id: pipeline.id.to_string(),
            name: pipeline.name,
            active: pipeline.active,
            team_id: pipeline.team_id.to_string(),
            stages: pipeline
                .stages
                .into_iter()
                .map(PipelineStageResponse::from)
                .collect(),
            relationships: pipeline
                .relationships
                .into_iter()
                .map(PipelineRelationshipResponse::from)
                .collect(),
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

fn validate_pipeline_name(name: &str) -> Result<(), RestError> {
    let trimmed = name.trim();
    if trimmed.len() < 5 || trimmed.len() > 100 {
        return Err(RestError::invalid_input(
            "Pipeline name must be between 5 and 100 characters",
        ));
    }
    Ok(())
}

fn map_stage_requests(stages: &[CreateStageRequest]) -> Result<Vec<CreateStageInput>, RestError> {
    if stages.is_empty() {
        return Err(RestError::invalid_input(
            "Pipeline must have at least one stage",
        ));
    }

    let mut mapped = Vec::with_capacity(stages.len());
    for stage in stages {
        if stage.order_index < 0 {
            return Err(RestError::invalid_input(
                "Stage order_index must be greater than or equal to 0",
            ));
        }
        let position_len = stage.position.trim().len();
        if !(1..=50).contains(&position_len) {
            return Err(RestError::invalid_input(
                "Stage position must be between 1 and 50 characters",
            ));
        }
        let env_uuid = parse_uuid(&stage.environment_id, "environment_id")?;
        mapped.push(CreateStageInput {
            environment_id: ID::from(env_uuid),
            order_index: stage.order_index,
            position: stage.position.clone(),
        });
    }

    Ok(mapped)
}

fn map_relationship_requests(
    relationships: &[CreateRelationshipRequest],
) -> Result<Vec<CreateRelationshipInput>, RestError> {
    let mut mapped = Vec::with_capacity(relationships.len());
    for rel in relationships {
        if rel.source_id < 0 {
            return Err(RestError::invalid_input(
                "Relationship source_id must be greater than or equal to 0",
            ));
        }
        if rel.target_id < 1 {
            return Err(RestError::invalid_input(
                "Relationship target_id must be greater than or equal to 1",
            ));
        }
        mapped.push(CreateRelationshipInput {
            source_id: rel.source_id,
            target_id: rel.target_id,
        });
    }
    Ok(mapped)
}

fn validate_pipeline_structure(
    stages: &[CreateStageInput],
    relationships: &[CreateRelationshipInput],
) -> Result<(), RestError> {
    validate_relationships_and_stages(stages, relationships)
        .map_err(RestError::invalid_input)?;
    validate_duplicate_environment_and_index(stages)
        .map_err(RestError::invalid_input)?;
    Ok(())
}

async fn ensure_pipeline_name_unique(
    repo: &dyn PipelineRepository,
    team_id: ID,
    name: &str,
    active: Option<bool>,
    exclude_id: Option<ID>,
) -> Result<(), RestError> {
    let team_uuid =
        Uuid::try_from(team_id).map_err(|e| RestError::invalid_input(e.to_string()))?;
    let pipelines = repo
        .get_pipelines(team_uuid, Some(name.to_string()), active)
        .await
        .map_err(RestError::from)?;

    let exclude_uuid = exclude_id
        .map(Uuid::try_from)
        .transpose()
        .map_err(|e| RestError::invalid_input(format!("invalid pipeline_id: {e}")))?;

    let has_conflict = if let Some(exclude) = exclude_uuid {
        pipelines.iter().any(|pipeline| pipeline.id != exclude)
    } else {
        !pipelines.is_empty()
    };

    if has_conflict {
        return Err(RestError::conflict(format!(
            "Pipeline with name '{}' already exists",
            name
        )));
    }

    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/pipelines",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("name" = Option<String>, Query, description = "Filter by pipeline name"),
        ("active" = Option<bool>, Query, description = "Filter by active status"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Pipelines list", body = PipelinesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Pipelines"
)]
#[get("/teams/{team_id}/pipelines")]
pub(crate) async fn list_pipelines(
    logic: web::Data<Box<dyn PipelineLogic>>,
    team_id: web::Path<String>,
    query: web::Query<PipelineListQuery>,
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

    let (items, total) = logic
        .get_pipelines_with_offset(
            ID::from(team_uuid),
            name,
            query.active,
            vec![],
            offset,
            limit,
        )
        .await
        .map_err(RestError::from)?;

    let response = PipelinesResponse {
        items: items.into_iter().map(PipelineResponse::from).collect(),
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
    path = "/api/v1/pipelines/{id}",
    params(
        ("id" = String, Path, description = "Pipeline ID")
    ),
    responses(
        (status = 200, description = "Pipeline", body = PipelineResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Pipelines"
)]
#[get("/pipelines/{id}")]
pub(crate) async fn get_pipeline(
    logic: web::Data<Box<dyn PipelineLogic>>,
    pipeline_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let pipeline_uuid = parse_uuid(&pipeline_id, "pipeline id")?;
    let pipeline = logic
        .get_pipeline_by_id(ID::from(pipeline_uuid))
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(PipelineResponse::from(pipeline)))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/pipelines",
    request_body = CreatePipelineRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Pipeline created", body = PipelineResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Pipelines"
)]
#[post("/teams/{team_id}/pipelines")]
pub(crate) async fn create_pipeline(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    logic: web::Data<Box<dyn PipelineLogic>>,
    team_id: web::Path<String>,
    payload: web::Json<CreatePipelineRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let actor = actor_from_request(&req);
    validate_pipeline_name(&payload.name)?;

    let repo = pipeline_repository_tx(db_pool.get_ref().clone());
    ensure_pipeline_name_unique(
        &repo,
        ID::from(team_uuid),
        payload.name.as_str(),
        Some(true),
        None,
    )
    .await?;

    let stages = map_stage_requests(&payload.stages)?;
    let relationships = map_relationship_requests(&payload.relationships)?;
    validate_pipeline_structure(&stages, &relationships)?;

    let input = CreatePipelineInput {
        name: payload.name.clone(),
        stages,
        relationships,
    };

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::pipeline_tx::create_pipeline_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(team_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(pipeline_id) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            let pipeline = logic
                .get_pipeline_by_id(pipeline_id)
                .await
                .map_err(RestError::from)?;
            Ok(HttpResponse::Created().json(PipelineResponse::from(pipeline)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/pipelines/{id}",
    request_body = UpdatePipelineRequest,
    params(
        ("id" = String, Path, description = "Pipeline ID")
    ),
    responses(
        (status = 200, description = "Pipeline updated", body = PipelineResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Pipelines"
)]
#[patch("/pipelines/{id}")]
pub(crate) async fn update_pipeline(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    logic: web::Data<Box<dyn PipelineLogic>>,
    pipeline_id: web::Path<String>,
    payload: web::Json<UpdatePipelineRequest>,
) -> Result<impl Responder, RestError> {
    let pipeline_uuid = parse_uuid(&pipeline_id, "pipeline id")?;
    let actor = actor_from_request(&req);
    let repo = pipeline_repository_tx(db_pool.get_ref().clone());

    if let Some(name) = payload.name.as_deref() {
        validate_pipeline_name(name)?;
        let existing = repo
            .get_pipeline_by_id(pipeline_uuid)
            .await
            .map_err(RestError::from)?;
        ensure_pipeline_name_unique(
            &repo,
            ID::from(existing.team_id),
            name,
            payload.active,
            Some(ID::from(existing.id)),
        )
        .await?;
    }

    let stages = map_stage_requests(&payload.stages)?;
    let relationships = map_relationship_requests(&payload.relationships)?;
    validate_pipeline_structure(&stages, &relationships)?;

    let input = UpdatePipelineInput {
        name: payload.name.clone(),
        active: payload.active,
        stages,
        relationships,
    };

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::pipeline_tx::update_pipeline_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        ID::from(pipeline_uuid),
        input,
        actor,
    )
    .await;

    match result {
        Ok(_) => {
            tx.commit().await.map_err(|e| {
                RestError::internal(format!("Failed to commit transaction: {e}"))
            })?;
            let pipeline = logic
                .get_pipeline_by_id(ID::from(pipeline_uuid))
                .await
                .map_err(RestError::from)?;
            Ok(HttpResponse::Ok().json(PipelineResponse::from(pipeline)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_pipelines)
        .service(get_pipeline)
        .service(create_pipeline)
        .service(update_pipeline);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};
    use crate::database::activity_log::PgActivityLogRepository;
    use crate::logic::pipeline::MockPipelineLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_env(env_id: Uuid, team_id: Uuid) -> crate::model::Environment {
        crate::model::Environment {
            id: ID::from(env_id),
            name: "Production".to_string(),
            team_id: ID::from(team_id),
            active: true,
            environment_type: "Production".to_string(),
        }
    }

    fn sample_pipeline(id: ID, team_id: Uuid, env_id: Uuid) -> Pipeline {
        Pipeline {
            id,
            name: "Release Pipeline".to_string(),
            active: true,
            team_id: ID::from(team_id),
            stages: vec![PipelineStage {
                id: ID::from(Uuid::new_v4()),
                environment: sample_env(env_id, team_id),
                order_index: 0,
                position: "{\"x\":0,\"y\":0}".to_string(),
            }],
            relationships: vec![],
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
        let name = format!("pipeline-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "pipeline test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_environment(pool: &sqlx::PgPool, team_id: Uuid) -> Uuid {
        let env_id = Uuid::new_v4();
        let name = format!("pipeline-env-{}", env_id);
        sqlx::query!(
            r#"INSERT INTO environments (id, name, active, team_id, environment_type)
               VALUES ($1, $2, $3, $4, $5)"#,
            env_id,
            name,
            true,
            team_id,
            "Development"
        )
        .execute(pool)
        .await
        .expect("Failed to insert environment");
        env_id
    }

    async fn insert_pipeline(
        pool: &sqlx::PgPool,
        team_id: Uuid,
        env_id: Uuid,
        name: &str,
    ) -> Uuid {
        let pipeline_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO pipelines (id, name, active, team_id) VALUES ($1, $2, $3, $4)"#,
            pipeline_id,
            name,
            true,
            team_id
        )
        .execute(pool)
        .await
        .expect("Failed to insert pipeline");

        let stage_id = Uuid::new_v4();
        sqlx::query!(
            r#"INSERT INTO pipeline_stages (id, pipeline_id, environment_id, order_index, parent_stage_id, position)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
            stage_id,
            pipeline_id,
            env_id,
            0,
            Option::<Uuid>::None,
            "{\"x\":0,\"y\":0}"
        )
        .execute(pool)
        .await
        .expect("Failed to insert pipeline stage");
        pipeline_id
    }

    #[actix_web::test]
    async fn list_pipelines_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let pipeline_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let pipeline = sample_pipeline(ID::from(pipeline_id), team_id, env_id);
        let pipeline_clone = pipeline.clone();

        let mut mock_logic = MockPipelineLogic::new();
        mock_logic
            .expect_get_pipelines_with_offset()
            .withf(move |id, name, active, _fields, offset, limit| {
                id.to_string() == team_id.to_string()
                    && name.as_deref() == Some("release")
                    && *active == Some(true)
                    && *offset == 5
                    && *limit == 10
            })
            .times(1)
            .returning(move |_, _, _, _, _, _| Ok((vec![pipeline_clone.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!(
            "/api/v1/teams/{team_id}/pipelines?offset=5&limit=10&name=release&active=true"
        );
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], pipeline_id.to_string());
        assert_eq!(json["meta"]["offset"], 5);
        assert_eq!(json["meta"]["limit"], 10);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn get_pipeline_returns_pipeline() {
        let team_id = Uuid::new_v4();
        let pipeline_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let pipeline = sample_pipeline(ID::from(pipeline_id), team_id, env_id);
        let pipeline_clone = pipeline.clone();

        let mut mock_logic = MockPipelineLogic::new();
        mock_logic
            .expect_get_pipeline_by_id()
            .withf(move |id| id.to_string() == pipeline_id.to_string())
            .times(1)
            .returning(move |_| Ok(pipeline_clone.clone()));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/pipelines/{pipeline_id}");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], pipeline_id.to_string());
        assert_eq!(json["name"], "Release Pipeline");
    }

    #[actix_web::test]
    async fn create_pipeline_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id).await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let mut mock_logic = MockPipelineLogic::new();
        mock_logic
            .expect_get_pipeline_by_id()
            .returning(move |id| Ok(sample_pipeline(id, team_id, env_id)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/pipelines");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreatePipelineRequest {
                name: format!("Release Pipeline {}", Uuid::new_v4()),
                stages: vec![CreateStageRequest {
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                }],
                relationships: vec![],
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[actix_web::test]
    async fn create_pipeline_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id).await;
        let existing_name = "Release Pipeline";
        let _pipeline_id = insert_pipeline(&pool, team_id, env_id, existing_name).await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(MockPipelineLogic::new()) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/pipelines");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreatePipelineRequest {
                name: existing_name.to_string(),
                stages: vec![CreateStageRequest {
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                }],
                relationships: vec![],
            })
            .to_request();
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body = test::read_body(resp).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "response body: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[actix_web::test]
    async fn update_pipeline_returns_updated() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id).await;
        let pipeline_id = insert_pipeline(&pool, team_id, env_id, "Original Pipeline").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let mut mock_logic = MockPipelineLogic::new();
        mock_logic
            .expect_get_pipeline_by_id()
            .returning(move |id| Ok(sample_pipeline(id, team_id, env_id)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/pipelines/{pipeline_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdatePipelineRequest {
                name: Some("Updated Pipeline".to_string()),
                active: Some(true),
                stages: vec![CreateStageRequest {
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                }],
                relationships: vec![],
            })
            .to_request();
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body = test::read_body(resp).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "response body: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[actix_web::test]
    async fn update_pipeline_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id).await;
        let pipeline_id = insert_pipeline(&pool, team_id, env_id, "Primary Pipeline").await;
        let _other_pipeline_id =
            insert_pipeline(&pool, team_id, env_id, "Updated Pipeline").await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool.clone()))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(MockPipelineLogic::new()) as Box<dyn PipelineLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/pipelines/{pipeline_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdatePipelineRequest {
                name: Some("Updated Pipeline".to_string()),
                active: Some(true),
                stages: vec![CreateStageRequest {
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                }],
                relationships: vec![],
            })
            .to_request();
        let resp = test::call_service(&app, req).await;
        let status = resp.status();
        let body = test::read_body(resp).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "response body: {}",
            String::from_utf8_lossy(&body)
        );
    }
}
