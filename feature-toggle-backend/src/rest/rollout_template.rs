use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, post, web};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::rollout_template::{
    CreateRolloutTemplate, RolloutTemplateRow, rollout_template_repository,
};
use crate::model::{CreateFeatureStageInput, CreateRelationshipInput, ID};
use crate::rest::error::RestError;
use crate::rest::feature::CreateFeatureStageRequest;
use crate::rest::pagination::PageMeta;
use crate::rest::pipeline::CreateRelationshipRequest;
use crate::validation::{
    validate_duplicate_environment_and_index, validate_relationships_and_stages,
};

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RolloutTemplateVariables {
    pub environment_ids: Vec<String>,
    pub percentages: Option<Vec<i32>>,
    pub approval_required: Option<bool>,
    pub metric_gate: Option<String>,
    pub schedule: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RolloutTemplateConfig {
    pub stages: Vec<CreateFeatureStageRequest>,
    pub relationships: Vec<CreateRelationshipRequest>,
    pub variables: Option<RolloutTemplateVariables>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutTemplateResponse {
    pub id: String,
    pub team_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub template_type: String,
    pub is_system: bool,
    pub variables: RolloutTemplateVariables,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RolloutTemplatesResponse {
    pub items: Vec<RolloutTemplateResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRolloutTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub template_type: Option<String>,
    pub config: RolloutTemplateConfig,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutTemplatePreviewRequest {
    pub template_id: Option<String>,
    pub template_type: Option<String>,
    pub variables: RolloutTemplateVariables,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutTemplatePreviewResponse {
    pub template_id: String,
    pub template_name: String,
    pub template_type: String,
    pub stages: Vec<CreateFeatureStageRequest>,
    pub relationships: Vec<CreateRelationshipRequest>,
    pub variables: RolloutTemplateVariables,
    pub validation_errors: Vec<String>,
}

#[derive(Debug, Clone)]
struct SystemRolloutTemplate {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    template_type: &'static str,
    percentages: Option<&'static [i32]>,
    approval_required: Option<bool>,
    metric_gate: Option<&'static str>,
    schedule: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct ResolvedRolloutTemplate {
    id: String,
    name: String,
    template_type: String,
    config: Option<RolloutTemplateConfig>,
}

const SYSTEM_TEMPLATES: &[SystemRolloutTemplate] = &[
    SystemRolloutTemplate {
        id: "simple_on_off",
        name: "Simple on/off",
        description: "Single-stage rollout for basic flag activation.",
        template_type: "simple_on_off",
        percentages: None,
        approval_required: Some(false),
        metric_gate: None,
        schedule: None,
    },
    SystemRolloutTemplate {
        id: "canary_10_50_100",
        name: "Canary 10-50-100",
        description: "Three-step progressive rollout with 10%, 50%, and 100% checkpoints.",
        template_type: "canary_10_50_100",
        percentages: Some(&[10, 50, 100]),
        approval_required: Some(false),
        metric_gate: Some("error_rate"),
        schedule: Some("manual"),
    },
    SystemRolloutTemplate {
        id: "approval_gated_production",
        name: "Approval-gated production",
        description: "Linear rollout intended for promotion into production with approval.",
        template_type: "approval_gated_production",
        percentages: Some(&[100]),
        approval_required: Some(true),
        metric_gate: None,
        schedule: Some("manual"),
    },
    SystemRolloutTemplate {
        id: "experiment_rollout",
        name: "Experiment rollout",
        description: "Two-stage experiment path for validating a contextual flag.",
        template_type: "experiment_rollout",
        percentages: Some(&[50, 50]),
        approval_required: Some(false),
        metric_gate: Some("conversion_rate"),
        schedule: Some("manual"),
    },
    SystemRolloutTemplate {
        id: "kill_switch_guarded",
        name: "Kill-switch guarded rollout",
        description: "Rollout path with approval and metric-gate metadata for safer launches.",
        template_type: "kill_switch_guarded",
        percentages: Some(&[10, 50, 100]),
        approval_required: Some(true),
        metric_gate: Some("error_rate"),
        schedule: Some("manual"),
    },
];

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<(Uuid, String)> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| (jwt.id, jwt.username.clone()))
}

fn require_template_admin(req: &HttpRequest) -> Result<JwtUser, RestError> {
    let jwt = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;

    if jwt.is_admin || jwt.roles.iter().any(|role| role == "Team Admin") {
        return Ok(jwt);
    }

    Err(RestError::forbidden(
        "Only system admins or Team Admin users can manage rollout templates",
    ))
}

fn system_template_response(template: &SystemRolloutTemplate) -> RolloutTemplateResponse {
    RolloutTemplateResponse {
        id: template.id.to_string(),
        team_id: None,
        name: template.name.to_string(),
        description: Some(template.description.to_string()),
        template_type: template.template_type.to_string(),
        is_system: true,
        variables: RolloutTemplateVariables {
            environment_ids: vec![],
            percentages: template.percentages.map(|values| values.to_vec()),
            approval_required: template.approval_required,
            metric_gate: template.metric_gate.map(str::to_string),
            schedule: template.schedule.map(str::to_string),
        },
        created_at: None,
        updated_at: None,
    }
}

fn custom_template_response(row: RolloutTemplateRow) -> RolloutTemplateResponse {
    let config = serde_json::from_value::<RolloutTemplateConfig>(row.config.clone()).ok();

    RolloutTemplateResponse {
        id: row.id.to_string(),
        team_id: row.team_id.map(|id| id.to_string()),
        name: row.name,
        description: row.description,
        template_type: row.template_type,
        is_system: row.is_system,
        variables: config
            .and_then(|config| config.variables)
            .unwrap_or_default(),
        created_at: Some(row.created_at),
        updated_at: Some(row.updated_at),
    }
}

fn validate_template_name(name: &str) -> Result<(), RestError> {
    let len = name.trim().len();
    if !(3..=100).contains(&len) {
        return Err(RestError::invalid_input(
            "Template name must be between 3 and 100 characters",
        ));
    }
    Ok(())
}

fn map_stage_requests(
    stages: &[CreateFeatureStageRequest],
) -> Result<Vec<CreateFeatureStageInput>, RestError> {
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
        let stage_id = match stage.id.as_deref() {
            Some(value) => Some(ID::from(parse_uuid(value, "stage id")?)),
            None => None,
        };
        mapped.push(CreateFeatureStageInput {
            id: stage_id,
            environment_id: ID::from(env_uuid),
            order_index: stage.order_index,
            position: stage.position.clone(),
            bucketing_key: stage.bucketing_key.clone(),
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

fn validate_generated_config(config: &RolloutTemplateConfig) -> Vec<String> {
    let stages = match map_stage_requests(&config.stages) {
        Ok(stages) => stages,
        Err(err) => return vec![rest_error_message(&err)],
    };
    let relationships = match map_relationship_requests(&config.relationships) {
        Ok(relationships) => relationships,
        Err(err) => return vec![rest_error_message(&err)],
    };

    let mut errors = Vec::new();
    if let Err(err) = validate_relationships_and_stages(&stages, &relationships) {
        errors.push(err);
    }
    if let Err(err) = validate_duplicate_environment_and_index(&stages) {
        errors.push(err);
    }
    errors
}

fn rest_error_message(err: &RestError) -> String {
    match err {
        RestError::NotFound { message, .. }
        | RestError::InvalidInput { message, .. }
        | RestError::Conflict { message, .. }
        | RestError::Unauthorized { message, .. }
        | RestError::Forbidden { message, .. }
        | RestError::Internal { message, .. } => message.clone(),
    }
}

fn relationship_chain(stage_count: usize) -> Vec<CreateRelationshipRequest> {
    (0..stage_count.saturating_sub(1))
        .map(|index| CreateRelationshipRequest {
            source_id: index as i32,
            target_id: index as i32 + 1,
        })
        .collect()
}

fn build_stages(environment_ids: Vec<String>) -> Vec<CreateFeatureStageRequest> {
    environment_ids
        .into_iter()
        .enumerate()
        .map(|(index, environment_id)| CreateFeatureStageRequest {
            id: None,
            environment_id,
            order_index: index as i32,
            position: json!({ "x": (index as i32) * 260, "y": 80 }).to_string(),
            bucketing_key: None,
        })
        .collect()
}

fn merge_variables(
    defaults: &SystemRolloutTemplate,
    mut requested: RolloutTemplateVariables,
) -> RolloutTemplateVariables {
    if requested.percentages.is_none() {
        requested.percentages = defaults.percentages.map(|values| values.to_vec());
    }
    if requested.approval_required.is_none() {
        requested.approval_required = defaults.approval_required;
    }
    if requested.metric_gate.is_none() {
        requested.metric_gate = defaults.metric_gate.map(str::to_string);
    }
    if requested.schedule.is_none() {
        requested.schedule = defaults.schedule.map(str::to_string);
    }
    requested
}

fn expand_system_template(
    template: &SystemRolloutTemplate,
    variables: RolloutTemplateVariables,
) -> RolloutTemplateConfig {
    let variables = merge_variables(template, variables);
    let mut environment_ids = variables.environment_ids.clone();

    match template.template_type {
        "simple_on_off" if environment_ids.len() > 1 => environment_ids.truncate(1),
        "experiment_rollout" if environment_ids.len() > 2 => environment_ids.truncate(2),
        _ => {}
    }

    let stages = build_stages(environment_ids);
    let relationships = relationship_chain(stages.len());

    RolloutTemplateConfig {
        stages,
        relationships,
        variables: Some(variables),
    }
}

fn system_template_by_id_or_type(value: &str) -> Option<&'static SystemRolloutTemplate> {
    SYSTEM_TEMPLATES
        .iter()
        .find(|template| template.id == value || template.template_type == value)
}

async fn resolve_template(
    repo: &crate::database::rollout_template::RolloutTemplateRepository,
    team_id: Uuid,
    template_id: Option<&str>,
    template_type: Option<&str>,
) -> Result<ResolvedRolloutTemplate, RestError> {
    if let Some(value) = template_id
        && let Some(system) = system_template_by_id_or_type(value)
    {
        return Ok(ResolvedRolloutTemplate {
            id: system.id.to_string(),
            name: system.name.to_string(),
            template_type: system.template_type.to_string(),
            config: None,
        });
    }

    if let Some(value) = template_type
        && let Some(system) = system_template_by_id_or_type(value)
    {
        return Ok(ResolvedRolloutTemplate {
            id: system.id.to_string(),
            name: system.name.to_string(),
            template_type: system.template_type.to_string(),
            config: None,
        });
    }

    if let Some(value) = template_id {
        let id = parse_uuid(value, "template_id")?;
        let row = repo
            .get_custom_for_team(id, team_id)
            .await
            .map_err(RestError::from)?;
        let config = serde_json::from_value::<RolloutTemplateConfig>(row.config.clone())
            .map_err(|_| RestError::internal("Stored rollout template config is invalid"))?;
        return Ok(ResolvedRolloutTemplate {
            id: row.id.to_string(),
            name: row.name,
            template_type: row.template_type,
            config: Some(config),
        });
    }

    Err(RestError::invalid_input(
        "templateId or templateType is required",
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/rollout-templates",
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Rollout templates", body = RolloutTemplatesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Rollout Templates"
)]
#[get("/teams/{team_id}/rollout-templates")]
pub(crate) async fn list_rollout_templates(
    db_pool: web::Data<sqlx::PgPool>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let repo = rollout_template_repository(db_pool.get_ref().clone());
    let custom = repo
        .list_custom_for_team(team_uuid)
        .await
        .map_err(RestError::from)?;

    let mut items: Vec<RolloutTemplateResponse> = SYSTEM_TEMPLATES
        .iter()
        .map(system_template_response)
        .collect();
    items.extend(custom.into_iter().map(custom_template_response));

    let total = items.len() as i64;
    Ok(HttpResponse::Ok().json(RolloutTemplatesResponse {
        items,
        meta: PageMeta {
            offset: 0,
            limit: total,
            total,
        },
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/rollout-templates/preview",
    request_body = RolloutTemplatePreviewRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Rollout template preview", body = RolloutTemplatePreviewResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Rollout Templates"
)]
#[post("/teams/{team_id}/rollout-templates/preview")]
pub(crate) async fn preview_rollout_template(
    db_pool: web::Data<sqlx::PgPool>,
    team_id: web::Path<String>,
    payload: web::Json<RolloutTemplatePreviewRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let repo = rollout_template_repository(db_pool.get_ref().clone());
    let resolved = resolve_template(
        &repo,
        team_uuid,
        payload.template_id.as_deref(),
        payload.template_type.as_deref(),
    )
    .await?;

    let config = if let Some(config) = resolved.config {
        config
    } else {
        let system = system_template_by_id_or_type(&resolved.template_type)
            .ok_or_else(|| RestError::not_found("System rollout template not found"))?;
        expand_system_template(system, payload.variables.clone())
    };

    let validation_errors = validate_generated_config(&config);
    let variables = config
        .variables
        .clone()
        .unwrap_or_else(|| payload.variables.clone());

    Ok(HttpResponse::Ok().json(RolloutTemplatePreviewResponse {
        template_id: resolved.id,
        template_name: resolved.name,
        template_type: resolved.template_type,
        stages: config.stages,
        relationships: config.relationships,
        variables,
        validation_errors,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/rollout-templates",
    request_body = CreateRolloutTemplateRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Rollout template created", body = RolloutTemplateResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Rollout Templates"
)]
#[post("/teams/{team_id}/rollout-templates")]
pub(crate) async fn create_rollout_template(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<CreateRolloutTemplateRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = require_template_admin(&req)?;
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    validate_template_name(&payload.name)?;

    let validation_errors = validate_generated_config(&payload.config);
    if !validation_errors.is_empty() {
        return Err(RestError::invalid_input(validation_errors.join("; ")));
    }

    let repo = rollout_template_repository(db_pool.get_ref().clone());
    let config = serde_json::to_value(&payload.config)
        .map_err(|_| RestError::invalid_input("Rollout template config is invalid"))?;

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = repo
        .create_custom_tx(
            &mut tx,
            CreateRolloutTemplate {
                team_id: team_uuid,
                name: payload.name.trim().to_string(),
                description: payload.description.clone(),
                template_type: payload
                    .template_type
                    .clone()
                    .unwrap_or_else(|| "custom".to_string()),
                config,
                created_by: Some(jwt.id),
            },
        )
        .await;

    let row = match result {
        Ok(row) => row,
        Err(crate::Error::RecordAlreadyExists(_)) => {
            let _ = tx.rollback().await;
            return Err(RestError::conflict("Rollout template name already exists"));
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    };

    let actor = actor_from_request(&req);
    activity_repo
        .create_activity_tx(
            &mut tx,
            CreateActivityLog {
                activity_type: "rollout_template.created".to_string(),
                entity_type: "rollout_template".to_string(),
                entity_id: row.id.to_string(),
                actor_id: actor.as_ref().map(|(id, _)| *id),
                actor_name: actor.map(|(_, name)| name),
                description: format!("Created rollout template '{}'", row.name),
                metadata: Some(json!({
                    "team_id": team_uuid,
                    "template_type": row.template_type,
                    "is_system": false
                })),
            },
        )
        .await
        .map_err(|e| RestError::internal(format!("Failed to audit rollout template: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;

    Ok(HttpResponse::Created().json(custom_template_response(row)))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_rollout_templates)
        .service(preview_rollout_template)
        .service(create_rollout_template);
}
