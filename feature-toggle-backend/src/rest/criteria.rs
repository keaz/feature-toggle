use actix_web::{delete, get, patch, post, put, web, HttpResponse, Responder};
use crate::model::ID;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::database::compound_rules::{
    compound_rules_repository_tx, CompoundRulesRepository, CompoundRulesRepositoryTx,
    CreateRuleConditionInput as DbCreateRuleConditionInput,
    CreateRuleGroupInput as DbCreateRuleGroupInput, UpdateRuleGroupInput as DbUpdateRuleGroupInput,
};
use crate::database::entity::LogicOperator as DbLogicOperator;
use crate::database::feature::FeatureRepository;
use crate::database::variant_allocations::{
    variant_allocations_repository_tx,
    CreateVariantAllocationInput as DbCreateVariantAllocationInput, VariantAllocationsRepositoryTx,
};
use crate::model::{
    CompoundRuleCondition as GqlCompoundRuleCondition, CompoundRuleGroup as GqlCompoundRuleGroup,
    CreateRuleConditionInput as GqlCreateRuleConditionInput,
    CreateStageCriterionInput as GqlCreateStageCriterionInput,
    CreateVariantAllocationInput as GqlCreateVariantAllocationInput,
    InlineRuleGroupInput as GqlInlineRuleGroupInput, LogicOperator as GqlLogicOperator,
    RuleOperator as GqlRuleOperator, StageCriterion as GqlStageCriterion,
    VariantAllocation as GqlVariantAllocation, VariantSelectionMode as GqlVariantSelectionMode,
};
use crate::logic::feature::FeatureLogic;
use crate::logic::feature_tx;
use crate::rest::error::RestError;

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuleOperator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Contains,
    StartsWith,
    EndsWith,
    Regex,
    In,
    NotIn,
    SemverGreaterThan,
    SemverLessThan,
}

impl RuleOperator {
    fn to_db_string(self) -> String {
        match self {
            RuleOperator::Equals => "EQUALS".to_string(),
            RuleOperator::NotEquals => "NOT_EQUALS".to_string(),
            RuleOperator::GreaterThan => "GREATER_THAN".to_string(),
            RuleOperator::LessThan => "LESS_THAN".to_string(),
            RuleOperator::GreaterThanOrEqual => "GREATER_THAN_OR_EQUAL".to_string(),
            RuleOperator::LessThanOrEqual => "LESS_THAN_OR_EQUAL".to_string(),
            RuleOperator::Contains => "CONTAINS".to_string(),
            RuleOperator::StartsWith => "STARTS_WITH".to_string(),
            RuleOperator::EndsWith => "ENDS_WITH".to_string(),
            RuleOperator::Regex => "REGEX".to_string(),
            RuleOperator::In => "IN".to_string(),
            RuleOperator::NotIn => "NOT_IN".to_string(),
            RuleOperator::SemverGreaterThan => "SEMVER_GREATER_THAN".to_string(),
            RuleOperator::SemverLessThan => "SEMVER_LESS_THAN".to_string(),
        }
    }

    fn from_db_str(value: &str) -> Self {
        match value.to_uppercase().as_str() {
            "EQUALS" => RuleOperator::Equals,
            "NOTEQUALS" | "NOT_EQUALS" => RuleOperator::NotEquals,
            "GREATERTHAN" | "GREATER_THAN" => RuleOperator::GreaterThan,
            "LESSTHAN" | "LESS_THAN" => RuleOperator::LessThan,
            "GREATERTHANOREQUAL" | "GREATER_THAN_OR_EQUAL" => RuleOperator::GreaterThanOrEqual,
            "LESSTHANOREQUAL" | "LESS_THAN_OR_EQUAL" => RuleOperator::LessThanOrEqual,
            "CONTAINS" => RuleOperator::Contains,
            "STARTSWITH" | "STARTS_WITH" => RuleOperator::StartsWith,
            "ENDSWITH" | "ENDS_WITH" => RuleOperator::EndsWith,
            "REGEX" => RuleOperator::Regex,
            "IN" => RuleOperator::In,
            "NOTIN" | "NOT_IN" => RuleOperator::NotIn,
            "SEMVERGREATERTHAN" | "SEMVER_GREATER_THAN" => RuleOperator::SemverGreaterThan,
            "SEMVERLESSTHAN" | "SEMVER_LESS_THAN" => RuleOperator::SemverLessThan,
            _ => RuleOperator::In,
        }
    }
}

impl From<GqlRuleOperator> for RuleOperator {
    fn from(value: GqlRuleOperator) -> Self {
        match value {
            GqlRuleOperator::Equals => RuleOperator::Equals,
            GqlRuleOperator::NotEquals => RuleOperator::NotEquals,
            GqlRuleOperator::GreaterThan => RuleOperator::GreaterThan,
            GqlRuleOperator::LessThan => RuleOperator::LessThan,
            GqlRuleOperator::GreaterThanOrEqual => RuleOperator::GreaterThanOrEqual,
            GqlRuleOperator::LessThanOrEqual => RuleOperator::LessThanOrEqual,
            GqlRuleOperator::Contains => RuleOperator::Contains,
            GqlRuleOperator::StartsWith => RuleOperator::StartsWith,
            GqlRuleOperator::EndsWith => RuleOperator::EndsWith,
            GqlRuleOperator::Regex => RuleOperator::Regex,
            GqlRuleOperator::In => RuleOperator::In,
            GqlRuleOperator::NotIn => RuleOperator::NotIn,
            GqlRuleOperator::SemverGreaterThan => RuleOperator::SemverGreaterThan,
            GqlRuleOperator::SemverLessThan => RuleOperator::SemverLessThan,
        }
    }
}

impl From<RuleOperator> for GqlRuleOperator {
    fn from(value: RuleOperator) -> Self {
        match value {
            RuleOperator::Equals => GqlRuleOperator::Equals,
            RuleOperator::NotEquals => GqlRuleOperator::NotEquals,
            RuleOperator::GreaterThan => GqlRuleOperator::GreaterThan,
            RuleOperator::LessThan => GqlRuleOperator::LessThan,
            RuleOperator::GreaterThanOrEqual => GqlRuleOperator::GreaterThanOrEqual,
            RuleOperator::LessThanOrEqual => GqlRuleOperator::LessThanOrEqual,
            RuleOperator::Contains => GqlRuleOperator::Contains,
            RuleOperator::StartsWith => GqlRuleOperator::StartsWith,
            RuleOperator::EndsWith => GqlRuleOperator::EndsWith,
            RuleOperator::Regex => GqlRuleOperator::Regex,
            RuleOperator::In => GqlRuleOperator::In,
            RuleOperator::NotIn => GqlRuleOperator::NotIn,
            RuleOperator::SemverGreaterThan => GqlRuleOperator::SemverGreaterThan,
            RuleOperator::SemverLessThan => GqlRuleOperator::SemverLessThan,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogicOperator {
    And,
    Or,
}

impl From<GqlLogicOperator> for LogicOperator {
    fn from(value: GqlLogicOperator) -> Self {
        match value {
            GqlLogicOperator::And => LogicOperator::And,
            GqlLogicOperator::Or => LogicOperator::Or,
        }
    }
}

impl From<LogicOperator> for GqlLogicOperator {
    fn from(value: LogicOperator) -> Self {
        match value {
            LogicOperator::And => GqlLogicOperator::And,
            LogicOperator::Or => GqlLogicOperator::Or,
        }
    }
}

impl From<LogicOperator> for DbLogicOperator {
    fn from(value: LogicOperator) -> Self {
        match value {
            LogicOperator::And => DbLogicOperator::And,
            LogicOperator::Or => DbLogicOperator::Or,
        }
    }
}

impl From<DbLogicOperator> for LogicOperator {
    fn from(value: DbLogicOperator) -> Self {
        match value {
            DbLogicOperator::And => LogicOperator::And,
            DbLogicOperator::Or => LogicOperator::Or,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VariantSelectionMode {
    WeightedSplit,
    SpecificVariant,
}

impl From<GqlVariantSelectionMode> for VariantSelectionMode {
    fn from(value: GqlVariantSelectionMode) -> Self {
        match value {
            GqlVariantSelectionMode::WeightedSplit => VariantSelectionMode::WeightedSplit,
            GqlVariantSelectionMode::SpecificVariant => VariantSelectionMode::SpecificVariant,
        }
    }
}

impl From<VariantSelectionMode> for GqlVariantSelectionMode {
    fn from(value: VariantSelectionMode) -> Self {
        match value {
            VariantSelectionMode::WeightedSplit => GqlVariantSelectionMode::WeightedSplit,
            VariantSelectionMode::SpecificVariant => GqlVariantSelectionMode::SpecificVariant,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VariantAllocationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub criteria_id: String,
    pub variant_control: String,
    pub weight: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompoundRuleConditionResponse {
    pub id: String,
    pub context_key: String,
    pub operator: RuleOperator,
    pub value: serde_json::Value,
    pub order_index: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompoundRuleGroupResponse {
    pub id: String,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CompoundRuleConditionResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StageCriterionResponse {
    pub id: String,
    pub stage_id: String,
    pub priority: i32,
    pub rule_groups: Vec<CompoundRuleGroupResponse>,
    pub variant_allocations: Vec<VariantAllocationResponse>,
    pub variant_selection_mode: VariantSelectionMode,
    pub selected_variant_control: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateVariantAllocationRequest {
    pub variant_control: String,
    pub weight: i32,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRuleConditionRequest {
    pub context_key: String,
    pub operator: RuleOperator,
    pub value: serde_json::Value,
    #[serde(default)]
    pub order_index: i32,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InlineRuleGroupRequest {
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionRequest>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateStageCriterionRequest {
    #[serde(default)]
    pub priority: i32,
    pub variant_allocations: Option<Vec<CreateVariantAllocationRequest>>,
    pub rule_groups: Option<Vec<InlineRuleGroupRequest>>,
    pub variant_selection_mode: Option<VariantSelectionMode>,
    pub selected_variant_control: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetVariantAllocationsRequest {
    pub allocations: Vec<CreateVariantAllocationRequest>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRuleGroupRequest {
    pub criteria_id: String,
    pub logic_operator: LogicOperator,
    pub conditions: Vec<CreateRuleConditionRequest>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRuleGroupRequest {
    pub logic_operator: Option<LogicOperator>,
    pub conditions: Option<Vec<CreateRuleConditionRequest>>,
}

impl From<GqlCompoundRuleCondition> for CompoundRuleConditionResponse {
    fn from(value: GqlCompoundRuleCondition) -> Self {
        Self {
            id: value.id.to_string(),
            context_key: value.context_key,
            operator: value.operator.into(),
            value: value.value,
            order_index: value.order_index,
        }
    }
}

impl From<GqlCompoundRuleGroup> for CompoundRuleGroupResponse {
    fn from(value: GqlCompoundRuleGroup) -> Self {
        Self {
            id: value.id.to_string(),
            logic_operator: value.logic_operator.into(),
            conditions: value
                .conditions
                .into_iter()
                .map(CompoundRuleConditionResponse::from)
                .collect(),
        }
    }
}

impl From<GqlVariantAllocation> for VariantAllocationResponse {
    fn from(value: GqlVariantAllocation) -> Self {
        Self {
            id: Some(value.id.to_string()),
            criteria_id: value.criteria_id.to_string(),
            variant_control: value.variant_control,
            weight: value.weight,
        }
    }
}

impl From<GqlStageCriterion> for StageCriterionResponse {
    fn from(value: GqlStageCriterion) -> Self {
        Self {
            id: value.id.to_string(),
            stage_id: value.stage_id.to_string(),
            priority: value.priority,
            rule_groups: value
                .rule_groups
                .into_iter()
                .map(CompoundRuleGroupResponse::from)
                .collect(),
            variant_allocations: value
                .variant_allocations
                .into_iter()
                .map(VariantAllocationResponse::from)
                .collect(),
            variant_selection_mode: value.variant_selection_mode.into(),
            selected_variant_control: value.selected_variant_control,
        }
    }
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn validate_context_key(value: &str) -> Result<(), RestError> {
    let len = value.trim().len();
    if len < 1 || len > 100 {
        return Err(RestError::invalid_input(
            "contextKey must be between 1 and 100 characters",
        ));
    }
    Ok(())
}

fn validate_variant_control(value: &str) -> Result<(), RestError> {
    let len = value.trim().len();
    if len < 1 || len > 100 {
        return Err(RestError::invalid_input(
            "variantControl must be between 1 and 100 characters",
        ));
    }
    Ok(())
}

fn validate_weight(weight: i32) -> Result<(), RestError> {
    if weight < 0 || weight > 100 {
        return Err(RestError::invalid_input(
            "weight must be between 0 and 100",
        ));
    }
    Ok(())
}

fn validate_rule_conditions(conditions: &[CreateRuleConditionRequest]) -> Result<(), RestError> {
    for condition in conditions {
        validate_context_key(&condition.context_key)?;
    }
    Ok(())
}

fn validate_rule_groups(groups: &[InlineRuleGroupRequest]) -> Result<(), RestError> {
    for group in groups {
        validate_rule_conditions(&group.conditions)?;
    }
    Ok(())
}

fn validate_allocations(
    allocations: &[CreateVariantAllocationRequest],
) -> Result<(), RestError> {
    for alloc in allocations {
        validate_variant_control(&alloc.variant_control)?;
        validate_weight(alloc.weight)?;
    }
    Ok(())
}

fn validate_stage_criteria(
    criteria: &[CreateStageCriterionRequest],
) -> Result<(), RestError> {
    for criterion in criteria {
        if let Some(allocs) = &criterion.variant_allocations {
            validate_allocations(allocs)?;
        }
        if let Some(groups) = &criterion.rule_groups {
            validate_rule_groups(groups)?;
        }
    }
    Ok(())
}

fn map_rule_group_response(
    group_id: Uuid,
    logic_operator: DbLogicOperator,
    conditions: Vec<crate::database::entity::RuleCondition>,
) -> CompoundRuleGroupResponse {
    let mapped_conditions = conditions
        .into_iter()
        .map(|cond| CompoundRuleConditionResponse {
            id: cond.id.to_string(),
            context_key: cond.context_key,
            operator: RuleOperator::from_db_str(&cond.operator),
            value: cond.value,
            order_index: cond.order_index,
        })
        .collect();

    CompoundRuleGroupResponse {
        id: group_id.to_string(),
        logic_operator: logic_operator.into(),
        conditions: mapped_conditions,
    }
}

fn map_create_stage_criterion(
    criterion: &CreateStageCriterionRequest,
) -> GqlCreateStageCriterionInput {
    GqlCreateStageCriterionInput {
        priority: criterion.priority,
        variant_allocations: criterion.variant_allocations.as_ref().map(|allocs| {
            allocs
                .iter()
                .map(|alloc| GqlCreateVariantAllocationInput {
                    variant_control: alloc.variant_control.clone(),
                    weight: alloc.weight,
                })
                .collect()
        }),
        rule_groups: criterion.rule_groups.as_ref().map(|groups| {
            groups
                .iter()
                .map(|group| GqlInlineRuleGroupInput {
                    logic_operator: group.logic_operator.into(),
                    conditions: group
                        .conditions
                        .iter()
                        .map(|cond| GqlCreateRuleConditionInput {
                            context_key: cond.context_key.clone(),
                            operator: cond.operator.into(),
                            value: cond.value.clone(),
                            order_index: cond.order_index,
                        })
                        .collect(),
                })
                .collect()
        }),
        variant_selection_mode: criterion
            .variant_selection_mode
            .map(|mode| mode.into()),
        selected_variant_control: criterion.selected_variant_control.clone(),
    }
}

async fn broadcast_feature_update(
    feature_repo: &dyn FeatureRepository,
    updates_tx: &tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>,
    feature_id: Uuid,
) {
    if let Ok(db_feature) = feature_repo.get_feature_by_id(feature_id).await {
        if let Ok(full) =
            crate::broadcast::map_db_feature_to_full_for_broadcast(feature_repo, db_feature).await
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
}

#[utoipa::path(
    get,
    path = "/api/v1/stages/{stage_id}/criteria",
    params(
        ("stage_id" = String, Path, description = "Stage ID")
    ),
    responses(
        (status = 200, description = "Stage criteria list", body = [StageCriterionResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Criteria"
)]
#[get("/stages/{stage_id}/criteria")]
pub(crate) async fn get_stage_criteria(
    logic: web::Data<Box<dyn FeatureLogic>>,
    stage_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let stage_uuid = parse_uuid(&stage_id, "stage id")?;
    let criteria = logic
        .get_stage_criteria(ID::from(stage_uuid))
        .await
        .map_err(RestError::from)?;

    let response: Vec<StageCriterionResponse> = criteria
        .into_iter()
        .map(StageCriterionResponse::from)
        .collect();
    Ok(HttpResponse::Ok().json(response))
}

#[utoipa::path(
    put,
    path = "/api/v1/stages/{stage_id}/criteria",
    request_body = [CreateStageCriterionRequest],
    params(
        ("stage_id" = String, Path, description = "Stage ID")
    ),
    responses(
        (status = 200, description = "Updated stage criteria", body = [StageCriterionResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Criteria"
)]
#[put("/stages/{stage_id}/criteria")]
pub(crate) async fn set_stage_criteria(
    pool: web::Data<sqlx::PgPool>,
    feature_repo: web::Data<Box<dyn FeatureRepository>>,
    updates_tx: web::Data<tokio::sync::broadcast::Sender<crate::grpc::pb::FeatureUpdate>>,
    stage_id: web::Path<String>,
    body: web::Json<Vec<CreateStageCriterionRequest>>,
) -> Result<impl Responder, RestError> {
    let stage_uuid = parse_uuid(&stage_id, "stage id")?;
    let criteria = body.into_inner();

    validate_stage_criteria(&criteria)?;

    let gql_criteria: Vec<GqlCreateStageCriterionInput> = criteria
        .iter()
        .map(map_create_stage_criterion)
        .collect();

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| RestError::internal(e.to_string()))?;

    let feature_repo_tx = crate::database::feature::feature_repository_tx(pool.get_ref().clone());
    let variant_repo_tx =
        crate::database::variant_allocations::variant_allocations_repository_tx(
            pool.get_ref().clone(),
        );
    let rules_repo_tx = crate::database::compound_rules::compound_rules_repository_tx(
        pool.get_ref().clone(),
    );

    let result = feature_tx::set_stage_criteria_in_tx(
        &mut tx,
        &feature_repo_tx,
        &variant_repo_tx,
        &rules_repo_tx,
        ID::from(stage_uuid),
        gql_criteria,
    )
    .await;

    match result {
        Ok(updated) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(e.to_string()))?;

            if let Ok(Some(feature_id)) = feature_repo.get_feature_id_by_stage_id(stage_uuid).await
            {
                broadcast_feature_update(feature_repo.as_ref().as_ref(), &updates_tx, feature_id)
                    .await;
            }

            let response: Vec<StageCriterionResponse> = updated
                .into_iter()
                .map(StageCriterionResponse::from)
                .collect();
            Ok(HttpResponse::Ok().json(response))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/criteria/{criteria_id}/variant-allocations",
    request_body = SetVariantAllocationsRequest,
    params(
        ("criteria_id" = String, Path, description = "Stage criterion ID")
    ),
    responses(
        (status = 200, description = "Updated variant allocations", body = [VariantAllocationResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Criteria"
)]
#[put("/criteria/{criteria_id}/variant-allocations")]
pub(crate) async fn set_variant_allocations(
    db_pool: web::Data<sqlx::PgPool>,
    criteria_id: web::Path<String>,
    body: web::Json<SetVariantAllocationsRequest>,
) -> Result<impl Responder, RestError> {
    let criteria_uuid = parse_uuid(&criteria_id, "criteria id")?;
    let allocations = body.into_inner().allocations;

    validate_allocations(&allocations)?;

    let total_weight: i32 = allocations.iter().map(|a| a.weight).sum();
    if total_weight > 100 {
        return Err(RestError::invalid_input(format!(
            "Total weight exceeds 100: got {}",
            total_weight
        )));
    }

    let db_allocations: Vec<DbCreateVariantAllocationInput> = allocations
        .into_iter()
        .map(|alloc| DbCreateVariantAllocationInput {
            criteria_id: criteria_uuid,
            variant_control: alloc.variant_control,
            weight: alloc.weight,
        })
        .collect();

    let repo_tx = variant_allocations_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(e.to_string()))?;

    let result = repo_tx
        .set_allocations_tx(&mut tx, criteria_uuid, db_allocations)
        .await;

    match result {
        Ok(saved) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(e.to_string()))?;
            let response: Vec<VariantAllocationResponse> = saved
                .into_iter()
                .map(|alloc| VariantAllocationResponse {
                    id: Some(alloc.id.to_string()),
                    criteria_id: alloc.criteria_id.to_string(),
                    variant_control: alloc.variant_control,
                    weight: alloc.weight,
                })
                .collect();
            Ok(HttpResponse::Ok().json(response))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/rule-groups",
    request_body = CreateRuleGroupRequest,
    responses(
        (status = 200, description = "Rule group created", body = CompoundRuleGroupResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Criteria"
)]
#[post("/rule-groups")]
pub(crate) async fn create_rule_group(
    db_pool: web::Data<sqlx::PgPool>,
    body: web::Json<CreateRuleGroupRequest>,
) -> Result<impl Responder, RestError> {
    let payload = body.into_inner();
    let criteria_uuid = parse_uuid(&payload.criteria_id, "criteria id")?;

    validate_rule_conditions(&payload.conditions)?;

    let db_input = DbCreateRuleGroupInput {
        criteria_id: criteria_uuid,
        logic_operator: payload.logic_operator.into(),
        conditions: payload
            .conditions
            .into_iter()
            .map(|cond| DbCreateRuleConditionInput {
                context_key: cond.context_key,
                operator: cond.operator.to_db_string(),
                value: cond.value,
                order_index: cond.order_index,
            })
            .collect(),
    };

    let repo_tx = compound_rules_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(e.to_string()))?;

    let result = repo_tx.create_rule_group_tx(&mut tx, db_input).await;

    match result {
        Ok(group) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(e.to_string()))?;
            let conditions = repo_tx
                .get_rule_conditions(group.id)
                .await
                .map_err(RestError::from)?;
            let response = map_rule_group_response(group.id, group.logic_operator, conditions);
            Ok(HttpResponse::Ok().json(response))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/rule-groups/{group_id}",
    request_body = UpdateRuleGroupRequest,
    params(
        ("group_id" = String, Path, description = "Rule group ID")
    ),
    responses(
        (status = 200, description = "Rule group updated", body = CompoundRuleGroupResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Criteria"
)]
#[patch("/rule-groups/{group_id}")]
pub(crate) async fn update_rule_group(
    db_pool: web::Data<sqlx::PgPool>,
    group_id: web::Path<String>,
    body: web::Json<UpdateRuleGroupRequest>,
) -> Result<impl Responder, RestError> {
    let group_uuid = parse_uuid(&group_id, "group id")?;
    let payload = body.into_inner();

    if let Some(conditions) = &payload.conditions {
        validate_rule_conditions(conditions)?;
    }

    let db_input = DbUpdateRuleGroupInput {
        logic_operator: payload.logic_operator.map(|op| op.into()),
        conditions: payload.conditions.map(|conds| {
            conds
                .into_iter()
                .map(|cond| DbCreateRuleConditionInput {
                    context_key: cond.context_key,
                    operator: cond.operator.to_db_string(),
                    value: cond.value,
                    order_index: cond.order_index,
                })
                .collect()
        }),
    };

    let repo_tx = compound_rules_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(e.to_string()))?;

    let result = repo_tx.update_rule_group_tx(&mut tx, group_uuid, db_input).await;

    match result {
        Ok(group) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(e.to_string()))?;
            let conditions = repo_tx
                .get_rule_conditions(group.id)
                .await
                .map_err(RestError::from)?;
            let response = map_rule_group_response(group.id, group.logic_operator, conditions);
            Ok(HttpResponse::Ok().json(response))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/rule-groups/{group_id}",
    params(
        ("group_id" = String, Path, description = "Rule group ID")
    ),
    responses(
        (status = 204, description = "Rule group deleted"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Criteria"
)]
#[delete("/rule-groups/{group_id}")]
pub(crate) async fn delete_rule_group(
    db_pool: web::Data<sqlx::PgPool>,
    group_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let group_uuid = parse_uuid(&group_id, "group id")?;
    let repo_tx = compound_rules_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(e.to_string()))?;

    let result = repo_tx.delete_rule_group_tx(&mut tx, group_uuid).await;

    match result {
        Ok(_) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(e.to_string()))?;
            Ok(HttpResponse::NoContent().finish())
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_stage_criteria)
        .service(set_stage_criteria)
        .service(set_variant_allocations)
        .service(create_rule_group)
        .service(update_rule_group)
        .service(delete_rule_group);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};

    use crate::logic::feature::MockFeatureLogic;
    use sqlx::postgres::PgPoolOptions;

    fn sample_stage_criterion(stage_id: Uuid) -> GqlStageCriterion {
        GqlStageCriterion {
            id: ID::from(Uuid::new_v4()),
            stage_id: ID::from(stage_id),
            priority: 0,
            rule_groups: vec![GqlCompoundRuleGroup {
                id: ID::from(Uuid::new_v4()),
                logic_operator: GqlLogicOperator::And,
                conditions: vec![GqlCompoundRuleCondition {
                    id: ID::from(Uuid::new_v4()),
                    context_key: "country".to_string(),
                    operator: GqlRuleOperator::Equals,
                    value: serde_json::Value::String("US".to_string()),
                    order_index: 0,
                }],
            }],
            variant_allocations: vec![GqlVariantAllocation {
                id: ID::from(Uuid::new_v4()),
                criteria_id: ID::from(Uuid::new_v4()),
                variant_control: "control".to_string(),
                weight: 100,
            }],
            variant_selection_mode: GqlVariantSelectionMode::WeightedSplit,
            selected_variant_control: None,
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

    #[actix_web::test]
    async fn get_stage_criteria_returns_items() {
        let stage_id = Uuid::new_v4();
        let criterion = sample_stage_criterion(stage_id);

        let mut mock_logic = MockFeatureLogic::new();
        mock_logic
            .expect_get_stage_criteria()
            .withf(move |id| id.to_string() == stage_id.to_string())
            .times(1)
            .returning(move |_| Ok(vec![criterion.clone()]));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn FeatureLogic>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri = format!("/api/v1/stages/{stage_id}/criteria");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json[0]["stageId"], stage_id.to_string());
    }

    #[actix_web::test]
    async fn set_variant_allocations_validates_total_weight() {
        let criteria_id = Uuid::new_v4();
        let pool = test_pool().await;

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let body = SetVariantAllocationsRequest {
            allocations: vec![
                CreateVariantAllocationRequest {
                    variant_control: "control".to_string(),
                    weight: 70,
                },
                CreateVariantAllocationRequest {
                    variant_control: "variant".to_string(),
                    weight: 50,
                },
            ],
        };
        let req = test::TestRequest::put()
            .uri(&format!(
                "/api/v1/criteria/{criteria_id}/variant-allocations"
            ))
            .set_json(body)
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn create_rule_group_returns_group() {
        let criteria_id =
            Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap();
        let pool = test_pool().await;

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let body = CreateRuleGroupRequest {
            criteria_id: criteria_id.to_string(),
            logic_operator: LogicOperator::And,
            conditions: vec![CreateRuleConditionRequest {
                context_key: "country".to_string(),
                operator: RuleOperator::Equals,
                value: serde_json::Value::String("US".to_string()),
                order_index: 0,
            }],
        };

        let req = test::TestRequest::post()
            .uri("/api/v1/rule-groups")
            .set_json(body)
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["id"].as_str().unwrap_or_default().len() > 0);
    }
}
