use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, delete, get, patch, post, web};
use chrono::{DateTime, Utc};
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::ai::{AiJudgment, AiJudgmentRepository};
use crate::database::approval::{
    ApprovalRepository, CreateApprovalPolicyInput, UpdateApprovalPolicyInput,
    approval_repository_tx,
};
use crate::database::entity::{ApprovalPolicy, ApprovalRequest, ApprovalStatus, ApprovalVote};
use crate::database::feature::{FeatureVersionDiffEntry, diff_feature_snapshots};
use crate::judgment::{JudgmentKind, SubjectType};
use crate::logic::ActorContext;
use crate::logic::approval::{ApprovalLogic, ApprovalPolicyPreview, ApprovalPolicyPreviewOutcome};
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
    pub reviewed_snapshot_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDiffEntryResponse {
    pub path: String,
    pub section: String,
    pub label: String,
    pub change_type: String,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub risk_markers: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalChangeDiffResponse {
    pub snapshot_id: Option<String>,
    pub before_snapshot: Option<serde_json::Value>,
    pub after_snapshot: Option<serde_json::Value>,
    pub entries: Vec<ApprovalDiffEntryResponse>,
    pub risk_markers: Vec<String>,
    pub raw_snapshot: serde_json::Value,
    pub malformed: bool,
}

#[derive(Debug, Serialize, ToSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicySummaryResponse {
    pub id: String,
    pub name: String,
    pub applies_to: AppliesTo,
    pub required_approvers: i32,
    pub approver_role_ids: Vec<String>,
    pub approver_user_ids: Vec<String>,
    pub allow_admin_override: bool,
    pub fallback_to_roles: bool,
    pub auto_approve_after_hours: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicyPreviewRequest {
    pub change_type: String,
    pub environment_id: String,
    pub requested_status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicyPreviewOutcomeResponse {
    NotRequired,
    RequiresApproval,
    AutoApproved,
    Blocked,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPolicyPreviewResponse {
    pub change_type: String,
    pub requested_status: Option<String>,
    pub environment_id: String,
    pub outcome: ApprovalPolicyPreviewOutcomeResponse,
    pub policy: Option<ApprovalPolicySummaryResponse>,
    pub eligible_approvers_count: Option<i64>,
    pub warnings: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AiRiskStatus {
    Pending,
    Done,
    Failed,
}

/// The AI risk assessment of an approval request. Advisory: `level`, `reasons`
/// and `signals` are set only when `status` is `done`.
#[derive(Debug, Serialize, ToSchema, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AiRiskSummary {
    pub status: AiRiskStatus,
    /// `low`, `medium`, or `high`.
    pub level: Option<String>,
    pub reasons: Vec<String>,
    pub signals: Option<serde_json::Value>,
    pub model: Option<String>,
    pub assessed_at: Option<DateTime<Utc>>,
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
    pub eligible_approver_ids: Vec<String>,
    pub routing_reason: Option<String>,
    pub admin_override_enabled: bool,
    pub status: ApprovalRequestStatus,
    pub approved_count: i32,
    pub rejected_count: i32,
    pub executed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub votes: Vec<ApprovalVoteResponse>,
    pub change_diff: ApprovalChangeDiffResponse,
    pub policy: Option<ApprovalPolicySummaryResponse>,
    /// `null` when no AI assessment exists (feature off, no key, or not yet queued).
    pub ai_risk: Option<AiRiskSummary>,
    /// Approvals needed to approve the request. The policy's `requiredApprovers`
    /// until AI risk enforcement raises it.
    pub required_approvals_effective: i32,
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
    pub approver_user_ids: Vec<String>,
    pub allow_admin_override: bool,
    pub fallback_to_roles: bool,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    /// One of `off`, `advisory`, `gate_auto_approve`, `require_extra_approver`.
    pub ai_risk_mode: String,
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
    pub approver_user_ids: Vec<String>,
    pub allow_admin_override: Option<bool>,
    pub fallback_to_roles: Option<bool>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
    /// One of `off`, `advisory` (default), `gate_auto_approve`, `require_extra_approver`.
    pub ai_risk_mode: Option<String>,
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
    pub approver_user_ids: Option<Vec<String>>,
    pub allow_admin_override: Option<bool>,
    pub fallback_to_roles: Option<bool>,
    pub auto_approve_after_hours: Option<i32>,
    pub enabled: Option<bool>,
    /// One of `off`, `advisory`, `gate_auto_approve`, `require_extra_approver`. Unchanged when absent.
    pub ai_risk_mode: Option<String>,
}

/// Allowed values of `approval_policies.ai_risk_mode`.
pub const AI_RISK_MODES: [&str; 4] = [
    "off",
    "advisory",
    "gate_auto_approve",
    "require_extra_approver",
];

pub const DEFAULT_AI_RISK_MODE: &str = "advisory";

fn validate_ai_risk_mode(mode: &str) -> Result<(), RestError> {
    if AI_RISK_MODES.contains(&mode) {
        return Ok(());
    }
    Err(RestError::invalid_input(format!(
        "aiRiskMode must be one of: {}",
        AI_RISK_MODES.join(", ")
    )))
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

fn validate_approver_routing(roles: &[String], users: &[String]) -> Result<(), RestError> {
    if roles.is_empty() && users.is_empty() {
        return Err(RestError::invalid_input(
            "At least one approver role or explicit approver user is required",
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

fn payload_value<'a>(
    payload: &'a serde_json::Value,
    snake_case: &str,
    camel_case: &str,
) -> Option<&'a serde_json::Value> {
    payload.get(snake_case).or_else(|| payload.get(camel_case))
}

fn snapshot_id_from_payload(payload: &serde_json::Value, fallback: Uuid) -> Option<String> {
    payload_value(payload, "snapshot_id", "snapshotId")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| Some(fallback.to_string()))
}

fn applies_to_from_policy(policy: &ApprovalPolicy) -> AppliesTo {
    match policy.applies_to.as_str() {
        "production_only" => AppliesTo::ProductionOnly,
        "specific_environments" => AppliesTo::SpecificEnvironments,
        _ => AppliesTo::All,
    }
}

fn map_vote(vote: ApprovalVote, reviewed_snapshot_id: Option<String>) -> ApprovalVoteResponse {
    ApprovalVoteResponse {
        id: vote.id.to_string(),
        approver_id: vote.approver_id.to_string(),
        vote: vote.vote.as_str().to_string(),
        comment: vote.comment,
        reviewed_snapshot_id,
        created_at: vote.created_at,
    }
}

fn map_policy_summary(policy: &ApprovalPolicy) -> ApprovalPolicySummaryResponse {
    ApprovalPolicySummaryResponse {
        id: policy.id.to_string(),
        name: policy.name.clone(),
        applies_to: applies_to_from_policy(policy),
        required_approvers: policy.required_approvers,
        approver_role_ids: policy
            .approver_role_ids
            .iter()
            .map(|id| id.to_string())
            .collect(),
        approver_user_ids: policy
            .approver_user_ids
            .iter()
            .map(|id| id.to_string())
            .collect(),
        allow_admin_override: policy.allow_admin_override,
        fallback_to_roles: policy.fallback_to_roles,
        auto_approve_after_hours: policy.auto_approve_after_hours,
    }
}

fn map_preview_outcome(
    outcome: ApprovalPolicyPreviewOutcome,
) -> ApprovalPolicyPreviewOutcomeResponse {
    match outcome {
        ApprovalPolicyPreviewOutcome::NotRequired => {
            ApprovalPolicyPreviewOutcomeResponse::NotRequired
        }
        ApprovalPolicyPreviewOutcome::RequiresApproval => {
            ApprovalPolicyPreviewOutcomeResponse::RequiresApproval
        }
        ApprovalPolicyPreviewOutcome::AutoApproved => {
            ApprovalPolicyPreviewOutcomeResponse::AutoApproved
        }
        ApprovalPolicyPreviewOutcome::Blocked => ApprovalPolicyPreviewOutcomeResponse::Blocked,
    }
}

fn map_policy_preview(
    preview: ApprovalPolicyPreview,
    change_type: String,
) -> ApprovalPolicyPreviewResponse {
    ApprovalPolicyPreviewResponse {
        change_type,
        requested_status: preview.requested_status,
        environment_id: preview.environment_id.to_string(),
        outcome: map_preview_outcome(preview.outcome),
        policy: preview.policy.as_ref().map(map_policy_summary),
        eligible_approvers_count: preview.eligible_approvers_count,
        warnings: preview.warnings,
        reason: preview.reason,
    }
}

fn diff_section(path: &str) -> String {
    if path.starts_with("feature.") || path == "feature" {
        "Metadata".to_string()
    } else if path.starts_with("stages") {
        "Stages".to_string()
    } else if path.starts_with("criteria") {
        "Criteria".to_string()
    } else if path.starts_with("variants") {
        "Variants".to_string()
    } else if path.starts_with("dependencies") {
        "Dependencies".to_string()
    } else if path.to_lowercase().contains("environment") {
        "Environments".to_string()
    } else {
        "Other".to_string()
    }
}

fn diff_label(path: &str) -> String {
    let label = path
        .replace("feature.", "")
        .replace('_', " ")
        .replace('.', " / ")
        .replace('[', " #")
        .replace(']', "");
    if label.is_empty() {
        "Snapshot".to_string()
    } else {
        label
    }
}

fn add_marker(markers: &mut Vec<String>, marker: &str) {
    if !markers.iter().any(|existing| existing == marker) {
        markers.push(marker.to_string());
    }
}

fn number_value(value: &Option<serde_json::Value>) -> Option<f64> {
    value.as_ref().and_then(|inner| match inner {
        serde_json::Value::Number(number) => number.as_f64(),
        _ => None,
    })
}

fn entry_risk_markers(
    entry: &FeatureVersionDiffEntry,
    policy: Option<&ApprovalPolicy>,
) -> Vec<String> {
    let mut markers = Vec::new();
    let path = entry.path.to_lowercase();

    if path.starts_with("dependencies") {
        add_marker(&mut markers, "dependency-change");
    }

    if path.contains("kill_switch") || path.contains("killswitch") || path.contains("rollback") {
        add_marker(&mut markers, "emergency-action");
    }

    if path.contains("weight")
        && let (Some(before), Some(after)) =
            (number_value(&entry.before), number_value(&entry.after))
        && after > before
    {
        add_marker(&mut markers, "rollout-increase");
    }

    let policy_is_production = policy
        .map(|policy| policy.applies_to == "production_only")
        .unwrap_or(false);
    let status_targets_prod = entry
        .after
        .as_ref()
        .and_then(|value| value.as_str())
        .map(|value| value.contains("DEPLOY") || value.contains("ROLLBACK"))
        .unwrap_or(false);
    if policy_is_production || (path.contains("status") && status_targets_prod) {
        add_marker(&mut markers, "production-impact");
    }

    markers
}

fn parse_payload_diff(
    payload: &serde_json::Value,
) -> (
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    Vec<FeatureVersionDiffEntry>,
    bool,
) {
    if !payload.is_object() {
        return (None, None, Vec::new(), true);
    }

    let before = payload_value(payload, "before_snapshot", "beforeSnapshot")
        .or_else(|| payload.get("before"))
        .cloned();
    let after = payload_value(payload, "after_snapshot", "afterSnapshot")
        .or_else(|| payload.get("after"))
        .cloned();

    if let (Some(before_snapshot), Some(after_snapshot)) = (&before, &after) {
        let entries = diff_feature_snapshots(before_snapshot, after_snapshot);
        return (before, after, entries, false);
    }

    if let Some(diff_value) = payload.get("diff") {
        match serde_json::from_value::<Vec<FeatureVersionDiffEntry>>(diff_value.clone()) {
            Ok(entries) => return (before, after, entries, false),
            Err(_) => return (before, after, Vec::new(), true),
        }
    }

    (before, after, Vec::new(), false)
}

fn build_change_diff(
    request_id: Uuid,
    payload: &serde_json::Value,
    policy: Option<&ApprovalPolicy>,
) -> ApprovalChangeDiffResponse {
    let snapshot_id = snapshot_id_from_payload(payload, request_id);
    let (before_snapshot, after_snapshot, entries, malformed) = parse_payload_diff(payload);
    let mut risk_markers = Vec::new();
    let entries = entries
        .into_iter()
        .map(|entry| {
            let entry_markers = entry_risk_markers(&entry, policy);
            for marker in &entry_markers {
                add_marker(&mut risk_markers, marker);
            }
            ApprovalDiffEntryResponse {
                section: diff_section(&entry.path),
                label: diff_label(&entry.path),
                path: entry.path,
                change_type: entry.change_type,
                before: entry.before,
                after: entry.after,
                risk_markers: entry_markers,
            }
        })
        .collect();

    if let Some(raw_markers) =
        payload_value(payload, "risk_markers", "riskMarkers").and_then(|value| value.as_array())
    {
        for marker in raw_markers.iter().filter_map(|value| value.as_str()) {
            add_marker(&mut risk_markers, marker);
        }
    }

    ApprovalChangeDiffResponse {
        snapshot_id,
        before_snapshot,
        after_snapshot,
        entries,
        risk_markers,
        raw_snapshot: payload.clone(),
        malformed,
    }
}

/// Maps a stored judgment row to the response summary. No row, or a status
/// this code does not know, means no assessment.
pub(crate) fn map_ai_risk(judgment: Option<&AiJudgment>) -> Option<AiRiskSummary> {
    let judgment = judgment?;
    let status = match judgment.status.as_str() {
        "pending" => AiRiskStatus::Pending,
        "done" => AiRiskStatus::Done,
        "failed" => AiRiskStatus::Failed,
        _ => return None,
    };
    if status != AiRiskStatus::Done {
        return Some(AiRiskSummary {
            status,
            level: None,
            reasons: Vec::new(),
            signals: None,
            model: None,
            assessed_at: None,
        });
    }

    let derived = judgment.derived.as_ref();
    Some(AiRiskSummary {
        status,
        level: derived
            .and_then(|value| value.get("level"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        reasons: derived
            .and_then(|value| value.get("reasons"))
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        signals: derived.and_then(|value| value.get("signals")).cloned(),
        model: judgment.model.clone(),
        assessed_at: judgment.completed_at,
    })
}

/// Loads the AI risk summaries of many requests with one query. Fails open: a
/// read error is logged and every request shows no assessment.
pub(crate) async fn load_ai_risk(
    repo: &dyn AiJudgmentRepository,
    request_ids: Vec<Uuid>,
) -> HashMap<Uuid, AiRiskSummary> {
    if request_ids.is_empty() {
        return HashMap::new();
    }
    match repo
        .get_for_subjects(
            SubjectType::ApprovalRequest,
            request_ids,
            JudgmentKind::ApprovalRisk,
        )
        .await
    {
        Ok(rows) => rows
            .iter()
            .filter_map(|row| map_ai_risk(Some(row)).map(|summary| (row.subject_id, summary)))
            .collect(),
        Err(err) => {
            warn!("Could not load AI risk assessments: {err}");
            HashMap::new()
        }
    }
}

/// Approvals needed. The request's AI risk override wins. Otherwise the policy
/// applies, falling back to the snapshot in the request payload when the policy
/// row is gone.
fn required_approvals_effective(
    required_approvers_override: Option<i32>,
    policy: Option<&ApprovalPolicy>,
    change_payload: &serde_json::Value,
) -> i32 {
    required_approvers_override
        .or_else(|| policy.map(|policy| policy.required_approvers))
        .or_else(|| {
            change_payload
                .get("policy")
                .and_then(|policy| policy.get("required_approvers"))
                .and_then(|value| value.as_i64())
                .and_then(|value| i32::try_from(value).ok())
        })
        .unwrap_or(1)
}

pub(crate) fn map_request_with_policy(
    request: ApprovalRequest,
    votes: Vec<ApprovalVote>,
    policy: Option<&ApprovalPolicy>,
    ai_risk: Option<AiRiskSummary>,
) -> ApprovalRequestResponse {
    let reviewed_snapshot_id = snapshot_id_from_payload(&request.change_payload, request.id);
    let change_diff = build_change_diff(request.id, &request.change_payload, policy);
    let required_approvals_effective = required_approvals_effective(
        request.required_approvers_override,
        policy,
        &request.change_payload,
    );
    let policy = policy.map(map_policy_summary);

    ApprovalRequestResponse {
        id: request.id.to_string(),
        policy_id: request.policy_id.to_string(),
        feature_id: request.feature_id.to_string(),
        environment_id: request.environment_id.map(|id| id.to_string()),
        change_type: request.change_type,
        change_payload: request.change_payload,
        change_description: request.change_description,
        requested_by: request.requested_by.to_string(),
        eligible_approver_ids: request
            .eligible_approver_ids
            .iter()
            .map(|id| id.to_string())
            .collect(),
        routing_reason: request.routing_reason,
        admin_override_enabled: request.admin_override_enabled,
        status: ApprovalRequestStatus::from(request.status),
        approved_count: request.approved_count,
        rejected_count: request.rejected_count,
        executed_at: request.executed_at,
        created_at: request.created_at,
        updated_at: request.updated_at,
        votes: votes
            .into_iter()
            .map(|vote| map_vote(vote, reviewed_snapshot_id.clone()))
            .collect(),
        change_diff,
        policy,
        ai_risk,
        required_approvals_effective,
    }
}

fn map_policy(policy: ApprovalPolicy) -> ApprovalPolicyResponse {
    let applies_to = applies_to_from_policy(&policy);
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
        approver_user_ids: policy
            .approver_user_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
        allow_admin_override: policy.allow_admin_override,
        fallback_to_roles: policy.fallback_to_roles,
        auto_approve_after_hours: policy.auto_approve_after_hours,
        enabled: policy.enabled,
        created_at: policy.created_at,
        ai_risk_mode: policy.ai_risk_mode,
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/teams/{team_id}/approval-policy-preview",
    request_body = ApprovalPolicyPreviewRequest,
    params(
        ("team_id" = String, Path, description = "Team ID")
    ),
    responses(
        (status = 200, description = "Approval policy preview", body = ApprovalPolicyPreviewResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "Approvals"
)]
#[post("/teams/{team_id}/approval-policy-preview")]
pub(crate) async fn preview_approval_policy(
    pool: Option<web::Data<sqlx::PgPool>>,
    logic: web::Data<Box<dyn ApprovalLogic>>,
    team_id: web::Path<String>,
    payload: web::Json<ApprovalPolicyPreviewRequest>,
) -> Result<impl Responder, RestError> {
    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let environment_uuid = parse_uuid(&payload.environment_id, "environment_id")?;
    let change_type = payload.change_type.trim();
    if change_type.is_empty() {
        return Err(RestError::invalid_input("changeType is required"));
    }
    if !matches!(
        change_type,
        "stage_change" | "feature_change" | "emergency_action"
    ) {
        return Err(RestError::invalid_input(
            "changeType must be stage_change, feature_change, or emergency_action",
        ));
    }

    let requested_status = payload
        .requested_status
        .as_deref()
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .unwrap_or("DEPLOYMENT_REQUESTED");

    let mut preview = logic
        .preview_stage_change_policy(team_uuid, environment_uuid, requested_status)
        .await
        .map_err(RestError::from)?;
    if let Some(pool) = pool {
        if let Some(window) = crate::rest::operational_safety::active_freeze_for_environment(
            pool.get_ref(),
            team_uuid,
            environment_uuid,
            Utc::now(),
        )
        .await
        .map_err(RestError::from)?
        {
            preview.warnings.push(format!(
                "Active freeze window '{}' applies to this environment",
                window.name
            ));
        }
    }

    Ok(HttpResponse::Ok().json(map_policy_preview(preview, change_type.to_string())))
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
    ai_judgments: web::Data<Box<dyn AiJudgmentRepository>>,
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

    let mut ai_risk = load_ai_risk(
        ai_judgments.get_ref().as_ref(),
        requests.iter().map(|request| request.id).collect(),
    )
    .await;

    let mut items = Vec::with_capacity(requests.len());
    for request in requests {
        let policy = repo
            .get_policy_by_id(request.policy_id)
            .await
            .map_err(RestError::from)?;
        let votes = repo
            .list_votes_for_request(request.id)
            .await
            .map_err(RestError::from)?;
        let summary = ai_risk.remove(&request.id);
        items.push(map_request_with_policy(
            request,
            votes,
            policy.as_ref(),
            summary,
        ));
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
    ai_judgments: web::Data<Box<dyn AiJudgmentRepository>>,
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
    let policy = repo
        .get_policy_by_id(updated.policy_id)
        .await
        .map_err(RestError::from)?;

    let ai_risk = load_ai_risk(ai_judgments.get_ref().as_ref(), vec![updated.id])
        .await
        .remove(&updated.id);

    Ok(HttpResponse::Ok().json(map_request_with_policy(
        updated,
        votes,
        policy.as_ref(),
        ai_risk,
    )))
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
    ai_judgments: web::Data<Box<dyn AiJudgmentRepository>>,
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
    let policy = repo
        .get_policy_by_id(updated.policy_id)
        .await
        .map_err(RestError::from)?;

    let ai_risk = load_ai_risk(ai_judgments.get_ref().as_ref(), vec![updated.id])
        .await
        .remove(&updated.id);

    Ok(HttpResponse::Ok().json(map_request_with_policy(
        updated,
        votes,
        policy.as_ref(),
        ai_risk,
    )))
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
    ai_judgments: web::Data<Box<dyn AiJudgmentRepository>>,
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
    let policy = repo
        .get_policy_by_id(updated.policy_id)
        .await
        .map_err(RestError::from)?;

    let ai_risk = load_ai_risk(ai_judgments.get_ref().as_ref(), vec![updated.id])
        .await
        .remove(&updated.id);

    Ok(HttpResponse::Ok().json(map_request_with_policy(
        updated,
        votes,
        policy.as_ref(),
        ai_risk,
    )))
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
    validate_approver_routing(&payload.approver_role_ids, &payload.approver_user_ids)?;
    validate_auto_approve(payload.auto_approve_after_hours)?;
    if let Some(mode) = payload.ai_risk_mode.as_deref() {
        validate_ai_risk_mode(mode)?;
    }

    let team_uuid = parse_uuid(&team_id, "team_id")?;
    let env_ids = normalize_environment_ids(payload.applies_to, payload.environment_ids.clone())?;
    let role_ids = parse_uuid_list(&payload.approver_role_ids, "role_id")?;
    let user_ids = parse_uuid_list(&payload.approver_user_ids, "user_id")?;

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
            approver_user_ids: user_ids,
            allow_admin_override: payload.allow_admin_override.unwrap_or(false),
            fallback_to_roles: payload.fallback_to_roles.unwrap_or(true),
            auto_approve_after_hours: payload.auto_approve_after_hours,
            enabled: payload.enabled.unwrap_or(true),
            ai_risk_mode: payload
                .ai_risk_mode
                .clone()
                .unwrap_or_else(|| DEFAULT_AI_RISK_MODE.to_string()),
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
    if let (Some(roles), Some(users)) = (&payload.approver_role_ids, &payload.approver_user_ids) {
        validate_approver_routing(roles, users)?;
    }
    validate_auto_approve(payload.auto_approve_after_hours)?;
    if let Some(mode) = payload.ai_risk_mode.as_deref() {
        validate_ai_risk_mode(mode)?;
    }

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
    let user_ids = payload
        .approver_user_ids
        .clone()
        .map(|ids| parse_uuid_list(&ids, "user_id"))
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
            approver_user_ids: user_ids,
            allow_admin_override: payload.allow_admin_override,
            fallback_to_roles: payload.fallback_to_roles,
            auto_approve_after_hours: payload.auto_approve_after_hours,
            enabled: payload.enabled,
            ai_risk_mode: payload.ai_risk_mode.clone(),
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
        .service(preview_approval_policy)
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
    use crate::database::ai::MockAiJudgmentRepository;
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
            change_payload: serde_json::json!({
                "stage_id": "stage-1",
                "snapshot_id": "snapshot-1",
                "before_snapshot": {
                    "feature": { "key": "checkout", "description": "old" },
                    "stages": [{ "id": "stage-1", "status": "NOT_DEPLOYED" }],
                    "criteria": [],
                    "variants": [],
                    "dependencies": []
                },
                "after_snapshot": {
                    "feature": { "key": "checkout", "description": "new" },
                    "stages": [{ "id": "stage-1", "status": "DEPLOYMENT_APPROVED" }],
                    "criteria": [],
                    "variants": [],
                    "dependencies": []
                }
            }),
            change_description: Some("Deploy".to_string()),
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: Vec::new(),
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
        }
    }

    fn sample_policy(policy_id: Uuid) -> ApprovalPolicy {
        ApprovalPolicy {
            id: policy_id,
            team_id: Uuid::new_v4(),
            name: "Production approval".to_string(),
            description: Some("Prod changes need review".to_string()),
            applies_to: "production_only".to_string(),
            environment_ids: None,
            required_approvers: 2,
            approver_role_ids: vec![Uuid::new_v4()],
            approver_user_ids: Vec::new(),
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: "advisory".to_string(),
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
    async fn preview_approval_policy_returns_policy_decision() {
        let team_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();
        let policy_id = Uuid::new_v4();
        let policy = sample_policy(policy_id);

        let mut logic = MockApprovalLogic::new();
        logic
            .expect_preview_stage_change_policy()
            .withf(move |team, environment, status| {
                *team == team_id
                    && *environment == environment_id
                    && status == "DEPLOYMENT_REQUESTED"
            })
            .times(1)
            .returning(move |_, _, _| {
                Ok(ApprovalPolicyPreview {
                    change_type: "stage_change".to_string(),
                    requested_status: Some("DEPLOYMENT_REQUESTED".to_string()),
                    environment_id,
                    outcome: ApprovalPolicyPreviewOutcome::RequiresApproval,
                    policy: Some(policy.clone()),
                    eligible_approvers_count: Some(3),
                    warnings: Vec::new(),
                    reason: "Matched policy requires manual approval".to_string(),
                })
            });

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Box::new(logic) as Box<dyn ApprovalLogic>))
                .service(preview_approval_policy),
        )
        .await;

        let req = test::TestRequest::post()
            .uri(&format!("/teams/{team_id}/approval-policy-preview"))
            .set_json(ApprovalPolicyPreviewRequest {
                change_type: "stage_change".to_string(),
                environment_id: environment_id.to_string(),
                requested_status: Some("DEPLOYMENT_REQUESTED".to_string()),
            })
            .to_request();

        let response: serde_json::Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(response["outcome"], "requires_approval");
        assert_eq!(response["policy"]["name"], "Production approval");
        assert_eq!(response["eligibleApproversCount"], 3);
    }

    #[actix_web::test]
    async fn list_approval_requests_returns_items_and_meta() {
        let team_id = Uuid::new_v4();
        let request_id = Uuid::new_v4();
        let request = sample_request(request_id);
        let policy_id = request.policy_id;
        let policy = sample_policy(policy_id);

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
            .expect_get_policy_by_id()
            .withf(move |id| *id == policy_id)
            .times(1)
            .returning(move |_| Ok(Some(policy.clone())));
        mock_repo
            .expect_list_votes_for_request()
            .withf(move |id| *id == request_id)
            .times(1)
            .returning(move |_| Ok(vec![sample_vote(request_id)]));

        let mut ai_repo = MockAiJudgmentRepository::new();
        ai_repo
            .expect_get_for_subjects()
            .times(1)
            .returning(|_, _, _| Ok(vec![]));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_repo) as Box<dyn ApprovalRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(ai_repo) as Box<dyn AiJudgmentRepository>
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
        assert_eq!(json["items"][0]["policy"]["name"], "Production approval");
        assert_eq!(
            json["items"][0]["votes"][0]["reviewedSnapshotId"],
            "snapshot-1"
        );
        assert_eq!(
            json["items"][0]["changeDiff"]["entries"][0]["section"],
            "Metadata"
        );
        assert_eq!(json["meta"]["offset"], 10);
        assert_eq!(json["meta"]["limit"], 5);
        assert_eq!(json["meta"]["total"], 1);
        assert_eq!(json["items"][0]["aiRisk"], serde_json::Value::Null);
        assert_eq!(json["items"][0]["requiredApprovalsEffective"], 2);
    }

    #[actix_web::test]
    async fn map_request_handles_malformed_payload_without_panicking() {
        let request_id = Uuid::new_v4();
        let request = ApprovalRequest {
            change_payload: serde_json::json!("not-a-json-object"),
            ..sample_request(request_id)
        };
        let policy = sample_policy(request.policy_id);

        let response = map_request_with_policy(request, vec![], Some(&policy), None);

        assert!(response.change_diff.malformed);
        assert!(response.change_diff.entries.is_empty());
        assert_eq!(response.policy.unwrap().name, "Production approval");
    }

    #[actix_web::test]
    async fn approve_request_returns_updated() {
        let request_id = Uuid::new_v4();
        let request = sample_request(request_id);
        let policy_id = request.policy_id;
        let policy = sample_policy(policy_id);

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
        mock_repo
            .expect_get_policy_by_id()
            .withf(move |id| *id == policy_id)
            .times(1)
            .returning(move |_| Ok(Some(policy.clone())));

        let mut ai_repo = MockAiJudgmentRepository::new();
        ai_repo
            .expect_get_for_subjects()
            .times(1)
            .returning(|_, _, _| Ok(vec![]));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_logic) as Box<dyn ApprovalLogic>
                ))
                .app_data(web::Data::new(
                    Box::new(mock_repo) as Box<dyn ApprovalRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(ai_repo) as Box<dyn AiJudgmentRepository>
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
            team_id: None,
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
                approver_user_ids: Vec::new(),
                allow_admin_override: Some(false),
                fallback_to_roles: Some(true),
                auto_approve_after_hours: None,
                enabled: Some(true),
                ai_risk_mode: None,
            })
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
    fn judgment_row(
        request_id: Uuid,
        status: &str,
        derived: Option<serde_json::Value>,
    ) -> AiJudgment {
        AiJudgment {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            subject_type: "approval_request".to_string(),
            subject_id: request_id,
            kind: "approval_risk".to_string(),
            status: status.to_string(),
            attempts: 1,
            input: serde_json::json!({}),
            input_hash: "h".to_string(),
            model: Some("jev-1.13.0".to_string()),
            raw_answers: None,
            derived,
            input_tokens: None,
            error: Some("boom".to_string()),
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
        }
    }

    fn done_derived() -> serde_json::Value {
        serde_json::json!({
            "level": "high",
            "reasons": ["Weakens a safety control"],
            "signals": { "overall_risk": 2.5 },
        })
    }

    #[actix_web::test]
    async fn ai_risk_is_none_without_a_judgment() {
        assert_eq!(map_ai_risk(None), None);
    }

    #[actix_web::test]
    async fn ai_risk_pending_and_failed_pass_through_without_a_result() {
        for (status, expected) in [
            ("pending", AiRiskStatus::Pending),
            ("failed", AiRiskStatus::Failed),
        ] {
            // Even when a stale derived value is stored, only `done` exposes it.
            let row = judgment_row(Uuid::new_v4(), status, Some(done_derived()));
            let summary = map_ai_risk(Some(&row)).unwrap();

            assert_eq!(summary.status, expected);
            assert_eq!(summary.level, None);
            assert!(summary.reasons.is_empty());
            assert_eq!(summary.signals, None);
            assert_eq!(summary.model, None);
            assert_eq!(summary.assessed_at, None);
        }
    }

    #[actix_web::test]
    async fn ai_risk_done_exposes_level_reasons_and_signals() {
        let row = judgment_row(Uuid::new_v4(), "done", Some(done_derived()));
        let summary = map_ai_risk(Some(&row)).unwrap();

        assert_eq!(summary.status, AiRiskStatus::Done);
        assert_eq!(summary.level.as_deref(), Some("high"));
        assert_eq!(
            summary.reasons,
            vec!["Weakens a safety control".to_string()]
        );
        assert_eq!(
            summary.signals,
            Some(serde_json::json!({ "overall_risk": 2.5 }))
        );
        assert_eq!(summary.model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(summary.assessed_at, row.completed_at);
    }

    #[actix_web::test]
    async fn ai_risk_unknown_status_means_no_assessment() {
        let row = judgment_row(Uuid::new_v4(), "weird", None);
        assert_eq!(map_ai_risk(Some(&row)), None);
    }

    #[actix_web::test]
    async fn required_approvals_effective_uses_policy_then_payload_snapshot() {
        let policy = sample_policy(Uuid::new_v4());
        let payload = serde_json::json!({ "policy": { "required_approvers": 4 } });

        assert_eq!(
            required_approvals_effective(None, Some(&policy), &payload),
            2
        );
        assert_eq!(required_approvals_effective(None, None, &payload), 4);
        assert_eq!(
            required_approvals_effective(None, None, &serde_json::json!({})),
            1
        );
    }

    #[actix_web::test]
    async fn required_approvals_effective_prefers_the_request_override() {
        let policy = sample_policy(Uuid::new_v4());
        let payload = serde_json::json!({ "policy": { "required_approvers": 4 } });

        assert_eq!(
            required_approvals_effective(Some(3), Some(&policy), &payload),
            3
        );
        assert_eq!(required_approvals_effective(Some(3), None, &payload), 3);
    }

    #[actix_web::test]
    async fn list_loads_ai_risk_in_one_query_and_maps_each_status() {
        let team_id = Uuid::new_v4();
        let ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
        let policy_id = Uuid::new_v4();
        let requests: Vec<ApprovalRequest> = ids
            .iter()
            .map(|id| ApprovalRequest {
                policy_id,
                ..sample_request(*id)
            })
            .collect();
        let policy = sample_policy(policy_id);

        let mut mock_repo = MockApprovalRepository::new();
        let listed = requests.clone();
        mock_repo
            .expect_list_requests_for_team_with_offset()
            .returning(move |_, _, _, _| Ok((listed.clone(), 4)));
        mock_repo
            .expect_get_policy_by_id()
            .returning(move |_| Ok(Some(policy.clone())));
        mock_repo
            .expect_list_votes_for_request()
            .returning(|_| Ok(vec![]));

        // ids[0] has no row; ids[1..] are pending, done, failed.
        let rows = vec![
            judgment_row(ids[1], "pending", None),
            judgment_row(ids[2], "done", Some(done_derived())),
            judgment_row(ids[3], "failed", None),
        ];
        let expected_ids = ids.clone();
        let mut ai_repo = MockAiJudgmentRepository::new();
        ai_repo
            .expect_get_for_subjects()
            .withf(move |subject_type, subject_ids, kind| {
                *subject_type == SubjectType::ApprovalRequest
                    && *kind == JudgmentKind::ApprovalRisk
                    && subject_ids == &expected_ids
            })
            .times(1)
            .returning(move |_, _, _| Ok(rows.clone()));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_repo) as Box<dyn ApprovalRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(ai_repo) as Box<dyn AiJudgmentRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!("/api/v1/teams/{team_id}/approval-requests"))
            .to_request();
        let json: serde_json::Value = test::call_and_read_body_json(&app, req).await;
        let items = &json["items"];

        assert_eq!(items[0]["aiRisk"], serde_json::Value::Null);
        assert_eq!(items[1]["aiRisk"]["status"], "pending");
        assert_eq!(items[1]["aiRisk"]["level"], serde_json::Value::Null);
        assert_eq!(items[1]["aiRisk"]["reasons"], serde_json::json!([]));
        assert_eq!(items[2]["aiRisk"]["status"], "done");
        assert_eq!(items[2]["aiRisk"]["level"], "high");
        assert_eq!(
            items[2]["aiRisk"]["reasons"],
            serde_json::json!(["Weakens a safety control"])
        );
        assert_eq!(items[2]["aiRisk"]["model"], "jev-1.13.0");
        assert!(items[2]["aiRisk"]["assessedAt"].is_string());
        assert_eq!(items[3]["aiRisk"]["status"], "failed");
        assert_eq!(items[3]["requiredApprovalsEffective"], 2);
    }

    #[actix_web::test]
    async fn list_fails_open_when_judgments_cannot_be_read() {
        let request_id = Uuid::new_v4();
        let request = sample_request(request_id);
        let policy = sample_policy(request.policy_id);

        let mut mock_repo = MockApprovalRepository::new();
        mock_repo
            .expect_list_requests_for_team_with_offset()
            .returning(move |_, _, _, _| Ok((vec![request.clone()], 1)));
        mock_repo
            .expect_get_policy_by_id()
            .returning(move |_| Ok(Some(policy.clone())));
        mock_repo
            .expect_list_votes_for_request()
            .returning(|_| Ok(vec![]));
        let mut ai_repo = MockAiJudgmentRepository::new();
        ai_repo
            .expect_get_for_subjects()
            .returning(|_, _, _| Err(crate::Error::InvalidInput("db down".to_string())));

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(
                    Box::new(mock_repo) as Box<dyn ApprovalRepository>
                ))
                .app_data(web::Data::new(
                    Box::new(ai_repo) as Box<dyn AiJudgmentRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let req = test::TestRequest::get()
            .uri(&format!(
                "/api/v1/teams/{}/approval-requests",
                Uuid::new_v4()
            ))
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&test::read_body(resp).await).unwrap();
        assert_eq!(json["items"][0]["aiRisk"], serde_json::Value::Null);
    }

    #[actix_web::test]
    async fn ai_risk_mode_accepts_only_the_four_modes() {
        for mode in AI_RISK_MODES {
            assert!(validate_ai_risk_mode(mode).is_ok(), "{mode}");
        }
        for mode in ["", "ADVISORY", "enforce", "gate"] {
            assert!(validate_ai_risk_mode(mode).is_err(), "{mode}");
        }
    }

    #[actix_web::test]
    async fn policy_response_exposes_ai_risk_mode_in_camel_case() {
        let mut policy = sample_policy(Uuid::new_v4());
        policy.ai_risk_mode = "gate_auto_approve".to_string();

        let json = serde_json::to_value(map_policy(policy)).unwrap();

        assert_eq!(json["aiRiskMode"], "gate_auto_approve");
    }

    #[actix_web::test]
    async fn create_and_update_policy_reject_unknown_ai_risk_mode() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/feature_toggle")
            .unwrap();
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(pool))
                .app_data(web::Data::new(
                    Box::new(MockActivityLogRepository::new()) as Box<dyn ActivityLogRepository>
                ))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;

        let create = test::TestRequest::post()
            .uri(&format!(
                "/api/v1/teams/{}/approval-policies",
                Uuid::new_v4()
            ))
            .set_json(serde_json::json!({
                "name": "Prod",
                "appliesTo": "all",
                "requiredApprovers": 1,
                "approverRoleIds": [Uuid::new_v4().to_string()],
                "approverUserIds": [],
                "aiRiskMode": "bogus",
            }))
            .to_request();
        assert_eq!(
            test::call_service(&app, create).await.status(),
            StatusCode::BAD_REQUEST
        );

        let update = test::TestRequest::patch()
            .uri(&format!("/api/v1/approval-policies/{}", Uuid::new_v4()))
            .set_json(serde_json::json!({ "aiRiskMode": "bogus" }))
            .to_request();
        assert_eq!(
            test::call_service(&app, update).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
}
