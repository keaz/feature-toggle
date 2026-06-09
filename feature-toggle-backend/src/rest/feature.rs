mod types;
pub use types::*;

use crate::model::ID;
use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, patch, post, web};
use sqlx::Row;
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

use crate::JwtUser;
use crate::broadcast::map_db_feature_to_full_for_broadcast;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::{DBStage, FeaturePipelineStage};
use crate::database::feature::{
    FeatureRepository, FeatureVersion, FeatureVersionDiffEntry, feature_repository_tx,
};
use crate::logic::authorization::RoleAuthorizer;
use crate::logic::environment::EnvironmentLogic;
use crate::logic::feature::{FeatureLogic, StageChangeRequestType};
use crate::logic::feature_tx;
use crate::logic::pipeline::PipelineLogic;
use crate::logic::{ActorContext, create_relationships, get_environment_map};
use crate::model::{
    CreateFeatureInput, CreateFeatureStageInput, CreateFeatureVariantInput,
    CreateRelationshipInput, Feature as ModelFeature, FeatureType as ModelFeatureType,
    LifecycleStage as ModelLifecycleStage, UpdateFeatureInput,
    VariantValueType as ModelVariantValueType,
};
use crate::rest::environment::EnvironmentResponse;
use crate::rest::error::RestError;
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};
use crate::rest::pipeline::CreateRelationshipRequest;
use crate::validation::{
    validate_duplicate_environment_and_index, validate_relationships_and_stages,
};

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

fn ensure_emergency_override_allowed(jwt: &JwtUser) -> Result<(), RestError> {
    let is_team_admin = jwt.roles.iter().any(|role| role == "Team Admin");
    if jwt.is_admin || is_team_admin {
        Ok(())
    } else {
        Err(RestError::forbidden(
            "Only system admins or Team Admins can apply emergency overrides",
        ))
    }
}

fn validate_emergency_reason(reason: &str) -> Result<String, RestError> {
    let trimmed = reason.trim();
    if trimmed.len() < 5 {
        return Err(RestError::invalid_input(
            "Emergency reason must be at least 5 characters",
        ));
    }
    if trimmed.len() > 500 {
        return Err(RestError::invalid_input(
            "Emergency reason must be at most 500 characters",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_emergency_expiry(
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(), RestError> {
    if let Some(expires_at) = expires_at
        && expires_at <= chrono::Utc::now()
    {
        return Err(RestError::invalid_input(
            "Emergency override expiry must be in the future",
        ));
    }
    Ok(())
}

fn validate_feature_key_create(key: &str) -> Result<(), RestError> {
    let trimmed = key.trim();
    if trimmed.len() < 3 || trimmed.len() > 40 {
        return Err(RestError::invalid_input(
            "Feature key must be between 3 and 40 characters",
        ));
    }
    Ok(())
}

fn validate_feature_key_update(key: &str) -> Result<(), RestError> {
    let trimmed = key.trim();
    if trimmed.len() < 3 || trimmed.len() > 100 {
        return Err(RestError::invalid_input(
            "Feature key must be between 3 and 100 characters",
        ));
    }
    Ok(())
}

fn validate_description_create(description: &Option<String>) -> Result<(), RestError> {
    if let Some(desc) = description.as_deref() {
        let trimmed = desc.trim();
        if trimmed.len() < 3 || trimmed.len() > 255 {
            return Err(RestError::invalid_input(
                "Feature description must be between 3 and 255 characters",
            ));
        }
    }
    Ok(())
}

fn validate_variant_requests(
    variants: &Option<Vec<CreateFeatureVariantRequest>>,
) -> Result<(), RestError> {
    if let Some(list) = variants {
        for variant in list {
            let control_len = variant.control.trim().len();
            if !(1..=100).contains(&control_len) {
                return Err(RestError::invalid_input(
                    "Variant control must be between 1 and 100 characters",
                ));
            }
            if let Some(desc) = variant.description.as_deref()
                && desc.trim().len() > 500
            {
                return Err(RestError::invalid_input(
                    "Variant description must be at most 500 characters",
                ));
            }
        }
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

fn validate_feature_structure(
    stages: &[CreateFeatureStageInput],
    relationships: &[CreateRelationshipInput],
) -> Result<(), RestError> {
    validate_relationships_and_stages(stages, relationships).map_err(RestError::invalid_input)?;
    validate_duplicate_environment_and_index(stages).map_err(RestError::invalid_input)?;
    Ok(())
}

fn map_dependencies(ids: &[String]) -> Result<Vec<ID>, RestError> {
    ids.iter()
        .map(|id| Ok(ID::from(parse_uuid(id, "dependency id")?)))
        .collect()
}

async fn ensure_feature_key_unique_for_create(
    logic: &dyn PipelineLogic,
    team_id: ID,
    key: &str,
) -> Result<(), RestError> {
    let pipelines = logic
        .get_pipelines(team_id, Some(key.to_string()), Some(true), vec![])
        .await
        .map_err(RestError::from)?;

    if !pipelines.is_empty() {
        return Err(RestError::conflict(format!(
            "Feature with name '{}' already exists",
            key
        )));
    }

    Ok(())
}

async fn ensure_feature_key_unique_for_update(
    logic: &dyn FeatureLogic,
    feature_id: &ID,
    key: &str,
) -> Result<ModelFeature, RestError> {
    let feature = logic
        .get_feature_by_id(feature_id.clone())
        .await
        .map_err(RestError::from)?;

    let existing = logic
        .get_features(feature.team_id.clone(), Some(key.to_string()), None)
        .await
        .map_err(RestError::from)?;

    let has_conflict = existing.iter().any(|item| item.id != *feature_id);

    if has_conflict {
        return Err(RestError::conflict(format!(
            "Feature with name '{}' already exists",
            key
        )));
    }

    Ok(feature)
}

fn feature_base_response(feature: &ModelFeature) -> FeatureResponse {
    FeatureResponse {
        id: feature.id.to_string(),
        key: feature.key.clone(),
        description: feature.description.clone(),
        feature_type: FeatureType::from(feature.feature_type),
        enabled: feature.enabled,
        created_at: feature.created_at,
        kill_switch_enabled: feature.kill_switch_enabled,
        kill_switch_activated_at: feature.kill_switch_activated_at,
        rollback_scheduled_at: feature.rollback_scheduled_at,
        emergency_override_reason: feature.emergency_override_reason.clone(),
        emergency_override_expires_at: feature.emergency_override_expires_at,
        emergency_override_actor_id: feature
            .emergency_override_actor_id
            .as_ref()
            .map(|id| id.to_string()),
        emergency_override_applied_at: feature.emergency_override_applied_at,
        lifecycle_stage: LifecycleStage::from(feature.lifecycle_stage),
        owner: feature.owner.clone(),
        purpose: feature.purpose.clone(),
        reference_url: feature.reference_url.clone(),
        expires_at: feature.expires_at,
        cleanup_reason: feature.cleanup_reason.clone(),
        tags: feature.tags.clone(),
        archived_at: feature.archived_at,
        deprecated_at: feature.deprecated_at,
        deprecation_notice: feature.deprecation_notice.clone(),
        last_evaluated_at: feature.last_evaluated_at,
        evaluation_count_7d: feature.evaluation_count_7d,
        evaluation_count_30d: feature.evaluation_count_30d,
        evaluation_count_90d: feature.evaluation_count_90d,
        is_stale: feature.is_stale,
        stale_reasons: feature.stale_reasons.clone(),
        dependencies: feature
            .dependencies
            .iter()
            .map(|id| id.to_string())
            .collect(),
        team_id: feature.team_id.to_string(),
        pending_approval_request_id: feature
            .pending_approval_request_id
            .as_ref()
            .map(|id| id.to_string()),
        relationships: None,
        stages: None,
        variants: None,
    }
}

fn stage_boxes(stages: &[FeaturePipelineStage]) -> Vec<Box<dyn DBStage>> {
    stages
        .iter()
        .cloned()
        .map(|stage| Box::new(stage) as Box<dyn DBStage>)
        .collect()
}

async fn load_stage_data(
    feature_repo: &dyn FeatureRepository,
    env_logic: &dyn EnvironmentLogic,
    feature_id: Uuid,
) -> Result<(Vec<FeatureStageResponse>, Vec<FeatureRelationshipResponse>), RestError> {
    let stages = feature_repo
        .get_feature_stages(feature_id)
        .await
        .map_err(RestError::from)?;

    let stages_for_env = stage_boxes(&stages);
    let stages_for_rels = stage_boxes(&stages);

    let environment_map = get_environment_map(env_logic, &stages_for_env, true)
        .await
        .map_err(RestError::from)?;

    let mut mapped_stages = Vec::with_capacity(stages.len());
    for stage in stages.iter() {
        let env = environment_map
            .get(&stage.environment_id)
            .ok_or_else(|| RestError::not_found("Environment not found"))?;
        mapped_stages.push(FeatureStageResponse {
            id: stage.id.to_string(),
            environment: EnvironmentResponse::from(env.clone()),
            order_index: stage.order_index,
            position: stage.position.clone(),
            status: stage.status.clone(),
        });
    }
    mapped_stages.sort_by(|a, b| a.order_index.cmp(&b.order_index));

    let relationships = create_relationships(true, stages_for_rels, |source_id, target_id| {
        FeatureRelationshipResponse {
            source_id,
            target_id,
        }
    });

    Ok((mapped_stages, relationships))
}

async fn load_variants(
    feature_repo: &dyn FeatureRepository,
    feature_id: Uuid,
) -> Result<Vec<FeatureVariantResponse>, RestError> {
    let variants = feature_repo
        .get_feature_variants(feature_id)
        .await
        .map_err(RestError::from)?;

    Ok(variants
        .into_iter()
        .map(|variant| FeatureVariantResponse {
            id: variant.id.to_string(),
            feature_id: variant.feature_id.to_string(),
            control: variant.control,
            value: variant.value,
            value_type: VariantValueType::from(variant.value_type),
            description: variant.description,
            created_at: variant.created_at,
            updated_at: variant.updated_at,
        })
        .collect())
}

async fn build_feature_response(
    feature: &ModelFeature,
    feature_repo: &dyn FeatureRepository,
    env_logic: &dyn EnvironmentLogic,
    include_variants: bool,
    include_relationships: bool,
    include_stages: bool,
) -> Result<FeatureResponse, RestError> {
    let mut response = feature_base_response(feature);
    let feature_id = parse_uuid(&feature.id.to_string(), "feature id")?;

    if include_stages || include_relationships {
        let (stages, relationships) = load_stage_data(feature_repo, env_logic, feature_id).await?;
        if include_stages {
            response.stages = Some(stages);
        }
        if include_relationships {
            response.relationships = Some(relationships);
        }
    }

    if include_variants {
        response.variants = Some(load_variants(feature_repo, feature_id).await?);
    }

    Ok(response)
}

fn map_version_diff_entry(entry: FeatureVersionDiffEntry) -> FeatureVersionDiffEntryResponse {
    FeatureVersionDiffEntryResponse {
        path: entry.path,
        change_type: entry.change_type,
        before: entry.before,
        after: entry.after,
    }
}

fn parse_change_summary(value: serde_json::Value) -> Vec<FeatureVersionDiffEntryResponse> {
    serde_json::from_value::<Vec<FeatureVersionDiffEntry>>(value)
        .unwrap_or_default()
        .into_iter()
        .map(map_version_diff_entry)
        .collect()
}

fn map_feature_version(version: FeatureVersion) -> FeatureVersionResponse {
    FeatureVersionResponse {
        id: version.id.to_string(),
        feature_id: version.feature_id.to_string(),
        version_number: version.version_number,
        snapshot: version.snapshot,
        change_summary: parse_change_summary(version.change_summary),
        actor_id: version.actor_id.map(|id| id.to_string()),
        actor_name: version.actor_name,
        source: version.source,
        created_at: version.created_at,
    }
}

async fn broadcast_feature_update(
    feature_repo: &dyn FeatureRepository,
    updates_tx: &tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    feature_id: Uuid,
) {
    if let Ok(db_feature) = feature_repo.get_feature_by_id(feature_id).await
        && let Ok(full) = map_db_feature_to_full_for_broadcast(feature_repo, db_feature).await
    {
        let _ = updates_tx.send(crate::grpc::pb::FeatureUpdate {
            message_id: uuid::Uuid::new_v4().to_string(),
            action: crate::grpc::pb::feature_update::Action::Upsert as i32,
            feature: Some(full),
            feature_key: String::new(),
            error: String::new(),
        });
    }
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized = tags
        .into_iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

async fn archive_warnings(pool: &sqlx::PgPool, feature_id: Uuid) -> Result<Vec<String>, RestError> {
    let dependent_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM feature_dependencies fd
           JOIN features f ON f.id = fd.feature_id
           WHERE fd.depends_on_id = $1
             AND f.lifecycle_stage <> 'archived'"#,
    )
    .bind(feature_id)
    .fetch_one(pool)
    .await
    .map_err(|e| RestError::internal(format!("Failed to check dependents: {e}")))?;

    let active_stage_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM features_pipeline_stages
           WHERE feature_id = $1
             AND status NOT IN ('NOT_DEPLOYED', 'ROLLBACKED')"#,
    )
    .bind(feature_id)
    .fetch_one(pool)
    .await
    .map_err(|e| RestError::internal(format!("Failed to check rollout stages: {e}")))?;

    let mut warnings = Vec::new();
    if dependent_count > 0 {
        warnings.push(format!("{dependent_count} dependent feature(s)"));
    }
    if active_stage_count > 0 {
        warnings.push(format!("{active_stage_count} active rollout stage(s)"));
    }
    Ok(warnings)
}

fn lifecycle_to_db(stage: LifecycleStage) -> &'static str {
    match stage {
        LifecycleStage::Draft => "draft",
        LifecycleStage::Active => "active",
        LifecycleStage::Deprecated => "deprecated",
        LifecycleStage::Archived => "archived",
    }
}

fn impact_severity(
    action: &str,
    direct_dependents: usize,
    transitive_dependents: usize,
    missing_dependencies: usize,
    cycles: usize,
) -> String {
    let destructive = matches!(action, "archive" | "emergency-disable" | "rollback");
    if cycles > 0 || missing_dependencies > 0 || (destructive && direct_dependents > 0) {
        "high".to_string()
    } else if direct_dependents > 0 || transitive_dependents > 0 {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn detect_cycles_for_impact(adjacency: &HashMap<Uuid, Vec<Uuid>>) -> Vec<Vec<Uuid>> {
    fn visit(
        node: Uuid,
        adjacency: &HashMap<Uuid, Vec<Uuid>>,
        states: &mut HashMap<Uuid, u8>,
        stack: &mut Vec<Uuid>,
        cycles: &mut Vec<Vec<Uuid>>,
    ) {
        states.insert(node, 1);
        stack.push(node);

        if let Some(next_nodes) = adjacency.get(&node) {
            for next in next_nodes {
                match states.get(next).copied().unwrap_or(0) {
                    0 => visit(*next, adjacency, states, stack, cycles),
                    1 => {
                        if let Some(start) = stack.iter().position(|id| id == next) {
                            let mut cycle = stack[start..].to_vec();
                            cycle.push(*next);
                            cycles.push(cycle);
                        }
                    }
                    _ => {}
                }
            }
        }

        stack.pop();
        states.insert(node, 2);
    }

    let mut nodes: HashSet<Uuid> = adjacency.keys().copied().collect();
    for next_nodes in adjacency.values() {
        nodes.extend(next_nodes.iter().copied());
    }

    let mut states = HashMap::new();
    let mut stack = Vec::new();
    let mut cycles = Vec::new();
    for node in nodes {
        if states.get(&node).copied().unwrap_or(0) == 0 {
            visit(node, adjacency, &mut states, &mut stack, &mut cycles);
        }
    }
    cycles
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/features",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("name" = Option<String>, Query, description = "Filter by feature name"),
        ("featureType" = Option<FeatureType>, Query, description = "Filter by feature type"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Feature list", body = FeaturesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/teams/{team_id}/features")]
pub(crate) async fn list_features(
    logic: web::Data<Box<dyn FeatureLogic>>,
    team_id: web::Path<String>,
    query: web::Query<FeatureListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let (features, total) = logic
        .get_features_with_offset_filtered(
            ID::from(team_uuid),
            query.name.clone(),
            query.feature_type.map(ModelFeatureType::from),
            query.lifecycle_stage.map(ModelLifecycleStage::from),
            query.stale,
            query.include_archived.unwrap_or(false),
            query.owner.clone(),
            query.expired,
            query.tag.clone(),
            query.dependency_status.clone(),
            query.approval_status.clone(),
            offset,
            limit,
        )
        .await
        .map_err(RestError::from)?;

    let items = features
        .iter()
        .map(feature_base_response)
        .collect::<Vec<_>>();

    Ok(HttpResponse::Ok().json(FeaturesResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/{id}",
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature detail", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/{id}")]
pub(crate) async fn get_feature(
    logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    feature_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let feature = logic
        .get_feature_by_id(ID::from(feature_uuid))
        .await
        .map_err(RestError::from)?;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[post("/teams/{team_id}/features/bulk-actions")]
pub(crate) async fn bulk_feature_action(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<BulkFeatureActionRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let actor = actor_from_request(&req);
    let (actor_id, actor_name) = actor
        .as_ref()
        .map(|ctx| ctx.as_option())
        .unwrap_or((None, None));

    if payload.feature_ids.is_empty() {
        return Err(RestError::invalid_input("featureIds is required"));
    }

    let requested_ids = payload
        .feature_ids
        .iter()
        .map(|id| parse_uuid(id, "feature id"))
        .collect::<Result<Vec<_>, _>>()?;

    let rows = sqlx::query(
        r#"SELECT id, key, lifecycle_stage, owner, purpose, reference_url, expires_at, tags
           FROM features
           WHERE team_id = $1 AND id = ANY($2)"#,
    )
    .bind(team_uuid)
    .bind(&requested_ids)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load bulk features: {e}")))?;

    let mut feature_rows = HashMap::new();
    for row in rows {
        let id: Uuid = row.get("id");
        feature_rows.insert(id, row);
    }

    let mut results = Vec::with_capacity(requested_ids.len());
    let mut export_rows = Vec::new();

    for feature_id in requested_ids {
        let Some(row) = feature_rows.get(&feature_id) else {
            results.push(BulkFeatureActionResult {
                feature_id: feature_id.to_string(),
                feature_key: None,
                status: "failed".to_string(),
                message: "Feature not found in team".to_string(),
                warnings: vec![],
            });
            continue;
        };

        let feature_key: String = row.get("key");
        let warnings = if matches!(payload.action, BulkFeatureAction::Archive | BulkFeatureAction::UpdateLifecycle)
            && payload.lifecycle_stage == Some(LifecycleStage::Archived)
        {
            archive_warnings(db_pool.get_ref(), feature_id).await?
        } else if matches!(payload.action, BulkFeatureAction::Archive) {
            archive_warnings(db_pool.get_ref(), feature_id).await?
        } else {
            Vec::new()
        };

        if !warnings.is_empty()
            && !payload.archive_confirmation.unwrap_or(false)
            && matches!(payload.action, BulkFeatureAction::Archive | BulkFeatureAction::UpdateLifecycle)
            && (matches!(payload.action, BulkFeatureAction::Archive)
                || payload.lifecycle_stage == Some(LifecycleStage::Archived))
        {
            results.push(BulkFeatureActionResult {
                feature_id: feature_id.to_string(),
                feature_key: Some(feature_key),
                status: "failed".to_string(),
                message: "Confirmation required before archive".to_string(),
                warnings,
            });
            continue;
        }

        let update_result = match payload.action {
            BulkFeatureAction::UpdateOwner => {
                let owner = payload
                    .owner
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                sqlx::query("UPDATE features SET owner = $1 WHERE id = $2 AND team_id = $3")
                    .bind(&owner)
                    .bind(feature_id)
                    .bind(team_uuid)
                    .execute(db_pool.get_ref())
                    .await
            }
            BulkFeatureAction::UpdateTags => {
                let tags = normalize_tags(payload.tags.clone().unwrap_or_default());
                sqlx::query("UPDATE features SET tags = $1 WHERE id = $2 AND team_id = $3")
                    .bind(&tags)
                    .bind(feature_id)
                    .bind(team_uuid)
                    .execute(db_pool.get_ref())
                    .await
            }
            BulkFeatureAction::UpdateLifecycle | BulkFeatureAction::Archive => {
                let lifecycle = if matches!(payload.action, BulkFeatureAction::Archive) {
                    "archived"
                } else {
                    payload
                        .lifecycle_stage
                        .map(lifecycle_to_db)
                        .unwrap_or("active")
                };
                let deprecated_at = if lifecycle == "deprecated" {
                    Some(chrono::Utc::now())
                } else {
                    None
                };
                let archived_at = if lifecycle == "archived" {
                    Some(chrono::Utc::now())
                } else {
                    None
                };
                sqlx::query(
                    r#"UPDATE features
                       SET lifecycle_stage = $1,
                           deprecated_at = COALESCE($2, deprecated_at),
                           archived_at = COALESCE($3, archived_at)
                       WHERE id = $4 AND team_id = $5"#,
                )
                .bind(lifecycle)
                .bind(deprecated_at)
                .bind(archived_at)
                .bind(feature_id)
                .bind(team_uuid)
                .execute(db_pool.get_ref())
                .await
            }
            BulkFeatureAction::Export => {
                let tags: Vec<String> = row.get("tags");
                export_rows.push(BulkFeatureExportRow {
                    id: feature_id.to_string(),
                    key: feature_key.clone(),
                    lifecycle_stage: row.get::<String, _>("lifecycle_stage"),
                    owner: row.get("owner"),
                    purpose: row.get("purpose"),
                    reference_url: row.get("reference_url"),
                    expires_at: row.get("expires_at"),
                    tags,
                });
                Ok(sqlx::postgres::PgQueryResult::default())
            }
        };

        match update_result {
            Ok(_) => {
                if !matches!(payload.action, BulkFeatureAction::Export) {
                    let _ = activity_repo
                        .create_activity(CreateActivityLog {
                            activity_type: "feature_bulk_updated".to_string(),
                            entity_type: "feature".to_string(),
                            entity_id: feature_id.to_string(),
                            actor_id,
                            actor_name: actor_name.clone(),
                            description: format!("Bulk updated feature '{}'", feature_key),
                            metadata: Some(serde_json::json!({
                                "feature_id": feature_id.to_string(),
                                "feature_key": feature_key,
                                "team_id": team_uuid.to_string(),
                                "action": format!("{:?}", payload.action),
                            })),
                        })
                        .await;
                }
                results.push(BulkFeatureActionResult {
                    feature_id: feature_id.to_string(),
                    feature_key: Some(feature_key),
                    status: "success".to_string(),
                    message: "Action applied".to_string(),
                    warnings,
                });
            }
            Err(err) => {
                results.push(BulkFeatureActionResult {
                    feature_id: feature_id.to_string(),
                    feature_key: Some(feature_key),
                    status: "failed".to_string(),
                    message: err.to_string(),
                    warnings,
                });
            }
        }
    }

    Ok(HttpResponse::Ok().json(BulkFeatureActionResponse {
        results,
        export_rows: if export_rows.is_empty() {
            None
        } else {
            Some(export_rows)
        },
    }))
}

#[get("/features/{id}/dependency-impact")]
pub(crate) async fn dependency_impact(
    db_pool: web::Data<sqlx::PgPool>,
    feature_id: web::Path<String>,
    query: web::Query<DependencyImpactQuery>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let action = query.action.clone().unwrap_or_else(|| "update".to_string());

    let root = sqlx::query("SELECT id, key, team_id FROM features WHERE id = $1")
        .bind(feature_uuid)
        .fetch_optional(db_pool.get_ref())
        .await
        .map_err(|e| RestError::internal(format!("Failed to load feature: {e}")))?
        .ok_or_else(|| RestError::not_found("Feature not found"))?;
    let team_id: Uuid = root.get("team_id");

    let feature_rows = sqlx::query(
        r#"SELECT id, key, lifecycle_stage, active
           FROM features
           WHERE team_id = $1"#,
    )
    .bind(team_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load team features: {e}")))?;

    let mut features = HashMap::new();
    for row in feature_rows {
        features.insert(row.get::<Uuid, _>("id"), row);
    }

    let dependency_rows = sqlx::query(
        r#"SELECT fd.feature_id, fd.depends_on_id
           FROM feature_dependencies fd
           JOIN features f ON f.id = fd.feature_id
           WHERE f.team_id = $1"#,
    )
    .bind(team_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load dependencies: {e}")))?;

    let mut adjacency: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut reverse: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for row in dependency_rows {
        let feature_id: Uuid = row.get("feature_id");
        let depends_on_id: Uuid = row.get("depends_on_id");
        adjacency.entry(feature_id).or_default().push(depends_on_id);
        reverse.entry(depends_on_id).or_default().push(feature_id);
    }

    let missing_dependencies = adjacency
        .get(&feature_uuid)
        .into_iter()
        .flatten()
        .filter(|id| !features.contains_key(id))
        .map(Uuid::to_string)
        .collect::<Vec<_>>();

    let direct_dependencies = adjacency
        .get(&feature_uuid)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| features.get(&id).map(|row| (id, row)))
        .map(|(id, row)| DependencyImpactNode {
            id: id.to_string(),
            key: row.get("key"),
            lifecycle_stage: row.get("lifecycle_stage"),
            enabled: row.get("active"),
            reason: "Feature depends on this flag".to_string(),
            severity: "medium".to_string(),
        })
        .collect::<Vec<_>>();

    let direct_dependent_ids = reverse.get(&feature_uuid).cloned().unwrap_or_default();
    let direct_dependents = direct_dependent_ids
        .iter()
        .filter_map(|id| features.get(id).map(|row| (*id, row)))
        .map(|(id, row)| DependencyImpactNode {
            id: id.to_string(),
            key: row.get("key"),
            lifecycle_stage: row.get("lifecycle_stage"),
            enabled: row.get("active"),
            reason: "This feature depends on target flag".to_string(),
            severity: if matches!(action.as_str(), "archive" | "emergency-disable" | "rollback") {
                "high".to_string()
            } else {
                "medium".to_string()
            },
        })
        .collect::<Vec<_>>();

    let mut visited = HashSet::new();
    let mut queue = direct_dependent_ids.into_iter().collect::<VecDeque<_>>();
    let mut transitive_ids = Vec::new();
    while let Some(id) = queue.pop_front() {
        if !visited.insert(id) {
            continue;
        }
        transitive_ids.push(id);
        for next in reverse.get(&id).into_iter().flatten() {
            queue.push_back(*next);
        }
    }
    transitive_ids.retain(|id| *id != feature_uuid);

    let transitive_dependents = transitive_ids
        .iter()
        .skip(direct_dependents.len())
        .filter_map(|id| features.get(id).map(|row| (*id, row)))
        .map(|(id, row)| DependencyImpactNode {
            id: id.to_string(),
            key: row.get("key"),
            lifecycle_stage: row.get("lifecycle_stage"),
            enabled: row.get("active"),
            reason: "Transitively impacted dependent".to_string(),
            severity: "medium".to_string(),
        })
        .collect::<Vec<_>>();

    let cycles = detect_cycles_for_impact(&adjacency)
        .into_iter()
        .filter(|cycle| cycle.contains(&feature_uuid))
        .map(|cycle| {
            cycle
                .into_iter()
                .map(|id| {
                    features
                        .get(&id)
                        .map(|row| row.get::<String, _>("key"))
                        .unwrap_or_else(|| id.to_string())
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let severity = impact_severity(
        action.as_str(),
        direct_dependents.len(),
        transitive_dependents.len(),
        missing_dependencies.len(),
        cycles.len(),
    );
    let mut warnings = Vec::new();
    if !direct_dependents.is_empty() {
        warnings.push(format!(
            "{} direct dependent feature(s) may be impacted",
            direct_dependents.len()
        ));
    }
    if !cycles.is_empty() {
        warnings.push("Dependency cycle detected".to_string());
    }
    if !missing_dependencies.is_empty() {
        warnings.push("Missing dependency references detected".to_string());
    }

    Ok(HttpResponse::Ok().json(DependencyImpactResponse {
        feature_id: feature_uuid.to_string(),
        action: action.clone(),
        severity: severity.clone(),
        summary: format!(
            "{} risk for {}: {} direct dependencies, {} direct dependents",
            severity,
            root.get::<String, _>("key"),
            direct_dependencies.len(),
            direct_dependents.len()
        ),
        direct_dependencies,
        direct_dependents,
        transitive_dependents,
        missing_dependencies,
        cycles,
        requires_confirmation: severity == "high",
        warnings,
    }))
}

#[get("/teams/{team_id}/audit-analytics")]
pub(crate) async fn audit_analytics(
    db_pool: web::Data<sqlx::PgPool>,
    team_id: web::Path<String>,
    query: web::Query<AuditAnalyticsQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let window_days = query.window_days.unwrap_or(30).clamp(1, 365);
    let since = chrono::Utc::now() - chrono::Duration::days(window_days);
    let actor_id = match query.actor_id.as_deref() {
        Some(value) if !value.trim().is_empty() => Some(parse_uuid(value, "actor_id")?),
        _ => None,
    };
    let action = query.action.as_deref().filter(|value| !value.is_empty());
    let environment_id = query.environment_id.as_deref().filter(|value| !value.is_empty());

    let base_sql = r#"FROM activity_log al
            LEFT JOIN features f ON al.entity_type = 'feature' AND al.entity_id = f.id::text
            WHERE al.created_at >= $2
              AND (f.team_id = $1 OR al.metadata->>'team_id' = $1::text)
              AND ($3::uuid IS NULL OR al.actor_id = $3)
              AND ($4::text IS NULL OR al.activity_type = $4)
              AND ($5::text IS NULL OR al.metadata->>'environment_id' = $5)"#;

    let total_events: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) {base_sql}"))
        .bind(team_uuid)
        .bind(since)
        .bind(actor_id)
        .bind(action)
        .bind(environment_id)
        .fetch_one(db_pool.get_ref())
        .await
        .map_err(|e| RestError::internal(format!("Failed to count audit events: {e}")))?;

    let emergency_actions: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) {base_sql} AND al.activity_type ILIKE '%kill_switch%'"
    ))
    .bind(team_uuid)
    .bind(since)
    .bind(actor_id)
    .bind(action)
    .bind(environment_id)
    .fetch_one(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to count emergency actions: {e}")))?;

    let rollback_events: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) {base_sql} AND (al.activity_type ILIKE '%rollback%' OR al.description ILIKE '%rollback%')"
    ))
    .bind(team_uuid)
    .bind(since)
    .bind(actor_id)
    .bind(action)
    .bind(environment_id)
    .fetch_one(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to count rollback events: {e}")))?;

    let approval_stats = sqlx::query(
        r#"SELECT
              COUNT(*) FILTER (WHERE r.status = 'rejected')::float8 AS rejected,
              COUNT(*)::float8 AS total,
              COALESCE(AVG(EXTRACT(EPOCH FROM (COALESCE(r.executed_at, r.updated_at) - r.created_at))) / 3600.0, 0)::float8 AS avg_hours
           FROM approval_requests r
           JOIN approval_policies p ON p.id = r.policy_id
           WHERE p.team_id = $1
             AND r.created_at >= $2"#,
    )
    .bind(team_uuid)
    .bind(since)
    .fetch_one(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to calculate approval analytics: {e}")))?;
    let approval_total: f64 = approval_stats.get("total");
    let approval_rejected: f64 = approval_stats.get("rejected");
    let rejection_rate = if approval_total > 0.0 {
        approval_rejected / approval_total
    } else {
        0.0
    };
    let approval_lead_time_hours: f64 = approval_stats.get("avg_hours");

    let top_changed_features = sqlx::query(&format!(
        r#"SELECT COALESCE(f.id::text, al.entity_id) AS feature_id,
                  COALESCE(f.key, al.metadata->>'feature_key', al.entity_id) AS feature_key,
                  COUNT(*) AS change_count,
                  MAX(al.created_at) AS last_changed_at
           {base_sql}
           AND al.entity_type = 'feature'
           GROUP BY COALESCE(f.id::text, al.entity_id), COALESCE(f.key, al.metadata->>'feature_key', al.entity_id)
           ORDER BY change_count DESC, last_changed_at DESC
           LIMIT 10"#
    ))
    .bind(team_uuid)
    .bind(since)
    .bind(actor_id)
    .bind(action)
    .bind(environment_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load top changed features: {e}")))?
    .into_iter()
    .map(|row| AuditAnalyticsTopFeature {
        feature_id: row.get("feature_id"),
        feature_key: row.get("feature_key"),
        change_count: row.get("change_count"),
        last_changed_at: row.get("last_changed_at"),
    })
    .collect::<Vec<_>>();

    let action_breakdown = sqlx::query(&format!(
        r#"SELECT al.activity_type AS key, COUNT(*) AS count
           {base_sql}
           GROUP BY al.activity_type
           ORDER BY count DESC
           LIMIT 12"#
    ))
    .bind(team_uuid)
    .bind(since)
    .bind(actor_id)
    .bind(action)
    .bind(environment_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load action breakdown: {e}")))?
    .into_iter()
    .map(|row| AuditAnalyticsBreakdownRow {
        key: row.get("key"),
        count: row.get("count"),
    })
    .collect::<Vec<_>>();

    let actor_breakdown = sqlx::query(&format!(
        r#"SELECT COALESCE(al.actor_name, al.actor_id::text, 'System') AS key, COUNT(*) AS count
           {base_sql}
           GROUP BY COALESCE(al.actor_name, al.actor_id::text, 'System')
           ORDER BY count DESC
           LIMIT 12"#
    ))
    .bind(team_uuid)
    .bind(since)
    .bind(actor_id)
    .bind(action)
    .bind(environment_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load actor breakdown: {e}")))?
    .into_iter()
    .map(|row| AuditAnalyticsBreakdownRow {
        key: row.get("key"),
        count: row.get("count"),
    })
    .collect::<Vec<_>>();

    let recent_events = sqlx::query(&format!(
        r#"SELECT al.id, al.activity_type, al.entity_type, al.entity_id,
                  al.actor_name, al.description, al.created_at
           {base_sql}
           ORDER BY al.created_at DESC
           LIMIT 20"#
    ))
    .bind(team_uuid)
    .bind(since)
    .bind(actor_id)
    .bind(action)
    .bind(environment_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|e| RestError::internal(format!("Failed to load audit events: {e}")))?
    .into_iter()
    .map(|row| AuditAnalyticsEvent {
        id: row.get::<Uuid, _>("id").to_string(),
        activity_type: row.get("activity_type"),
        entity_type: row.get("entity_type"),
        entity_id: row.get("entity_id"),
        actor_name: row.get("actor_name"),
        description: row.get("description"),
        created_at: row.get("created_at"),
    })
    .collect::<Vec<_>>();

    Ok(HttpResponse::Ok().json(AuditAnalyticsResponse {
        total_events,
        emergency_actions,
        rollback_events,
        rejection_rate,
        approval_lead_time_hours,
        top_changed_features,
        action_breakdown,
        actor_breakdown,
        recent_events,
        generated_at: chrono::Utc::now(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/{id}/versions",
    params(
        ("id" = String, Path, description = "Feature ID"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Feature version history", body = FeatureVersionsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/{id}/versions")]
pub(crate) async fn list_feature_versions(
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    feature_id: web::Path<String>,
    query: web::Query<PaginationQuery>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });
    let (versions, total) = feature_repo
        .list_feature_versions(feature_uuid, offset, limit)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(FeatureVersionsResponse {
        items: versions.into_iter().map(map_feature_version).collect(),
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/{id}/versions/{version_id}/diff",
    params(
        ("id" = String, Path, description = "Feature ID"),
        ("version_id" = String, Path, description = "Version ID")
    ),
    responses(
        (status = 200, description = "Feature version diff", body = FeatureVersionDiffResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/{id}/versions/{version_id}/diff")]
pub(crate) async fn get_feature_version_diff(
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    path: web::Path<(String, String)>,
) -> Result<impl Responder, RestError> {
    let (feature_id, version_id) = path.into_inner();
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let version_uuid = parse_uuid(&version_id, "version id")?;
    let version = feature_repo
        .get_feature_version(feature_uuid, version_uuid)
        .await
        .map_err(RestError::from)?;
    let entries = parse_change_summary(version.change_summary);

    Ok(HttpResponse::Ok().json(FeatureVersionDiffResponse {
        version_id: version.id.to_string(),
        version_number: version.version_number,
        entries,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/versions/{version_id}/rollback",
    request_body = RollbackFeatureVersionRequest,
    params(
        ("id" = String, Path, description = "Feature ID"),
        ("version_id" = String, Path, description = "Version ID")
    ),
    responses(
        (status = 200, description = "Feature rolled back", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/features/{id}/versions/{version_id}/rollback")]
pub(crate) async fn rollback_feature_version(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    path: web::Path<(String, String)>,
    payload: web::Json<RollbackFeatureVersionRequest>,
) -> Result<impl Responder, RestError> {
    let (feature_id, version_id) = path.into_inner();
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let version_uuid = parse_uuid(&version_id, "version id")?;
    let actor = actor_from_request(&req);
    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = feature_tx::rollback_feature_to_version_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(feature_uuid),
        ID::from(version_uuid),
        payload.archive_confirmation.unwrap_or(false),
        actor,
    )
    .await;

    let rolled_back = match result {
        Ok(feature) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    };

    broadcast_feature_update(
        feature_repo.as_ref().as_ref(),
        updates_tx.get_ref(),
        feature_uuid,
    )
    .await;

    let response = build_feature_response(
        &rolled_back,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/features",
    request_body = CreateFeatureRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Feature created", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/teams/{team_id}/features")]
pub(crate) async fn create_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    pipeline_logic: web::Data<Box<dyn PipelineLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    team_id: web::Path<String>,
    payload: web::Json<CreateFeatureRequest>,
) -> Result<impl Responder, RestError> {
    validate_feature_key_create(&payload.key)?;
    validate_description_create(&payload.description)?;
    validate_variant_requests(&payload.variants)?;

    let team_uuid = parse_uuid(&team_id, "team_id")?;
    ensure_feature_key_unique_for_create(
        pipeline_logic.as_ref().as_ref(),
        ID::from(team_uuid),
        payload.key.as_str(),
    )
    .await?;

    let stages = map_stage_requests(&payload.stages)?;
    let relationships = map_relationship_requests(&payload.relationships)?;
    validate_feature_structure(&stages, &relationships)?;

    let dependencies = map_dependencies(&payload.dependencies)?;
    let variants = payload.variants.as_ref().map(|list| {
        list.iter()
            .cloned()
            .map(|variant| CreateFeatureVariantInput {
                control: variant.control,
                value: variant.value,
                value_type: ModelVariantValueType::from(variant.value_type),
                description: variant.description,
            })
            .collect::<Vec<_>>()
    });

    if payload.feature_type == FeatureType::Simple
        && let Some(ref list) = variants
        && !list.is_empty()
    {
        return Err(RestError::invalid_input(
            "Variants can only be defined for Contextual features, not Simple features",
        ));
    }

    let input = CreateFeatureInput {
        key: payload.key.clone(),
        description: payload.description.clone(),
        feature_type: ModelFeatureType::from(payload.feature_type),
        enabled: payload.enabled,
        lifecycle_stage: payload.lifecycle_stage.map(ModelLifecycleStage::from),
        owner: payload.owner.clone(),
        purpose: payload.purpose.clone(),
        reference_url: payload.reference_url.clone(),
        expires_at: payload.expires_at,
        cleanup_reason: payload.cleanup_reason.clone(),
        tags: payload.tags.clone(),
        dependencies,
        relationships,
        stages,
        variants,
    };

    let actor = actor_from_request(&req);
    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::feature_tx::create_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(team_uuid),
        input,
        actor,
    )
    .await;

    let feature_id = match result {
        Ok(feature_id) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature_id
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    };

    let feature = feature_logic
        .get_feature_by_id(feature_id)
        .await
        .map_err(RestError::from)?;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Created().json(response))
}

#[utoipa::path(
    patch,
    path = "/api/v1/features/{id}",
    request_body = UpdateFeatureRequest,
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature updated", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[patch("/features/{id}")]
pub(crate) async fn update_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    feature_id: web::Path<String>,
    payload: web::Json<UpdateFeatureRequest>,
) -> Result<impl Responder, RestError> {
    validate_feature_key_update(&payload.key)?;
    validate_variant_requests(&payload.variants)?;

    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let existing_feature = ensure_feature_key_unique_for_update(
        feature_logic.as_ref().as_ref(),
        &ID::from(feature_uuid),
        &payload.key,
    )
    .await?;

    let stages = map_stage_requests(&payload.stages)?;
    let relationships = map_relationship_requests(&payload.relationships)?;
    validate_feature_structure(&stages, &relationships)?;

    let dependencies = map_dependencies(&payload.dependencies)?;
    let variants = payload.variants.as_ref().map(|list| {
        list.iter()
            .cloned()
            .map(|variant| CreateFeatureVariantInput {
                control: variant.control,
                value: variant.value,
                value_type: ModelVariantValueType::from(variant.value_type),
                description: variant.description,
            })
            .collect::<Vec<_>>()
    });

    if payload.feature_type == FeatureType::Simple
        && let Some(ref list) = variants
        && !list.is_empty()
    {
        return Err(RestError::invalid_input(
            "Variants can only be defined for Contextual features, not Simple features",
        ));
    }

    let jwt_user = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    let team_uuid = Uuid::try_from(existing_feature.team_id.clone())
        .map_err(|_| RestError::invalid_input("invalid feature team id"))?;
    for stage in &payload.stages {
        let environment_uuid = parse_uuid(&stage.environment_id, "environment_id")?;
        crate::rest::operational_safety::enforce_freeze_for_feature_environment(
            db_pool.get_ref(),
            activity_repo.as_ref().as_ref(),
            team_uuid,
            feature_uuid,
            existing_feature.key.as_str(),
            environment_uuid,
            &jwt_user,
            payload.freeze_override_reason.as_deref(),
        )
        .await?;
    }

    let input = UpdateFeatureInput {
        key: payload.key.clone(),
        description: payload.description.clone(),
        feature_type: ModelFeatureType::from(payload.feature_type),
        enabled: payload.enabled,
        lifecycle_stage: payload.lifecycle_stage.map(ModelLifecycleStage::from),
        owner: payload.owner.clone().map(Some),
        purpose: payload.purpose.clone().map(Some),
        reference_url: payload.reference_url.clone().map(Some),
        expires_at: payload.expires_at.map(Some),
        cleanup_reason: payload.cleanup_reason.clone().map(Some),
        tags: payload.tags.clone(),
        archive_confirmation: payload.archive_confirmation.unwrap_or(false),
        dependencies,
        relationships,
        stages,
        variants,
    };

    let actor = actor_from_request(&req);
    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::feature_tx::update_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        existing_feature.id.clone(),
        input,
        actor,
    )
    .await;

    let updated = match result {
        Ok(feature) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature
        }
        Err(err) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(err));
        }
    };

    if let Ok(fid) = Uuid::try_from(existing_feature.id.clone()) {
        broadcast_feature_update(feature_repo.as_ref().as_ref(), updates_tx.get_ref(), fid).await;
    }

    let response = build_feature_response(
        &updated,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/emergency-disable",
    request_body = EmergencyDisableRequest,
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature emergency disabled", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/features/{id}/emergency-disable")]
pub(crate) async fn emergency_disable_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    feature_id: web::Path<String>,
    payload: web::Json<EmergencyDisableRequest>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let jwt_user = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    ensure_emergency_override_allowed(&jwt_user)?;
    let actor = Some(ActorContext::new(jwt_user.id, jwt_user.username.clone()));
    let reason = validate_emergency_reason(&payload.reason)?;
    validate_emergency_expiry(payload.expires_at)?;

    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let result = feature_tx::emergency_disable_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(feature_uuid),
        payload.rollback_in_minutes,
        reason,
        payload.expires_at,
        actor,
    )
    .await;

    let feature = match result {
        Ok(feature) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature
        }
        Err(e) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(e));
        }
    };

    broadcast_feature_update(
        feature_repo.as_ref().as_ref(),
        updates_tx.get_ref(),
        feature_uuid,
    )
    .await;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/features/{id}/emergency-enable",
    request_body = EmergencyEnableRequest,
    params(
        ("id" = String, Path, description = "Feature ID")
    ),
    responses(
        (status = 200, description = "Feature emergency enabled", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/features/{id}/emergency-enable")]
pub(crate) async fn emergency_enable_feature(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    feature_id: web::Path<String>,
    payload: web::Json<EmergencyEnableRequest>,
) -> Result<impl Responder, RestError> {
    let feature_uuid = parse_uuid(&feature_id, "feature id")?;
    let jwt_user = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    ensure_emergency_override_allowed(&jwt_user)?;
    let actor = Some(ActorContext::new(jwt_user.id, jwt_user.username.clone()));
    let reason = validate_emergency_reason(&payload.reason)?;

    let repo_tx = feature_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to begin transaction: {e}")))?;

    let result = feature_tx::emergency_enable_feature_in_tx(
        &mut tx,
        &repo_tx,
        activity_repo.as_ref().as_ref(),
        ID::from(feature_uuid),
        reason,
        actor,
    )
    .await;

    let feature = match result {
        Ok(feature) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            feature
        }
        Err(e) => {
            let _ = tx.rollback().await;
            return Err(RestError::from(e));
        }
    };

    broadcast_feature_update(
        feature_repo.as_ref().as_ref(),
        updates_tx.get_ref(),
        feature_uuid,
    )
    .await;

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    post,
    path = "/api/v1/stages/{id}/request-change",
    request_body = StageChangeRequestBody,
    params(
        ("id" = String, Path, description = "Stage ID")
    ),
    responses(
        (status = 200, description = "Stage change requested", body = FeatureResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[post("/stages/{id}/request-change")]
pub(crate) async fn request_stage_change(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    stage_id: web::Path<String>,
    payload: web::Json<StageChangeRequestBody>,
) -> Result<impl Responder, RestError> {
    let stage_uuid = parse_uuid(&stage_id, "stage id")?;
    let jwt_user = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;

    RoleAuthorizer::authorize_stage_change_request(&jwt_user.roles, payload.request.as_str())
        .map_err(|e| RestError::forbidden(e.to_string()))?;

    crate::rest::operational_safety::enforce_freeze_for_stage(
        db_pool.get_ref(),
        activity_repo.as_ref().as_ref(),
        stage_uuid,
        &jwt_user,
        payload.freeze_override_reason.as_deref(),
    )
    .await?;

    let request_type = StageChangeRequestType::from(payload.request);
    let feature = feature_logic
        .request_stage_change(ID::from(stage_uuid), request_type, jwt_user.id)
        .await
        .map_err(RestError::from)?;

    if let Ok(fid) = Uuid::try_from(feature.id.clone()) {
        broadcast_feature_update(feature_repo.as_ref().as_ref(), updates_tx.get_ref(), fid).await;
    }

    let response = build_feature_response(
        &feature,
        feature_repo.as_ref().as_ref(),
        env_logic.as_ref().as_ref(),
        true,
        true,
        true,
    )
    .await?;

    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/pending-approvals",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Pending approvals", body = FeaturesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/pending-approvals")]
pub(crate) async fn pending_approvals(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    query: web::Query<FeatureRolloutQuery>,
) -> Result<impl Responder, RestError> {
    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let (features, total) = feature_logic
        .get_features_with_pending_approvals_with_offset(team_id, offset, limit)
        .await
        .map_err(RestError::from)?;

    let mut items = Vec::with_capacity(features.len());
    for feature in features.iter() {
        let mut response = feature_base_response(feature);
        response.stages = Some(
            load_stage_data(
                feature_repo.as_ref().as_ref(),
                env_logic.as_ref().as_ref(),
                parse_uuid(&feature.id.to_string(), "feature id")?,
            )
            .await?
            .0,
        );
        items.push(response);
    }

    Ok(HttpResponse::Ok().json(FeaturesResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/active-kill-switches",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Active kill switches", body = FeaturesResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/active-kill-switches")]
pub(crate) async fn active_kill_switches(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    env_logic: web::Data<Box<dyn EnvironmentLogic>>,
    query: web::Query<FeatureRolloutQuery>,
) -> Result<impl Responder, RestError> {
    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let (features, total) = feature_logic
        .get_features_with_kill_switches_with_offset(team_id, offset, limit)
        .await
        .map_err(RestError::from)?;

    let mut items = Vec::with_capacity(features.len());
    for feature in features.iter() {
        let mut response = feature_base_response(feature);
        response.stages = Some(
            load_stage_data(
                feature_repo.as_ref().as_ref(),
                env_logic.as_ref().as_ref(),
                parse_uuid(&feature.id.to_string(), "feature id")?,
            )
            .await?
            .0,
        );
        items.push(response);
    }

    Ok(HttpResponse::Ok().json(FeaturesResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/features/rollout-metrics",
    params(
        ("teamId" = Option<String>, Query, description = "Filter by team")
    ),
    responses(
        (status = 200, description = "Rollout metrics", body = RolloutMetricsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Features"
)]
#[get("/features/rollout-metrics")]
pub(crate) async fn rollout_metrics(
    feature_logic: web::Data<Box<dyn FeatureLogic>>,
    query: web::Query<RolloutMetricsQuery>,
) -> Result<impl Responder, RestError> {
    let team_id = match query.team_id.as_deref() {
        Some(value) => Some(ID::from(parse_uuid(value, "team_id")?)),
        None => None,
    };

    let metrics = feature_logic
        .get_rollout_metrics(team_id)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(RolloutMetricsResponse {
        average_time_in_pipeline: metrics.average_time_in_pipeline,
        approval_rate: metrics.approval_rate,
        features_deployed_this_week: metrics.features_deployed_this_week,
        features_deployed_last_week: metrics.features_deployed_last_week,
        deployment_change: metrics.deployment_change,
        bottleneck_stage: metrics.bottleneck_stage,
        bottleneck_duration: metrics.bottleneck_duration,
        total_pending_approvals: metrics.total_pending_approvals,
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_features)
        // Register static paths before /features/{feature_id} to avoid path conflicts.
        .service(rollout_metrics)
        .service(pending_approvals)
        .service(active_kill_switches)
        .service(audit_analytics)
        .service(bulk_feature_action)
        .service(list_feature_versions)
        .service(get_feature_version_diff)
        .service(rollback_feature_version)
        .service(dependency_impact)
        .service(get_feature)
        .service(create_feature)
        .service(update_feature)
        .service(emergency_disable_feature)
        .service(emergency_enable_feature)
        .service(request_stage_change);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::PgActivityLogRepository;
    use crate::database::environment::environment_repository;
    use crate::database::feature::{MockFeatureRepository, feature_repository};
    use crate::database::user::user_repository;
    use crate::logic::environment::{MockEnvironmentLogic, environment_logic};
    use crate::logic::feature::{MockFeatureLogic, feature_logic};
    use crate::logic::pipeline::MockPipelineLogic;
    use crate::model::{
        Feature as ModelFeature, FeatureType as ModelFeatureType,
        LifecycleStage as ModelLifecycleStage,
    };
    use actix_web::{App, http::StatusCode, test};
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    fn sample_feature(feature_id: Uuid, team_id: Uuid) -> ModelFeature {
        ModelFeature {
            id: ID::from(feature_id),
            key: "checkout".to_string(),
            description: Some("Test feature".to_string()),
            feature_type: ModelFeatureType::Simple,
            enabled: true,
            created_at: chrono::Utc::now(),
            kill_switch_enabled: true,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            lifecycle_stage: ModelLifecycleStage::Active,
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            is_stale: false,
            stale_reasons: vec![],
            dependencies: vec![],
            team_id: ID::from(team_id),
            pending_approval_request_id: None,
        }
    }

    #[actix_web::test]
    async fn emergency_reason_validation_requires_clear_reason() {
        assert!(validate_emergency_reason("    ").is_err());
        assert!(validate_emergency_reason("fix").is_err());
        assert_eq!(
            validate_emergency_reason("  Incident mitigation  ").unwrap(),
            "Incident mitigation"
        );
    }

    #[actix_web::test]
    async fn emergency_override_requires_admin_or_team_admin() {
        let base_user = JwtUser {
            id: Uuid::new_v4(),
            username: "user".to_string(),
            is_admin: false,
            roles: vec![],
            team_id: None,
            token_hash: "hash".to_string(),
        };

        assert!(ensure_emergency_override_allowed(&base_user).is_err());

        let mut team_admin = base_user.clone();
        team_admin.roles = vec!["Team Admin".to_string()];
        assert!(ensure_emergency_override_allowed(&team_admin).is_ok());

        let mut system_admin = base_user;
        system_admin.is_admin = true;
        assert!(ensure_emergency_override_allowed(&system_admin).is_ok());
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
        let name = format!("feature-test-{}", team_id);
        sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)"#,
            team_id,
            name,
            "feature test team"
        )
        .execute(pool)
        .await
        .expect("Failed to insert team");
        team_id
    }

    async fn insert_environment(pool: &sqlx::PgPool, team_id: Uuid) -> Uuid {
        let env_id = Uuid::new_v4();
        let name = format!("env-{}", env_id);
        sqlx::query!(
            r#"INSERT INTO environments (id, name, active, team_id, environment_type)
               VALUES ($1, $2, $3, $4, $5)"#,
            env_id,
            name,
            true,
            team_id,
            "Production"
        )
        .execute(pool)
        .await
        .expect("Failed to insert environment");
        env_id
    }

    #[actix_web::test]
    async fn list_features_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let feature = sample_feature(feature_id, team_id);

        let mut mock_logic = MockFeatureLogic::new();
        mock_logic
            .expect_get_features_with_offset_filtered()
            .withf(
                move |id,
                      name,
                      feature_type,
                      lifecycle_stage,
                      stale,
                      include_archived,
                      owner,
                      expired,
                      tag,
                      dependency_status,
                      approval_status,
                      offset,
                      limit| {
                    id.to_string() == team_id.to_string()
                        && name.as_deref() == Some("check")
                        && matches!(feature_type, Some(ModelFeatureType::Simple))
                        && lifecycle_stage.is_none()
                        && stale.is_none()
                        && !*include_archived
                        && owner.is_none()
                        && expired.is_none()
                        && tag.is_none()
                        && dependency_status.is_none()
                        && approval_status.is_none()
                        && *offset == 10
                        && *limit == 5
                },
            )
            .times(1)
            .returning(move |_, _, _, _, _, _, _, _, _, _, _, _, _| Ok((vec![feature.clone()], 1)));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Box::new(mock_logic) as Box<dyn FeatureLogic>))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!(
            "/api/v1/teams/{team_id}/features?offset=10&limit=5&name=check&featureType=SIMPLE"
        );
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], feature_id.to_string());
        assert_eq!(json["meta"]["offset"], 10);
        assert_eq!(json["meta"]["limit"], 5);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn create_feature_returns_created() {
        let pool = test_pool().await;
        let team_id = insert_team(&pool).await;
        let env_id = insert_environment(&pool, team_id).await;

        let mut mock_pipeline_logic = MockPipelineLogic::new();
        mock_pipeline_logic
            .expect_get_pipelines()
            .returning(|_, _, _, _| Ok(vec![]));

        let activity_repo = PgActivityLogRepository::new(pool.clone());
        let env_logic_for_handler = environment_logic(
            environment_repository(pool.clone()),
            Box::new(PgActivityLogRepository::new(pool.clone())),
        );
        let env_logic_for_feature = environment_logic(
            environment_repository(pool.clone()),
            Box::new(PgActivityLogRepository::new(pool.clone())),
        );
        let feature_logic = feature_logic(
            feature_repository(pool.clone()),
            env_logic_for_feature,
            Box::new(PgActivityLogRepository::new(pool.clone())),
            user_repository(pool.clone()),
        );
        let feature_repo = feature_repository(pool.clone());

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(feature_logic))
                .app_data(web::Data::new(
                    Box::new(mock_pipeline_logic) as Box<dyn PipelineLogic>
                ))
                .app_data(web::Data::new(feature_repo))
                .app_data(web::Data::new(env_logic_for_handler))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/teams/{team_id}/features");
        let req = test::TestRequest::post()
            .uri(&uri)
            .set_json(CreateFeatureRequest {
                key: "checkout".to_string(),
                description: Some("Test feature".to_string()),
                feature_type: FeatureType::Simple,
                enabled: Some(true),
                lifecycle_stage: None,
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: None,
                dependencies: vec![],
                relationships: vec![],
                stages: vec![CreateFeatureStageRequest {
                    id: None,
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                    bucketing_key: None,
                }],
                variants: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["key"], "checkout");
        assert_eq!(json["teamId"], team_id.to_string());
    }

    #[actix_web::test]
    async fn update_feature_duplicate_name_returns_conflict() {
        let pool = test_pool().await;
        let activity_repo = PgActivityLogRepository::new(pool.clone());
        let team_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        let feature = sample_feature(feature_id, team_id);
        let mut duplicate = sample_feature(Uuid::new_v4(), team_id);
        duplicate.key = "checkout".to_string();

        let mut mock_feature_logic = MockFeatureLogic::new();
        mock_feature_logic
            .expect_get_feature_by_id()
            .withf(move |id| id.to_string() == feature_id.to_string())
            .times(1)
            .returning(move |_| Ok(feature.clone()));
        mock_feature_logic
            .expect_get_features()
            .withf(move |id, name, _| {
                id.to_string() == team_id.to_string() && name.as_deref() == Some("checkout")
            })
            .times(1)
            .returning(move |_, _, _| Ok(vec![duplicate.clone()]));

        let mock_feature_repo = MockFeatureRepository::new();
        let mock_env_logic = MockEnvironmentLogic::new();
        let (updates_tx, _updates_rx) =
            tokio::sync::broadcast::channel::<crate::grpc::pb::FeatureUpdate>(1);

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(activity_repo) as Box<dyn ActivityLogRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_feature_logic) as Box<dyn FeatureLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_feature_repo) as Box<dyn FeatureRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_env_logic) as Box<dyn EnvironmentLogic>
                ))
                .app_data(web::Data::new(updates_tx))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/features/{feature_id}");
        let req = test::TestRequest::patch()
            .uri(&uri)
            .set_json(UpdateFeatureRequest {
                key: "checkout".to_string(),
                description: None,
                feature_type: FeatureType::Simple,
                enabled: Some(true),
                lifecycle_stage: None,
                owner: None,
                purpose: None,
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: None,
                archive_confirmation: None,
                dependencies: vec![],
                relationships: vec![],
                stages: vec![CreateFeatureStageRequest {
                    id: None,
                    environment_id: env_id.to_string(),
                    order_index: 0,
                    position: "{\"x\":0,\"y\":0}".to_string(),
                    bucketing_key: None,
                }],
                variants: None,
                freeze_override_reason: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "conflict");
    }
}
