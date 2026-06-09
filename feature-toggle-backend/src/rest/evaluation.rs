use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, post, web};
use evaluation_engine as engine;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet, VecDeque};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::feature::{FeatureRepository, feature_repository};
use crate::rest::error::RestError;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateRequest {
    pub team_id: Option<String>,
    pub feature_key: String,
    pub environment_id: String,
    pub targeting_key: String,
    #[serde(default)]
    pub context: HashMap<String, JsonValue>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateResponse {
    pub flag_key: String,
    pub value: JsonValue,
    pub variant: Option<String>,
    pub reason: String,
    pub error_code: Option<String>,
}

#[derive(Clone)]
struct EngineFeatureBase {
    id: String,
    key: String,
    feature_type: String,
    active: bool,
    enabled: bool,
    dependency_ids: Vec<Uuid>,
    stages: Vec<engine::FeatureStage>,
    variants: Vec<engine::FeatureVariant>,
}

fn map_operator(value: &str) -> engine::Operator {
    match value.to_uppercase().as_str() {
        "EQUALS" => engine::Operator::Equals,
        "NOTEQUALS" | "NOT_EQUALS" => engine::Operator::NotEquals,
        "GREATERTHAN" | "GREATER_THAN" => engine::Operator::GreaterThan,
        "LESSTHAN" | "LESS_THAN" => engine::Operator::LessThan,
        "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => engine::Operator::GreaterThanOrEqual,
        "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => engine::Operator::LessThanOrEqual,
        "CONTAINS" => engine::Operator::Contains,
        "STARTSWITH" | "STARTS_WITH" => engine::Operator::StartsWith,
        "ENDSWITH" | "ENDS_WITH" => engine::Operator::EndsWith,
        "REGEX" => engine::Operator::Regex,
        "IN" => engine::Operator::In,
        "NOTIN" | "NOT_IN" => engine::Operator::NotIn,
        "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => engine::Operator::SemverGreaterThan,
        "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => engine::Operator::SemverLessThan,
        _ => engine::Operator::In,
    }
}

async fn map_feature_payload(
    repo: &dyn FeatureRepository,
    feature: &crate::database::entity::Feature,
) -> Result<(Vec<engine::FeatureStage>, Vec<engine::FeatureVariant>), RestError> {
    let db_stages = repo
        .get_feature_stages(feature.id)
        .await
        .map_err(|err| RestError::internal(format!("Failed to load feature stages: {err}")))?;

    let mut stages = Vec::with_capacity(db_stages.len());
    for stage in db_stages {
        let criteria = repo
            .get_stage_criteria(stage.id)
            .await
            .map_err(|err| RestError::internal(format!("Failed to load stage criteria: {err}")))?;

        let criterias = criteria
            .into_iter()
            .map(|criterion| {
                let rule_groups = criterion
                    .rule_groups
                    .into_iter()
                    .map(|group| engine::RuleGroup {
                        logic_operator: match group.logic_operator {
                            crate::database::entity::LogicOperator::And => {
                                engine::LogicOperator::And
                            }
                            crate::database::entity::LogicOperator::Or => engine::LogicOperator::Or,
                        },
                        conditions: group
                            .conditions
                            .into_iter()
                            .map(|condition| engine::RuleCondition {
                                context_key: condition.context_key,
                                operator: map_operator(&condition.operator),
                                value: condition.value,
                            })
                            .collect(),
                    })
                    .collect();

                engine::StageCriterion {
                    priority: criterion.priority,
                    rule_groups,
                    variant_allocations: criterion
                        .variant_allocations
                        .into_iter()
                        .map(|allocation| engine::VariantAllocation {
                            variant_control: allocation.variant_control,
                            weight: allocation.weight,
                        })
                        .collect(),
                    variant_selection_mode: match criterion.variant_selection_mode {
                        crate::database::entity::VariantSelectionMode::SpecificVariant => {
                            engine::VariantSelectionMode::SpecificVariant
                        }
                        crate::database::entity::VariantSelectionMode::WeightedSplit => {
                            engine::VariantSelectionMode::WeightedSplit
                        }
                    },
                    selected_variant_control: criterion.selected_variant_control,
                }
            })
            .collect();

        stages.push(engine::FeatureStage {
            environment_id: stage.environment_id.to_string(),
            enabled: stage.enabled,
            criterias,
        });
    }

    let variants = if matches!(
        feature.feature_type,
        crate::database::entity::FeatureType::Contextual
    ) {
        repo.get_feature_variants(feature.id)
            .await
            .map_err(|err| RestError::internal(format!("Failed to load variants: {err}")))?
            .into_iter()
            .map(|variant| engine::FeatureVariant {
                control: variant.control,
                value: variant.value,
            })
            .collect()
    } else {
        vec![]
    };

    Ok((stages, variants))
}

fn build_engine_feature(
    feature_id: Uuid,
    base_map: &HashMap<Uuid, EngineFeatureBase>,
    memo: &mut HashMap<Uuid, engine::Feature>,
    visiting: &mut HashSet<Uuid>,
) -> engine::Feature {
    if let Some(cached) = memo.get(&feature_id) {
        return cached.clone();
    }

    let Some(base) = base_map.get(&feature_id) else {
        return engine::Feature {
            id: feature_id.to_string(),
            key: feature_id.to_string(),
            feature_type: "Simple".to_string(),
            active: false,
            enabled: false,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        };
    };

    if !visiting.insert(feature_id) {
        return engine::Feature {
            id: base.id.clone(),
            key: base.key.clone(),
            feature_type: base.feature_type.clone(),
            active: false,
            enabled: false,
            dependencies: vec![],
            stages: base.stages.clone(),
            variants: base.variants.clone(),
        };
    }

    let dependencies = base
        .dependency_ids
        .iter()
        .map(|dependency_id| build_engine_feature(*dependency_id, base_map, memo, visiting))
        .collect();

    visiting.remove(&feature_id);

    let feature = engine::Feature {
        id: base.id.clone(),
        key: base.key.clone(),
        feature_type: base.feature_type.clone(),
        active: base.active,
        enabled: base.enabled,
        dependencies,
        stages: base.stages.clone(),
        variants: base.variants.clone(),
    };
    memo.insert(feature_id, feature.clone());
    feature
}

async fn load_engine_feature(
    repo: &dyn FeatureRepository,
    root: crate::database::entity::Feature,
) -> Result<engine::Feature, RestError> {
    let root_id = root.id;
    let mut graph = HashMap::new();
    let mut queue: VecDeque<Uuid> = root
        .dependencies
        .iter()
        .map(|dependency| dependency.depends_on_id)
        .collect();
    graph.insert(root.id, root);

    while let Some(feature_id) = queue.pop_front() {
        if graph.contains_key(&feature_id) {
            continue;
        }
        let dependency = repo
            .get_feature_by_id(feature_id)
            .await
            .map_err(|err| RestError::internal(format!("Failed to load dependency: {err}")))?;
        for nested in &dependency.dependencies {
            if !graph.contains_key(&nested.depends_on_id) {
                queue.push_back(nested.depends_on_id);
            }
        }
        graph.insert(dependency.id, dependency);
    }

    let mut base_map = HashMap::new();
    for feature in graph.values() {
        let (stages, variants) = map_feature_payload(repo, feature).await?;
        base_map.insert(
            feature.id,
            EngineFeatureBase {
                id: feature.id.to_string(),
                key: feature.key.clone(),
                feature_type: format!("{:?}", feature.feature_type),
                active: feature.active,
                enabled: feature.active && feature.kill_switch_enabled,
                dependency_ids: feature
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.depends_on_id)
                    .collect(),
                stages,
                variants,
            },
        );
    }

    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    Ok(build_engine_feature(
        root_id,
        &base_map,
        &mut memo,
        &mut visiting,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/evaluate",
    request_body = EvaluateRequest,
    responses(
        (status = 200, description = "Feature evaluation result", body = EvaluateResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Feature not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Evaluation"
)]
#[post("/evaluate")]
pub(crate) async fn evaluate_feature(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    payload: web::Json<EvaluateRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = req
        .extensions()
        .get::<crate::JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;

    let body = payload.into_inner();
    let team_id = if let Some(team_id) = jwt.team_id {
        team_id
    } else {
        Uuid::parse_str(
            body.team_id
                .as_deref()
                .ok_or_else(|| RestError::invalid_input("teamId is required"))?,
        )
        .map_err(|_| RestError::invalid_input("invalid teamId"))?
    };

    if body.feature_key.trim().is_empty() {
        return Err(RestError::invalid_input("featureKey is required"));
    }
    if body.environment_id.trim().is_empty() {
        return Err(RestError::invalid_input("environmentId is required"));
    }
    if body.targeting_key.trim().is_empty() {
        return Err(RestError::invalid_input("targetingKey is required"));
    }

    let repo = feature_repository(pool.get_ref().clone());
    let mut features = repo
        .get_features(team_id, Some(body.feature_key.trim().to_string()), None)
        .await
        .map_err(|err| RestError::internal(format!("Failed to load feature: {err}")))?;
    let db_feature = features
        .pop()
        .ok_or_else(|| RestError::not_found("feature not found"))?;

    let engine_feature = load_engine_feature(repo.as_ref(), db_feature.clone()).await?;
    let result = engine::evaluate(
        &engine::FeatureEvaluationContext {
            flag_key: db_feature.key.clone(),
            context: engine::ContextObject {
                targeting_key: body.targeting_key,
                environment_id: body.environment_id,
                attributes: body.context,
            },
        },
        &engine_feature,
    );

    Ok(HttpResponse::Ok().json(EvaluateResponse {
        flag_key: result.flag_key,
        value: result.value,
        variant: result.variant,
        reason: result.reason.as_str().to_string(),
        error_code: result.error_code.map(|code| code.as_str().to_string()),
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(evaluate_feature);
}
