use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, patch, post, web};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::approval::{
    ApprovalRepository, CreateApprovalPolicyInput, UpdateApprovalPolicyInput,
    approval_repository_tx,
};
use crate::database::entity::{ApprovalPolicy, ApprovalRequest, ApprovalStatus, ApprovalVote};
use crate::logic::ActorContext;
use crate::logic::approval::ApprovalLogic;
use crate::rest::error::RestError;
use crate::rest::pagination::{PageMeta, PaginationQuery, normalize_pagination};

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequestStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    AutoApproved,
}

impl From<ApprovalStatus> for ApprovalRequestStatus {
    fn from(status: ApprovalStatus) -> Self {
        match status {
            ApprovalStatus::Pending => ApprovalRequestStatus::Pending,
            ApprovalStatus::Approved => ApprovalRequestStatus::Approved,
            ApprovalStatus::Rejected => ApprovalRequestStatus::Rejected,
            ApprovalStatus::Cancelled => ApprovalRequestStatus::Cancelled,
            ApprovalStatus::AutoApproved => ApprovalRequestStatus::AutoApproved,
        }
    }
}

impl From<ApprovalRequestStatus> for ApprovalStatus {
    fn from(status: ApprovalRequestStatus) -> Self {
        match status {
            ApprovalRequestStatus::Pending => ApprovalStatus::Pending,
            ApprovalRequestStatus::Approved => ApprovalStatus::Approved,
            ApprovalRequestStatus::Rejected => ApprovalStatus::Rejected,
            ApprovalRequestStatus::Cancelled => ApprovalStatus::Cancelled,
            ApprovalRequestStatus::AutoApproved => ApprovalStatus::AutoApproved,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppliesTo {
    All,
    ProductionOnly,
    SpecificEnvironments,
}

impl AppliesTo {
    fn as_str(&self) -> &'static str {
        match self {
            AppliesTo::All => "all",
            AppliesTo::ProductionOnly => "production_only",
            AppliesTo::SpecificEnvironments => "specific_environments",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApprovalRequestListQuery {
    pub statuses: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalVoteResponse {
    pub id: String,
    pub approver_id: String,
    pub vote: String,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestResponse {
    pub id: String,
    pub policy_id: String,
    pub feature_id: String,
    pub environment_id: Option<String>,
    pub change_type: String,
    pub change_payload: serde_json::Value,
    pub change_description: Option<String>,
    pub requested_by: String,
    pub status: ApprovalRequestStatus,
    pub approved_count: i32,
    pub rejected_count: i32,
    pub executed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub votes: Vec<ApprovalVoteResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApprovalRequestsResponse {
    pub items: Vec<ApprovalRequestResponse>,
    pub meta: PageMeta,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalActionRequest {
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicyResponse {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub description: Option<String>,
    pub applies_to: AppliesTo,
    pub environment_ids: Option<Vec<String>>,
    pub required_approvers: i32,
    pub approver_role_ids: Vec<String>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApprovalPolicyRequest {
    pub name: String,
    pub description: Option<String>,
    pub applies_to: AppliesTo,
    pub environment_ids: Option<Vec<String>>,
    pub required_approvers: i32,
    pub approver_role_ids: Vec<String>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateApprovalPolicyRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub applies_to: Option<AppliesTo>,
    pub environment_ids: Option<Vec<String>>,
    pub required_approvers: Option<i32>,
    pub approver_role_ids: Option<Vec<String>>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value).map_err(|_| RestError::invalid_input(format!("invalid {field}")))
}

fn actor_from_request(req: &HttpRequest) -> Option<ActorContext> {
    req.extensions()
        .get::<JwtUser>()
        .map(|jwt| ActorContext::new(jwt.id, jwt.username.clone()))
}

fn jwt_user(req: &HttpRequest) -> Result<JwtUser, RestError> {
    req.extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))
}

fn validate_policy_name(name: &str) -> Result<(), RestError> {
    if name.trim().is_empty() {
        return Err(RestError::invalid_input("Policy name is required"));
    }
    Ok(())
}

fn validate_required_approvers(required: i32) -> Result<(), RestError> {
    if required < 1 {
        return Err(RestError::invalid_input(
            "Required approvers must be at least 1",
        ));
    }
    Ok(())
}

fn validate_approver_roles(roles: &[String]) -> Result<(), RestError> {
    if roles.is_empty() {
        return Err(RestError::invalid_input(
            "At least one approver role is required",
        ));
    }
    Ok(())
}

fn validate_auto_approve(value: Option<i32>) -> Result<(), RestError> {
    if let Some(hours) = value
        && hours < 1
    {
        return Err(RestError::invalid_input(
            "Auto-approve hours must be at least 1",
        ));
    }
    Ok(())
}

fn parse_uuid_list(values: &[String], field: &str) -> Result<Vec<Uuid>, RestError> {
    values
        .iter()
        .map(|value| parse_uuid(value, field))
        .collect()
}

fn parse_statuses(raw: Option<&str>) -> Result<Option<Vec<ApprovalStatus>>, RestError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let mut statuses = Vec::new();
    for item in value.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let status = match trimmed.to_lowercase().as_str() {
            "pending" => ApprovalStatus::Pending,
            "approved" => ApprovalStatus::Approved,
            "rejected" => ApprovalStatus::Rejected,
            "cancelled" => ApprovalStatus::Cancelled,
            "auto_approved" | "autoapproved" | "auto-approved" => ApprovalStatus::AutoApproved,
            _ => {
                return Err(RestError::invalid_input(format!(
                    "invalid approval status: {trimmed}"
                )));
            }
        };
        statuses.push(status);
    }

    if statuses.is_empty() {
        Ok(None)
    } else {
        Ok(Some(statuses))
    }
}

fn normalize_environment_ids(
    applies_to: AppliesTo,
    environment_ids: Option<Vec<String>>,
) -> Result<Option<Vec<Uuid>>, RestError> {
    if applies_to == AppliesTo::SpecificEnvironments {
        let ids = environment_ids.unwrap_or_default();
        if ids.is_empty() {
            return Err(RestError::invalid_input(
                "At least one environment must be selected",
            ));
        }
        return Ok(Some(parse_uuid_list(&ids, "environment_id")?));
    }
    Ok(None)
}

fn map_vote(vote: ApprovalVote) -> ApprovalVoteResponse {
    ApprovalVoteResponse {
        id: vote.id.to_string(),
        approver_id: vote.approver_id.to_string(),
        vote: vote.vote.as_str().to_string(),
        comment: vote.comment,
        created_at: vote.created_at,
    }
}

fn map_request(request: ApprovalRequest, votes: Vec<ApprovalVote>) -> ApprovalRequestResponse {
    ApprovalRequestResponse {
        id: request.id.to_string(),
        policy_id: request.policy_id.to_string(),
        feature_id: request.feature_id.to_string(),
        environment_id: request.environment_id.map(|id| id.to_string()),
        change_type: request.change_type,
        change_payload: request.change_payload,
        change_description: request.change_description,
        requested_by: request.requested_by.to_string(),
        status: ApprovalRequestStatus::from(request.status),
        approved_count: request.approved_count,
        rejected_count: request.rejected_count,
        executed_at: request.executed_at,
        created_at: request.created_at,
        updated_at: request.updated_at,
        votes: votes.into_iter().map(map_vote).collect(),
    }
}

fn map_policy(policy: ApprovalPolicy) -> ApprovalPolicyResponse {
    let applies_to = match policy.applies_to.as_str() {
        "production_only" => AppliesTo::ProductionOnly,
        "specific_environments" => AppliesTo::SpecificEnvironments,
        _ => AppliesTo::All,
    };

    ApprovalPolicyResponse {
        id: policy.id.to_string(),
        team_id: policy.team_id.to_string(),
        name: policy.name,
        description: policy.description,
        applies_to,
        environment_ids: policy
            .environment_ids
            .map(|ids| ids.into_iter().map(|id| id.to_string()).collect()),
        required_approvers: policy.required_approvers,
        approver_role_ids: policy
            .approver_role_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
        auto_approve_after_hours: policy.auto_approve_after_hours,
        enabled: policy.enabled,
        created_at: policy.created_at,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/approval-requests",
    params(
        ("team_id" = String, Path, description = "Team ID"),
        ("statuses" = Option<String>, Query, description = "Comma-separated status filter"),
        ("offset" = Option<i64>, Query, description = "Pagination offset"),
        ("limit" = Option<i64>, Query, description = "Pagination limit")
    ),
    responses(
        (status = 200, description = "Approval requests", body = ApprovalRequestsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[get("/teams/{team_id}/approval-requests")]
pub(crate) async fn list_approval_requests(
    repo: web::Data<Box<dyn ApprovalRepository>>,
    team_id: web::Path<String>,
    query: web::Query<ApprovalRequestListQuery>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let (offset, limit) = normalize_pagination(&PaginationQuery {
        offset: query.offset,
        limit: query.limit,
    });

    let statuses = parse_statuses(query.statuses.as_deref())?;

    let (requests, total) = repo
        .list_requests_for_team_with_offset(Some(team_uuid), statuses, offset, limit)
        .await
        .map_err(RestError::from)?;

    let mut items = Vec::with_capacity(requests.len());
    for request in requests {
        let votes = repo
            .list_votes_for_request(request.id)
            .await
            .map_err(RestError::from)?;
        items.push(map_request(request, votes));
    }

    Ok(HttpResponse::Ok().json(ApprovalRequestsResponse {
        items,
        meta: PageMeta {
            offset,
            limit,
            total,
        },
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/approval-requests/{id}/approve",
    request_body = ApprovalActionRequest,
    params(
        ("id" = String, Path, description = "Approval request ID")
    ),
    responses(
        (status = 200, description = "Approval request approved", body = ApprovalRequestResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[post("/approval-requests/{id}/approve")]
pub(crate) async fn approve_request(
    logic: web::Data<Box<dyn ApprovalLogic>>,
    repo: web::Data<Box<dyn ApprovalRepository>>,
    req: HttpRequest,
    request_id: web::Path<String>,
    payload: web::Json<ApprovalActionRequest>,
) -> Result<impl Responder, RestError> {
    let user = jwt_user(&req)?;
    let request_uuid = parse_uuid(&request_id, "request_id")?;

    let updated = logic
        .approve_request(request_uuid, user.id, payload.comment.clone())
        .await
        .map_err(RestError::from)?;

    let votes = repo
        .list_votes_for_request(request_uuid)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(map_request(updated, votes)))
}

#[utoipa::path(
    post,
    path = "/api/v1/approval-requests/{id}/reject",
    request_body = ApprovalActionRequest,
    params(
        ("id" = String, Path, description = "Approval request ID")
    ),
    responses(
        (status = 200, description = "Approval request rejected", body = ApprovalRequestResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[post("/approval-requests/{id}/reject")]
pub(crate) async fn reject_request(
    logic: web::Data<Box<dyn ApprovalLogic>>,
    repo: web::Data<Box<dyn ApprovalRepository>>,
    req: HttpRequest,
    request_id: web::Path<String>,
    payload: web::Json<ApprovalActionRequest>,
) -> Result<impl Responder, RestError> {
    let user = jwt_user(&req)?;
    let request_uuid = parse_uuid(&request_id, "request_id")?;

    let updated = logic
        .reject_request(request_uuid, user.id, payload.comment.clone())
        .await
        .map_err(RestError::from)?;

    let votes = repo
        .list_votes_for_request(request_uuid)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(map_request(updated, votes)))
}

#[utoipa::path(
    post,
    path = "/api/v1/approval-requests/{id}/cancel",
    params(
        ("id" = String, Path, description = "Approval request ID")
    ),
    responses(
        (status = 200, description = "Approval request cancelled", body = ApprovalRequestResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[post("/approval-requests/{id}/cancel")]
pub(crate) async fn cancel_request(
    logic: web::Data<Box<dyn ApprovalLogic>>,
    repo: web::Data<Box<dyn ApprovalRepository>>,
    req: HttpRequest,
    request_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let user = jwt_user(&req)?;
    let request_uuid = parse_uuid(&request_id, "request_id")?;

    let updated = logic
        .cancel_request(request_uuid, user.id)
        .await
        .map_err(RestError::from)?;

    let votes = repo
        .list_votes_for_request(request_uuid)
        .await
        .map_err(RestError::from)?;

    Ok(HttpResponse::Ok().json(map_request(updated, votes)))
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/approval-policies",
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Approval policies", body = [ApprovalPolicyResponse]),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[get("/teams/{team_id}/approval-policies")]
pub(crate) async fn list_approval_policies(
    repo: web::Data<Box<dyn ApprovalRepository>>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let policies = repo
        .list_policies_for_team(team_uuid)
        .await
        .map_err(RestError::from)?;
    let items = policies.into_iter().map(map_policy).collect::<Vec<_>>();
    Ok(HttpResponse::Ok().json(items))
}

#[utoipa::path(
    get,
    path = "/api/v1/approval-policies/{id}",
    params(
        ("id" = String, Path, description = "Approval policy ID")
    ),
    responses(
        (status = 200, description = "Approval policy", body = ApprovalPolicyResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[get("/approval-policies/{id}")]
pub(crate) async fn get_approval_policy(
    repo: web::Data<Box<dyn ApprovalRepository>>,
    policy_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let policy_uuid = parse_uuid(&policy_id, "policy_id")?;
    let policy = repo
        .get_policy_by_id(policy_uuid)
        .await
        .map_err(RestError::from)?
        .ok_or_else(|| RestError::not_found("Approval policy not found"))?;
    Ok(HttpResponse::Ok().json(map_policy(policy)))
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/approval-policies",
    request_body = CreateApprovalPolicyRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 201, description = "Approval policy created", body = ApprovalPolicyResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[post("/teams/{team_id}/approval-policies")]
pub(crate) async fn create_approval_policy(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<CreateApprovalPolicyRequest>,
) -> Result<impl Responder, RestError> {
    validate_policy_name(&payload.name)?;
    validate_required_approvers(payload.required_approvers)?;
    validate_approver_roles(&payload.approver_role_ids)?;
    validate_auto_approve(payload.auto_approve_after_hours)?;

    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let env_ids = normalize_environment_ids(payload.applies_to, payload.environment_ids.clone())?;
    let role_ids = parse_uuid_list(&payload.approver_role_ids, "role_id")?;

    let actor = actor_from_request(&req);
    let repo = approval_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::approval_tx::create_approval_policy_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        CreateApprovalPolicyInput {
            team_id: team_uuid,
            name: payload.name.clone(),
            description: payload.description.clone(),
            applies_to: payload.applies_to.as_str().to_string(),
            environment_ids: env_ids,
            required_approvers: payload.required_approvers,
            approver_role_ids: role_ids,
            auto_approve_after_hours: payload.auto_approve_after_hours,
            enabled: payload.enabled.unwrap_or(true),
        },
        actor,
    )
    .await;

    match result {
        Ok(policy) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::Created().json(map_policy(policy)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    patch,
    path = "/api/v1/approval-policies/{id}",
    request_body = UpdateApprovalPolicyRequest,
    params(
        ("id" = String, Path, description = "Approval policy ID")
    ),
    responses(
        (status = 200, description = "Approval policy updated", body = ApprovalPolicyResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[patch("/approval-policies/{id}")]
pub(crate) async fn update_approval_policy(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    policy_id: web::Path<String>,
    payload: web::Json<UpdateApprovalPolicyRequest>,
) -> Result<impl Responder, RestError> {
    if let Some(name) = payload.name.as_deref() {
        validate_policy_name(name)?;
    }
    if let Some(required) = payload.required_approvers {
        validate_required_approvers(required)?;
    }
    if let Some(ref roles) = payload.approver_role_ids {
        validate_approver_roles(roles)?;
    }
    validate_auto_approve(payload.auto_approve_after_hours)?;

    if let Some(AppliesTo::SpecificEnvironments) = payload.applies_to {
        let ids = payload.environment_ids.clone().unwrap_or_default();
        if ids.is_empty() {
            return Err(RestError::invalid_input(
                "At least one environment must be selected",
            ));
        }
    }

    let policy_uuid = parse_uuid(&policy_id, "policy_id")?;
    let env_ids = payload
        .environment_ids
        .clone()
        .map(|ids| parse_uuid_list(&ids, "environment_id"))
        .transpose()?;
    let role_ids = payload
        .approver_role_ids
        .clone()
        .map(|ids| parse_uuid_list(&ids, "role_id"))
        .transpose()?;

    let actor = actor_from_request(&req);
    let repo = approval_repository_tx(db_pool.get_ref().clone());
    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::approval_tx::update_approval_policy_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        policy_uuid,
        UpdateApprovalPolicyInput {
            name: payload.name.clone(),
            description: payload.description.clone(),
            applies_to: payload.applies_to.map(|value| value.as_str().to_string()),
            environment_ids: env_ids,
            required_approvers: payload.required_approvers,
            approver_role_ids: role_ids,
            auto_approve_after_hours: payload.auto_approve_after_hours,
            enabled: payload.enabled,
        },
        actor,
    )
    .await;

    match result {
        Ok(policy) => {
            tx.commit()
                .await
                .map_err(|e| RestError::internal(format!("Failed to commit transaction: {e}")))?;
            Ok(HttpResponse::Ok().json(map_policy(policy)))
        }
        Err(err) => {
            let _ = tx.rollback().await;
            Err(RestError::from(err))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/approval-policies/{id}",
    params(
        ("id" = String, Path, description = "Approval policy ID")
    ),
    responses(
        (status = 204, description = "Approval policy deleted"),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[delete("/approval-policies/{id}")]
pub(crate) async fn delete_approval_policy(
    db_pool: web::Data<sqlx::PgPool>,
    activity_repo: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    policy_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let policy_uuid = parse_uuid(&policy_id, "policy_id")?;
    let actor = actor_from_request(&req);
    let repo = approval_repository_tx(db_pool.get_ref().clone());

    let policy = repo
        .get_policy_by_id(policy_uuid)
        .await
        .map_err(RestError::from)?
        .ok_or_else(|| RestError::not_found("Approval policy not found"))?;
    let policy_name = policy.name.clone();

    let mut tx = db_pool
        .begin()
        .await
        .map_err(|e| RestError::internal(format!("Failed to start transaction: {e}")))?;

    let result = crate::logic::approval_tx::delete_approval_policy_in_tx(
        &mut tx,
        &repo,
        activity_repo.as_ref().as_ref(),
        policy_uuid,
        policy_name,
        actor,
    )
    .await;

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
    cfg.service(list_approval_requests)
        .service(approve_request)
        .service(reject_request)
        .service(cancel_request)
        .service(list_approval_policies)
        .service(get_approval_policy)
        .service(create_approval_policy)
        .service(update_approval_policy)
        .service(delete_approval_policy);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::activity_log::MockActivityLogRepository;
    use crate::database::approval::MockApprovalRepository;
    use crate::database::entity::{ApprovalStatus, ApprovalVoteValue};
    use crate::logic::approval::MockApprovalLogic;
    use actix_web::{App, http::StatusCode, test};
    use chrono::Utc;
    use sqlx::postgres::PgPoolOptions;

    fn sample_request(request_id: Uuid) -> ApprovalRequest {
        ApprovalRequest {
            id: request_id,
            policy_id: Uuid::new_v4(),
            feature_id: Uuid::new_v4(),
            environment_id: Some(Uuid::new_v4()),
            change_type: "stage_change".to_string(),
            change_payload: serde_json::json!({"stage_id": "stage-1"}),
            change_description: Some("Deploy".to_string()),
            requested_by: Uuid::new_v4(),
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_vote(request_id: Uuid) -> ApprovalVote {
        ApprovalVote {
            id: Uuid::new_v4(),
            request_id,
            approver_id: Uuid::new_v4(),
            vote: ApprovalVoteValue::Approve,
            comment: Some("ok".to_string()),
            created_at: Utc::now(),
        }
    }

    #[actix_web::test]
    async fn list_approval_requests_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let request = sample_request(request_id);

        let mut mock_repo = MockApprovalRepository::new();
        mock_repo
            .expect_list_requests_for_team_with_offset()
            .withf(move |team, statuses, offset, limit| {
                team.map(|id| id.to_string()) == Some(team_id.to_string())
                    && statuses
                        .as_ref()
                        .map(|list| list == &vec![ApprovalStatus::Pending])
                        .unwrap_or(false)
                    && *offset == 10
                    && *limit == 5
            })
            .times(1)
            .returning(move |_, _, _, _| Ok((vec![request.clone()], 1)));
        mock_repo
            .expect_list_votes_for_request()
            .withf(move |id| *id == request_id)
            .times(1)
            .returning(move |_| Ok(vec![sample_vote(request_id)]));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_repo) as Box<dyn ApprovalRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let uri =
            format!("/api/v1/teams/{team_id}/approval-requests?offset=10&limit=5&statuses=pending");
        let req = test::TestRequest::get().uri(&uri).to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = test::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["items"][0]["id"], request_id.to_string());
        assert_eq!(json["meta"]["offset"], 10);
        assert_eq!(json["meta"]["limit"], 5);
        assert_eq!(json["meta"]["total"], 1);
    }

    #[actix_web::test]
    async fn approve_request_returns_updated() {
        let request_id = Uuid::new_v4();
        let request = sample_request(request_id);

        let mut mock_logic = MockApprovalLogic::new();
        mock_logic
            .expect_approve_request()
            .times(1)
            .returning(move |_, _, _| Ok(request.clone()));

        let mut mock_repo = MockApprovalRepository::new();
        mock_repo
            .expect_list_votes_for_request()
            .times(1)
            .returning(move |_| Ok(vec![sample_vote(request_id)]));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn ApprovalLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_repo) as Box<dyn ApprovalRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri(&format!("/api/v1/approval-requests/{request_id}/approve"))
            .set_json(ApprovalActionRequest {
                comment: Some("Looks good".to_string()),
            })
            .to_request();
        req.extensions_mut().insert(JwtUser {
            id: Uuid::new_v4(),
            username: "tester".to_string(),
            is_admin: true,
            roles: vec!["Approver".to_string()],
            token_hash: "hash".to_string(),
        });
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn create_policy_with_empty_name_returns_bad_request() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/feature_toggle")
            .unwrap();
        let mock_activity = MockActivityLogRepository::new();
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(mock_activity) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri(&format!(
                "/api/v1/teams/{}/approval-policies",
                Uuid::new_v4()
            ))
            .set_json(CreateApprovalPolicyRequest {
                name: "   ".to_string(),
                description: None,
                applies_to: AppliesTo::All,
                environment_ids: None,
                required_approvers: 1,
                approver_role_ids: vec!["role-1".to_string()],
                auto_approve_after_hours: None,
                enabled: Some(true),
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
