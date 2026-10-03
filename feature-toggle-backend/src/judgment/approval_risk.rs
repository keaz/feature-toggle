//! Approval risk triage (AI-10): rates how risky a stage-change approval
//! request is. Advisory, unless the policy mode enforces (AI-11); see
//! `docs/ai-judgments/design.md` §5.1.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::JudgmentKind;
use super::service::JudgmentHandler;
use super::types::{Answers, Question, RequestParts};
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::ai::AiJudgment;
use crate::database::approval::ApprovalRepository;
use crate::database::entity::ApprovalRequest;
use crate::database::entity::{ApprovalStatus, Feature, FeaturePipelineStage};
use crate::model::Environment;
use crate::utils::activity_logger::{activity_types, entity_types};

pub const MAX_DIFF_ENTRIES: usize = 30;
pub const MAX_VALUE_CHARS: usize = 200;

pub const HIGH_OVERALL_RISK: f64 = 2.2;
pub const HIGH_SAFETY_CONTROL: f64 = 0.7;
pub const HIGH_WIDENS_EXPOSURE: f64 = 0.7;
pub const HIGH_SENSITIVE_WITH_WIDENS: f64 = 0.6;
pub const MEDIUM_OVERALL_RISK: f64 = 1.3;
pub const MEDIUM_WIDENS_EXPOSURE: f64 = 0.7;
pub const MEDIUM_SENSITIVE: f64 = 0.7;
pub const LOW_CONFIDENCE: f64 = 0.3;

/// Longest `feature.description` / `feature.purpose` sent to Jev.
const MAX_TEXT_CHARS: usize = 500;

const Q_WIDENS_PROD_EXPOSURE: &str = "widens_prod_exposure";
const Q_DISABLES_SAFETY_CONTROL: &str = "disables_safety_control";
const Q_TOUCHES_SENSITIVE_DOMAIN: &str = "touches_sensitive_domain";
const Q_OVERALL_RISK: &str = "overall_risk";

const REASON_WIDENS: &str = "Widens production exposure";
const REASON_SAFETY: &str = "Weakens a safety control";
const REASON_SENSITIVE: &str = "Sensitive area (payments, auth, security, or personal data)";
const REASON_IMPACT: &str = "High potential user impact";
const REASON_UNCERTAIN: &str = "Assessment uncertain";

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A diff value as the short text Jev reads. `null` stays `null`.
fn diff_value_text(value: Option<&Value>) -> Value {
    match value {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(text)) => Value::String(truncate_chars(text, MAX_VALUE_CHARS)),
        Some(other) => Value::String(truncate_chars(&other.to_string(), MAX_VALUE_CHARS)),
    }
}

fn build_diff(change_payload: &Value) -> Vec<Value> {
    change_payload
        .get("diff")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .take(MAX_DIFF_ENTRIES)
                .map(|entry| {
                    json!({
                        "path": entry.get("path").cloned().unwrap_or(Value::Null),
                        "change_type": entry.get("change_type").cloned().unwrap_or(Value::Null),
                        "before": diff_value_text(entry.get("before")),
                        "after": diff_value_text(entry.get("after")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn client_count_bucket(count: i64) -> &'static str {
    match count {
        ..=0 => "none",
        1..=3 => "few",
        4..=10 => "several",
        _ => "many",
    }
}

fn evaluation_volume_bucket(volume: i64) -> &'static str {
    match volume {
        ..=0 => "none",
        1..=999 => "low",
        1_000..=99_999 => "medium",
        _ => "high",
    }
}

fn dependent_flags_bucket(count: i64) -> &'static str {
    match count {
        ..=0 => "none",
        1..=3 => "some",
        _ => "many",
    }
}

fn blast_i64(blast: &Value, key: &str) -> i64 {
    blast.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn lowercase_text(value: &str) -> String {
    value.to_lowercase()
}

/// The input snapshot sent to Jev (design §5.1). Numbers are bucketed, the diff
/// is capped and truncated, and no owner, user id, or email is included.
pub fn build_input(
    feature: &Feature,
    stage: &FeaturePipelineStage,
    environment: &Environment,
    change_payload: &Value,
) -> Value {
    let blast = change_payload.get("blast_radius").unwrap_or(&Value::Null);
    let text_field = |key: &str, fallback: &str| {
        change_payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string()
    };

    let mut environment_types: Vec<String> = blast
        .get("affectedEnvironments")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("environmentType").and_then(Value::as_str))
                .map(lowercase_text)
                .collect()
        })
        .unwrap_or_default();
    if environment_types.is_empty() {
        environment_types.push(lowercase_text(&environment.environment_type));
    }

    json!({
        "feature": {
            "key": feature.key,
            "description": feature.description.as_deref().map(|text| truncate_chars(text, MAX_TEXT_CHARS)),
            "purpose": feature.purpose.as_deref().map(|text| truncate_chars(text, MAX_TEXT_CHARS)),
            "tags": feature.tags,
            "flag_kind": feature.flag_kind,
        },
        "change": {
            "type": "stage_change",
            "environment_name": environment.name,
            "environment_type": lowercase_text(&environment.environment_type),
            "from_status": text_field("previous_status", &stage.status),
            "to_status": text_field("next_status", ""),
            "diff": build_diff(change_payload),
        },
        "impact": {
            "risk_level": blast.get("riskLevel").cloned().unwrap_or(Value::Null),
            "risk_markers": blast.get("riskMarkers").cloned().unwrap_or_else(|| json!([])),
            "affected_environment_types": environment_types,
            "client_count": client_count_bucket(blast_i64(blast, "affectedClients")),
            "evaluation_volume": evaluation_volume_bucket(blast_i64(blast, "evaluationVolume7d")),
            "dependent_flags": dependent_flags_bucket(blast_i64(blast, "dependencyCount")),
        },
    })
}

fn questions() -> BTreeMap<String, Question> {
    let widens = Question::noul_with(
        "Does `change` make `feature` reach more end users in a production environment than before? Count it as yes when targeting rules are removed or loosened, a rollout percentage goes up, or the feature is turned on in production.",
        "More production end users get the feature after this change.",
        "Production exposure stays the same or goes down, or the change is not in production.",
    );
    let safety = Question::noul_with(
        "Does `change` turn off, bypass, or weaken a safety control, such as a kill switch, circuit breaker, rate limit, fraud check, or permission check?",
        "A safety control is turned off, bypassed, or weakened.",
        "No safety control is weakened.",
    );
    let sensitive = Question::noul_with(
        "Does `feature` control behavior in a sensitive area: payments or billing, authentication or authorization, security, personal data or privacy, or data deletion?",
        "The feature controls one of these sensitive areas.",
        "The feature controls none of these areas.",
    );
    let overall = Question::score(
        "If this change turns out to be wrong, how serious is the harm to end users? Use `change` and `impact`.",
        [
            "No end-user impact: internal tooling, a development environment only, or a cosmetic change.",
            "Limited impact: a small or opt-in audience, or a problem that is easy to notice and revert.",
            "Broad impact: many end users in production see changed behavior.",
            "Critical impact: money, access, security, or data integrity for many users could be affected.",
        ],
    )
    .expect("overall_risk has 4 levels");

    BTreeMap::from([
        (Q_WIDENS_PROD_EXPOSURE.to_string(), widens),
        (Q_DISABLES_SAFETY_CONTROL.to_string(), safety),
        (Q_TOUCHES_SENSITIVE_DOMAIN.to_string(), sensitive),
        (Q_OVERALL_RISK.to_string(), overall),
    ])
}

/// Risk level and reasons from the four answers. A missing answer counts as 0
/// with zero confidence, so an incomplete answer set reads as uncertain.
fn risk_from_signals(
    widens: f64,
    safety: f64,
    sensitive: f64,
    overall: f64,
    confidence: f64,
) -> (&'static str, Vec<&'static str>) {
    let widens_flag = widens >= MEDIUM_WIDENS_EXPOSURE;
    let safety_flag = safety >= HIGH_SAFETY_CONTROL;
    let sensitive_with_widens =
        widens >= HIGH_WIDENS_EXPOSURE && sensitive >= HIGH_SENSITIVE_WITH_WIDENS;
    let sensitive_flag = sensitive >= MEDIUM_SENSITIVE || sensitive_with_widens;
    let impact_flag = overall >= MEDIUM_OVERALL_RISK;
    let uncertain = confidence < LOW_CONFIDENCE;

    let level = if overall >= HIGH_OVERALL_RISK || safety_flag || sensitive_with_widens {
        "high"
    } else if impact_flag || widens_flag || sensitive_flag || uncertain {
        "medium"
    } else {
        "low"
    };

    let reasons = [
        (widens_flag, REASON_WIDENS),
        (safety_flag, REASON_SAFETY),
        (sensitive_flag, REASON_SENSITIVE),
        (impact_flag, REASON_IMPACT),
        (uncertain, REASON_UNCERTAIN),
    ]
    .into_iter()
    .filter_map(|(crossed, reason)| crossed.then_some(reason))
    .collect();

    (level, reasons)
}

pub struct ApprovalRiskHandler {
    activity: Box<dyn ActivityLogRepository>,
    approvals: Box<dyn ApprovalRepository>,
}

impl ApprovalRiskHandler {
    pub fn new(
        activity: Box<dyn ActivityLogRepository>,
        approvals: Box<dyn ApprovalRepository>,
    ) -> Self {
        Self {
            activity,
            approvals,
        }
    }
}

/// What `require_extra_approver` did for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtraApprover {
    /// Not applicable (mode, level, closed request) or a lost update race.
    None,
    /// The request needs this many approvals.
    Required(i32),
    /// The level asked for an extra approver, but no additional eligible
    /// approver exists, so the requirement stays as the policy says.
    NotPossible,
}

/// The approvals a high-risk request needs under `require_extra_approver`:
/// the policy's count plus one, capped at the request's eligible approvers so
/// votes alone can still approve it. Routing resolves both named approvers and
/// role members into `eligible_approver_ids` (requester excluded), so the cap
/// applies to role-based policies too. The list is empty only for a request
/// created without a database pool (routing not resolved) or a legacy row
/// created before routing existed; then the count is not known and no cap
/// applies. `None` when the cap leaves nothing to add. Votes later count
/// against a requirement that is also capped at who can still vote
/// (`logic::approval::effective_required_approvals`).
fn extra_approver_requirement(required_approvers: i32, eligible_approvers: usize) -> Option<i32> {
    let raised = required_approvers.saturating_add(1);
    let target = if eligible_approvers == 0 {
        raised
    } else {
        raised.min(i32::try_from(eligible_approvers).unwrap_or(i32::MAX))
    };
    (target > required_approvers).then_some(target)
}

impl ApprovalRiskHandler {
    /// Applies `require_extra_approver` once `level` is known (user decision
    /// 2026-10-03: cap at the eligible approver count).
    async fn enforce_extra_approver(
        &self,
        request: &ApprovalRequest,
        level: &str,
    ) -> Result<ExtraApprover, crate::Error> {
        if level != "high" || !matches!(request.status, ApprovalStatus::Pending) {
            return Ok(ExtraApprover::None);
        }
        if let Some(existing) = request.required_approvers_override {
            return Ok(ExtraApprover::Required(existing));
        }
        let Some(policy) = self.approvals.get_policy_by_id(request.policy_id).await? else {
            return Ok(ExtraApprover::None);
        };
        if policy.ai_risk_mode != "require_extra_approver" {
            return Ok(ExtraApprover::None);
        }
        let Some(required) = extra_approver_requirement(
            policy.required_approvers,
            request.eligible_approver_ids.len(),
        ) else {
            return Ok(ExtraApprover::NotPossible);
        };
        let changed = self
            .approvals
            .set_required_approvers_override(request.id, required)
            .await?;
        Ok(if changed {
            ExtraApprover::Required(required)
        } else {
            ExtraApprover::None
        })
    }
}

#[async_trait]
impl JudgmentHandler for ApprovalRiskHandler {
    fn kind(&self) -> JudgmentKind {
        JudgmentKind::ApprovalRisk
    }

    fn build(&self, input: &Value) -> RequestParts {
        RequestParts {
            state: input.clone(),
            questions: questions(),
        }
    }

    fn derive(&self, _input: &Value, answers: &Answers) -> Value {
        let widens = answers.noul(Q_WIDENS_PROD_EXPOSURE).unwrap_or(0.0);
        let safety = answers.noul(Q_DISABLES_SAFETY_CONTROL).unwrap_or(0.0);
        let sensitive = answers.noul(Q_TOUCHES_SENSITIVE_DOMAIN).unwrap_or(0.0);
        let (overall, confidence) = answers
            .score(Q_OVERALL_RISK)
            .map(|answer| (answer.score, answer.confidence))
            .unwrap_or((0.0, 0.0));

        let (level, reasons) = risk_from_signals(widens, safety, sensitive, overall, confidence);
        json!({
            "level": level,
            "reasons": reasons,
            "signals": {
                "widens_prod_exposure": widens,
                "disables_safety_control": safety,
                "touches_sensitive_domain": sensitive,
                "overall_risk": overall,
                "overall_risk_confidence": confidence,
            },
        })
    }

    /// Records the assessment in the activity log. Skips when the request no
    /// longer exists. A closed request still gets its entry, because the
    /// assessment is history, not a decision.
    ///
    /// Enforcement (AI-11): for a `high` level on a still-pending request under
    /// a `require_extra_approver` policy, raises the approvals needed by one,
    /// capped at the request's eligible approver list (no cap when the list
    /// is empty). When the cap leaves nothing to add, no override is written
    /// and the entry says `extra_approver_skipped`.
    /// The update is guarded in SQL by `status = 'pending'`, so a closed request
    /// is never changed, and by `required_approvers_override IS NULL`, so a
    /// repeated apply keeps the first value.
    async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error> {
        let Some(request) = self
            .approvals
            .get_request_by_id(judgment.subject_id)
            .await?
        else {
            return Ok(());
        };
        let derived = judgment.derived.as_ref().unwrap_or(&Value::Null);
        let level = derived
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let reasons = derived.get("reasons").cloned().unwrap_or_else(|| json!([]));
        let extra_approver = self.enforce_extra_approver(&request, level).await?;

        let mut metadata = json!({
            "approval_request_id": request.id.to_string(),
            "feature_id": request.feature_id.to_string(),
            "level": level,
            "reasons": reasons,
            "model": judgment.model,
        });
        match extra_approver {
            ExtraApprover::Required(required) => {
                metadata["required_approvers_override"] = json!(required);
            }
            ExtraApprover::NotPossible => {
                metadata["extra_approver_skipped"] = json!("no_additional_eligible_approver");
            }
            ExtraApprover::None => {}
        }

        self.activity
            .create_activity(CreateActivityLog {
                activity_type: activity_types::APPROVAL_RISK_ASSESSED.to_string(),
                entity_type: entity_types::FEATURE.to_string(),
                entity_id: request.feature_id.to_string(),
                actor_id: None,
                actor_name: Some("AI risk assessment".to_string()),
                description: format!("AI risk assessment of approval request: {level}"),
                metadata: Some(metadata),
            })
            .await
            .map_err(crate::Error::DatabaseError)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::database::activity_log::{ActivityLogRow, MockActivityLogRepository};
    use crate::database::approval::MockApprovalRepository;
    use crate::database::entity::{ApprovalPolicy, ApprovalRequest, ApprovalStatus, FeatureType};
    use crate::judgment::types::{Answer, Question, ScoreAnswer};
    use crate::model::ID;

    fn feature() -> Feature {
        Feature {
            id: Uuid::new_v4(),
            key: "checkout_v2".into(),
            description: Some("New checkout flow".into()),
            feature_type: FeatureType::Simple,
            team_id: Uuid::new_v4(),
            active: true,
            created_at: Utc::now(),
            kill_switch_enabled: false,
            kill_switch_activated_at: None,
            rollback_scheduled_at: None,
            emergency_override_reason: None,
            emergency_override_expires_at: None,
            emergency_override_actor_id: None,
            emergency_override_applied_at: None,
            lifecycle_stage: "active".into(),
            owner: Some("alice@example.com".into()),
            purpose: Some("Roll out the new checkout".into()),
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec!["payments".into()],
            archived_at: None,
            deprecated_at: None,
            deprecation_notice: None,
            last_evaluated_at: None,
            evaluation_count_7d: 0,
            evaluation_count_30d: 0,
            evaluation_count_90d: 0,
            dependencies: vec![],
            flag_kind: None,
            flag_kind_confidence: None,
            flag_kind_source: None,
        }
    }

    fn stage() -> FeaturePipelineStage {
        FeaturePipelineStage {
            id: Uuid::new_v4(),
            feature_id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            order_index: 0,
            parent_stage_id: None,
            position: "production".into(),
            enabled: false,
            status: "NOT_DEPLOYED".into(),
        }
    }

    fn environment() -> Environment {
        Environment {
            id: ID::from(Uuid::new_v4()),
            name: "prod-eu".into(),
            team_id: ID::from(Uuid::new_v4()),
            active: true,
            environment_type: "Production".into(),
        }
    }

    fn payload(blast_radius: Value, diff: Value) -> Value {
        json!({
            "next_status": "DEPLOYMENT_REQUESTED",
            "previous_status": "NOT_DEPLOYED",
            "diff": diff,
            "risk_markers": ["production-impact"],
            "blast_radius": blast_radius,
        })
    }

    fn blast(clients: i64, volume: i64, deps: i64) -> Value {
        json!({
            "riskLevel": "high",
            "riskMarkers": ["production-impact", "high-traffic"],
            "affectedEnvironments": [{ "id": "e1", "name": "prod-eu", "environmentType": "Production" }],
            "affectedClients": clients,
            "dependencyCount": deps,
            "evaluationVolume7d": volume,
        })
    }

    fn input_for(clients: i64, volume: i64, deps: i64) -> Value {
        build_input(
            &feature(),
            &stage(),
            &environment(),
            &payload(blast(clients, volume, deps), json!([])),
        )
    }

    #[test]
    fn build_input_has_the_designed_shape() {
        let mut classified = feature();
        classified.flag_kind = Some(crate::model::FlagKind::Ops);
        let input = build_input(
            &classified,
            &stage(),
            &environment(),
            &payload(
                blast(5, 2_000, 1),
                json!([{ "path": "stages[0].status", "change_type": "changed",
                         "before": "NOT_DEPLOYED", "after": "DEPLOYMENT_APPROVED" }]),
            ),
        );

        assert_eq!(
            input,
            json!({
                "feature": {
                    "key": "checkout_v2",
                    "description": "New checkout flow",
                    "purpose": "Roll out the new checkout",
                    "tags": ["payments"],
                    "flag_kind": "ops",
                },
                "change": {
                    "type": "stage_change",
                    "environment_name": "prod-eu",
                    "environment_type": "production",
                    "from_status": "NOT_DEPLOYED",
                    "to_status": "DEPLOYMENT_REQUESTED",
                    "diff": [{ "path": "stages[0].status", "change_type": "changed",
                               "before": "NOT_DEPLOYED", "after": "DEPLOYMENT_APPROVED" }],
                },
                "impact": {
                    "risk_level": "high",
                    "risk_markers": ["production-impact", "high-traffic"],
                    "affected_environment_types": ["production"],
                    "client_count": "several",
                    "evaluation_volume": "medium",
                    "dependent_flags": "some",
                },
            })
        );
    }

    #[test]
    fn client_count_buckets() {
        for (count, expected) in [
            (0, "none"),
            (1, "few"),
            (3, "few"),
            (4, "several"),
            (10, "several"),
            (11, "many"),
        ] {
            assert_eq!(
                input_for(count, 0, 0)["impact"]["client_count"],
                expected,
                "{count} clients"
            );
        }
    }

    #[test]
    fn evaluation_volume_buckets() {
        for (volume, expected) in [
            (0, "none"),
            (1, "low"),
            (999, "low"),
            (1_000, "medium"),
            (99_999, "medium"),
            (100_000, "high"),
        ] {
            assert_eq!(
                input_for(0, volume, 0)["impact"]["evaluation_volume"],
                expected,
                "{volume} evaluations"
            );
        }
    }

    #[test]
    fn dependent_flag_buckets() {
        for (deps, expected) in [(0, "none"), (1, "some"), (3, "some"), (4, "many")] {
            assert_eq!(
                input_for(0, 0, deps)["impact"]["dependent_flags"],
                expected,
                "{deps} dependents"
            );
        }
    }

    #[test]
    fn diff_keeps_first_30_entries_and_truncates_values() {
        let long = "x".repeat(500);
        let entries: Vec<Value> = (0..45)
            .map(|index| {
                json!({ "path": format!("p{index}"), "change_type": "changed",
                        "before": long, "after": { "nested": long } })
            })
            .collect();
        let input = build_input(
            &feature(),
            &stage(),
            &environment(),
            &payload(blast(0, 0, 0), Value::Array(entries)),
        );

        let diff = input["change"]["diff"].as_array().unwrap();
        assert_eq!(diff.len(), MAX_DIFF_ENTRIES);
        assert_eq!(diff[0]["path"], "p0");
        assert_eq!(diff[29]["path"], "p29");
        assert_eq!(diff[0]["before"].as_str().unwrap().chars().count(), 200);
        assert_eq!(diff[0]["after"].as_str().unwrap().chars().count(), 200);
    }

    #[test]
    fn diff_truncation_counts_characters_not_bytes() {
        let long = "é".repeat(300);
        let input = build_input(
            &feature(),
            &stage(),
            &environment(),
            &payload(
                blast(0, 0, 0),
                json!([{ "path": "p", "change_type": "added", "before": null, "after": long }]),
            ),
        );
        let entry = &input["change"]["diff"][0];
        assert_eq!(entry["before"], Value::Null);
        assert_eq!(entry["after"].as_str().unwrap().chars().count(), 200);
    }

    #[test]
    fn input_never_contains_owner_or_user_data() {
        let mut payload = payload(blast(1, 1, 1), json!([]));
        payload["requested_by"] = json!("user-id-123");
        payload["routing"] = json!({ "eligible_approver_ids": ["u1"] });
        let input = build_input(&feature(), &stage(), &environment(), &payload);
        let text = input.to_string();

        assert!(!text.contains("alice@example.com"));
        assert!(!text.contains("owner"));
        assert!(!text.contains("user-id-123"));
        assert!(!text.contains("eligible_approver_ids"));
    }

    #[test]
    fn an_unclassified_feature_sends_a_null_flag_kind() {
        let input = input_for(0, 0, 0);
        assert_eq!(input["feature"]["flag_kind"], Value::Null);
    }

    #[test]
    fn build_input_tolerates_a_bare_payload() {
        let input = build_input(&feature(), &stage(), &environment(), &json!({}));

        assert_eq!(input["change"]["from_status"], "NOT_DEPLOYED");
        assert_eq!(input["change"]["diff"], json!([]));
        assert_eq!(input["impact"]["client_count"], "none");
        assert_eq!(input["impact"]["risk_level"], Value::Null);
        assert_eq!(
            input["impact"]["affected_environment_types"],
            json!(["production"])
        );
    }

    fn handler() -> ApprovalRiskHandler {
        ApprovalRiskHandler::new(
            Box::new(MockActivityLogRepository::new()),
            Box::new(MockApprovalRepository::new()),
        )
    }

    fn answers(widens: f64, safety: f64, sensitive: f64, risk: f64, confidence: f64) -> Answers {
        Answers(BTreeMap::from([
            (
                "widens_prod_exposure".to_string(),
                Answer::Noul { noul: widens },
            ),
            (
                "disables_safety_control".to_string(),
                Answer::Noul { noul: safety },
            ),
            (
                "touches_sensitive_domain".to_string(),
                Answer::Noul { noul: sensitive },
            ),
            (
                "overall_risk".to_string(),
                Answer::Score(ScoreAnswer {
                    score: risk,
                    legend: BTreeMap::new(),
                    probabilities: BTreeMap::new(),
                    confidence,
                }),
            ),
        ]))
    }

    fn derived(answers: &Answers) -> Value {
        handler().derive(&json!({}), answers)
    }

    #[test]
    fn derive_low_baseline() {
        let result = derived(&answers(0.1, 0.0, 0.2, 0.4, 0.9));

        assert_eq!(result["level"], "low");
        assert_eq!(result["reasons"], json!([]));
        assert_eq!(
            result["signals"],
            json!({
                "widens_prod_exposure": 0.1,
                "disables_safety_control": 0.0,
                "touches_sensitive_domain": 0.2,
                "overall_risk": 0.4,
                "overall_risk_confidence": 0.9,
            })
        );
    }

    #[test]
    fn derive_high_triggers_each_alone() {
        let cases = [
            (
                answers(0.0, 0.0, 0.0, 2.2, 0.9),
                "High potential user impact",
            ),
            (answers(0.0, 0.7, 0.0, 0.0, 0.9), "Weakens a safety control"),
        ];
        for (answers, reason) in cases {
            let result = derived(&answers);
            assert_eq!(result["level"], "high", "{reason}");
            assert!(
                result["reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(reason))
            );
        }
        // Just under each threshold is not high.
        assert_ne!(derived(&answers(0.0, 0.0, 0.0, 2.19, 0.9))["level"], "high");
        assert_ne!(derived(&answers(0.0, 0.69, 0.0, 0.0, 0.9))["level"], "high");
    }

    #[test]
    fn derive_high_for_widening_a_sensitive_area() {
        let result = derived(&answers(0.7, 0.0, 0.6, 0.0, 0.9));

        assert_eq!(result["level"], "high");
        assert_eq!(
            result["reasons"],
            json!([
                "Widens production exposure",
                "Sensitive area (payments, auth, security, or personal data)"
            ])
        );
        // Each half alone is only medium.
        assert_eq!(
            derived(&answers(0.7, 0.0, 0.59, 0.0, 0.9))["level"],
            "medium"
        );
        assert_eq!(derived(&answers(0.69, 0.0, 0.6, 0.0, 0.9))["level"], "low");
    }

    #[test]
    fn derive_medium_triggers_each_alone() {
        let cases = [
            (
                answers(0.0, 0.0, 0.0, 1.3, 0.9),
                "High potential user impact",
            ),
            (
                answers(0.7, 0.0, 0.0, 0.0, 0.9),
                "Widens production exposure",
            ),
            (
                answers(0.0, 0.0, 0.7, 0.0, 0.9),
                "Sensitive area (payments, auth, security, or personal data)",
            ),
        ];
        for (answers, reason) in cases {
            let result = derived(&answers);
            assert_eq!(result["level"], "medium", "{reason}");
            assert_eq!(result["reasons"], json!([reason]));
        }
        assert_eq!(derived(&answers(0.0, 0.0, 0.0, 1.29, 0.9))["level"], "low");
        assert_eq!(derived(&answers(0.69, 0.0, 0.0, 0.0, 0.9))["level"], "low");
        assert_eq!(derived(&answers(0.0, 0.0, 0.69, 0.0, 0.9))["level"], "low");
    }

    #[test]
    fn derive_low_confidence_forces_at_least_medium() {
        let result = derived(&answers(0.0, 0.0, 0.0, 0.0, 0.29));
        assert_eq!(result["level"], "medium");
        assert_eq!(result["reasons"], json!(["Assessment uncertain"]));
        assert_eq!(derived(&answers(0.0, 0.0, 0.0, 0.0, 0.3))["level"], "low");

        // Low confidence never lowers a high result.
        let high = derived(&answers(0.0, 0.9, 0.0, 0.0, 0.1));
        assert_eq!(high["level"], "high");
        assert_eq!(
            high["reasons"],
            json!(["Weakens a safety control", "Assessment uncertain"])
        );
    }

    #[test]
    fn derive_missing_answers_count_as_uncertain() {
        let result = derived(&Answers::default());

        assert_eq!(result["level"], "medium");
        assert_eq!(result["reasons"], json!(["Assessment uncertain"]));
        assert_eq!(result["signals"]["overall_risk_confidence"], 0.0);
    }

    #[test]
    fn derive_lists_every_crossed_signal_in_order() {
        let result = derived(&answers(0.9, 0.9, 0.9, 2.8, 0.1));

        assert_eq!(result["level"], "high");
        assert_eq!(
            result["reasons"],
            json!([
                "Widens production exposure",
                "Weakens a safety control",
                "Sensitive area (payments, auth, security, or personal data)",
                "High potential user impact",
                "Assessment uncertain"
            ])
        );
    }

    #[test]
    fn build_asks_the_four_designed_questions() {
        let input = input_for(1, 1, 0);
        let parts = handler().build(&input);

        assert_eq!(parts.state, input);
        let ids: Vec<&str> = parts.questions.keys().map(String::as_str).collect();
        assert_eq!(
            ids,
            [
                "disables_safety_control",
                "overall_risk",
                "touches_sensitive_domain",
                "widens_prod_exposure"
            ]
        );
        match &parts.questions["overall_risk"] {
            Question::Score { criteria, .. } => assert_eq!(criteria.len(), 4),
            other => panic!("expected a score question, got {other:?}"),
        }
        assert!(matches!(
            parts.questions["widens_prod_exposure"],
            Question::Noul {
                criteria: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn build_serializes_to_the_wire_snapshot() {
        let parts = handler().build(&json!({ "feature": { "key": "k" } }));
        let wire = serde_json::to_value(&parts.questions).unwrap();

        assert_eq!(
            wire["widens_prod_exposure"]["instructions"],
            "Does `change` make `feature` reach more end users in a production environment than before? Count it as yes when targeting rules are removed or loosened, a rollout percentage goes up, or the feature is turned on in production."
        );
        assert_eq!(
            wire["widens_prod_exposure"]["criteria"],
            json!({
                "true": "More production end users get the feature after this change.",
                "false": "Production exposure stays the same or goes down, or the change is not in production."
            })
        );
        assert_eq!(
            wire["disables_safety_control"]["instructions"],
            "Does `change` turn off, bypass, or weaken a safety control, such as a kill switch, circuit breaker, rate limit, fraud check, or permission check?"
        );
        assert_eq!(
            wire["disables_safety_control"]["criteria"],
            json!({
                "true": "A safety control is turned off, bypassed, or weakened.",
                "false": "No safety control is weakened."
            })
        );
        assert_eq!(
            wire["touches_sensitive_domain"]["instructions"],
            "Does `feature` control behavior in a sensitive area: payments or billing, authentication or authorization, security, personal data or privacy, or data deletion?"
        );
        assert_eq!(
            wire["touches_sensitive_domain"]["criteria"],
            json!({
                "true": "The feature controls one of these sensitive areas.",
                "false": "The feature controls none of these areas."
            })
        );
        assert_eq!(
            wire["overall_risk"],
            json!({
                "type": "score",
                "instructions": "If this change turns out to be wrong, how serious is the harm to end users? Use `change` and `impact`.",
                "criteria": [
                    "No end-user impact: internal tooling, a development environment only, or a cosmetic change.",
                    "Limited impact: a small or opt-in audience, or a problem that is easy to notice and revert.",
                    "Broad impact: many end users in production see changed behavior.",
                    "Critical impact: money, access, security, or data integrity for many users could be affected."
                ]
            })
        );
    }

    fn judgment(request_id: Uuid, derived: Option<Value>) -> AiJudgment {
        AiJudgment {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            subject_type: "approval_request".into(),
            subject_id: request_id,
            kind: "approval_risk".into(),
            status: "done".into(),
            attempts: 1,
            input: json!({}),
            input_hash: "h".into(),
            model: Some("jev-1.13.0".into()),
            raw_answers: None,
            derived,
            input_tokens: Some(10),
            error: None,
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
        }
    }

    fn request(id: Uuid, feature_id: Uuid) -> ApprovalRequest {
        ApprovalRequest {
            id,
            policy_id: Uuid::new_v4(),
            feature_id,
            environment_id: None,
            change_type: "stage_change".into(),
            change_payload: json!({}),
            change_description: None,
            requested_by: Uuid::new_v4(),
            eligible_approver_ids: vec![],
            routing_reason: None,
            admin_override_enabled: false,
            status: ApprovalStatus::Pending,
            approved_count: 0,
            rejected_count: 0,
            executed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            required_approvers_override: None,
            external_ref: None,
            request_reason: None,
            approval_source: "fluxgate".to_string(),
            external_approver: None,
        }
    }

    #[tokio::test]
    async fn apply_writes_the_approval_risk_assessed_activity() {
        let request_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let mut approvals = MockApprovalRepository::new();
        approvals
            .expect_get_request_by_id()
            .withf(move |id| *id == request_id)
            .times(1)
            .returning(move |id| Ok(Some(request(id, feature_id))));
        approvals
            .expect_get_policy_by_id()
            .returning(|_| Ok(Some(policy("advisory", 1))));
        let mut activity = MockActivityLogRepository::new();
        activity
            .expect_create_activity()
            .withf(move |entry| {
                entry.activity_type == "approval_risk_assessed"
                    && entry.entity_id == feature_id.to_string()
                    && entry.actor_id.is_none()
                    && entry.metadata
                        == Some(json!({
                            "approval_request_id": request_id.to_string(),
                            "feature_id": feature_id.to_string(),
                            "level": "high",
                            "reasons": ["Weakens a safety control"],
                            "model": "jev-1.13.0",
                        }))
            })
            .times(1)
            .returning(|entry| {
                Ok(ActivityLogRow {
                    id: Uuid::new_v4(),
                    activity_type: entry.activity_type,
                    entity_type: entry.entity_type,
                    entity_id: entry.entity_id,
                    actor_id: entry.actor_id,
                    actor_name: entry.actor_name,
                    description: entry.description,
                    metadata: entry.metadata,
                    created_at: Utc::now(),
                })
            });
        let handler = ApprovalRiskHandler::new(Box::new(activity), Box::new(approvals));
        let derived =
            json!({ "level": "high", "reasons": ["Weakens a safety control"], "signals": {} });

        handler
            .apply(&judgment(request_id, Some(derived)))
            .await
            .unwrap();
    }

    fn policy(mode: &str, required_approvers: i32) -> ApprovalPolicy {
        ApprovalPolicy {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            name: "Production approval".into(),
            description: None,
            applies_to: "all".into(),
            environment_ids: None,
            required_approvers,
            approver_role_ids: vec![],
            approver_user_ids: vec![],
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: true,
            created_at: Utc::now(),
            ai_risk_mode: mode.into(),
        }
    }

    fn activity_row(entry: CreateActivityLog) -> ActivityLogRow {
        ActivityLogRow {
            id: Uuid::new_v4(),
            activity_type: entry.activity_type,
            entity_type: entry.entity_type,
            entity_id: entry.entity_id,
            actor_id: entry.actor_id,
            actor_name: entry.actor_name,
            description: entry.description,
            metadata: entry.metadata,
            created_at: Utc::now(),
        }
    }

    /// Runs `apply` for `level` against a request in `status` under a policy
    /// with `mode` and 2 required approvers. `override_calls` is how often the
    /// override update is expected. Returns the activity metadata written.
    async fn apply_enforcement(
        mode: &str,
        level: &str,
        status: ApprovalStatus,
        existing_override: Option<i32>,
        override_calls: usize,
        update_changes_row: bool,
    ) -> Value {
        apply_enforcement_with_eligible(
            mode,
            level,
            status,
            existing_override,
            0,
            3,
            override_calls,
            update_changes_row,
        )
        .await
    }

    /// [`apply_enforcement`] for a request with `eligible` approvers in its
    /// eligible list (0 = empty list: no pool at creation, or a legacy row).
    /// `expected_override` is the value the override update must be called with.
    #[allow(clippy::too_many_arguments)]
    async fn apply_enforcement_with_eligible(
        mode: &str,
        level: &str,
        status: ApprovalStatus,
        existing_override: Option<i32>,
        eligible: usize,
        expected_override: i32,
        override_calls: usize,
        update_changes_row: bool,
    ) -> Value {
        let request_id = Uuid::new_v4();
        let feature_id = Uuid::new_v4();
        let mut stored = request(request_id, feature_id);
        stored.status = status;
        stored.required_approvers_override = existing_override;
        stored.eligible_approver_ids = (0..eligible).map(|_| Uuid::new_v4()).collect();
        let policy = policy(mode, 2);

        let mut approvals = MockApprovalRepository::new();
        approvals
            .expect_get_request_by_id()
            .returning(move |_| Ok(Some(stored.clone())));
        approvals
            .expect_get_policy_by_id()
            .returning(move |_| Ok(Some(policy.clone())));
        approvals
            .expect_set_required_approvers_override()
            .withf(move |id, required| *id == request_id && *required == expected_override)
            .times(override_calls)
            .returning(move |_, _| Ok(update_changes_row));

        let written = std::sync::Arc::new(std::sync::Mutex::new(Value::Null));
        let sink = written.clone();
        let mut activity = MockActivityLogRepository::new();
        activity
            .expect_create_activity()
            .times(1)
            .returning(move |entry| {
                *sink.lock().unwrap() = entry.metadata.clone().unwrap();
                Ok(activity_row(entry))
            });

        let handler = ApprovalRiskHandler::new(Box::new(activity), Box::new(approvals));
        let derived = json!({ "level": level, "reasons": [], "signals": {} });
        handler
            .apply(&judgment(request_id, Some(derived)))
            .await
            .unwrap();
        written.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn high_risk_under_require_extra_approver_raises_the_requirement() {
        let metadata = apply_enforcement(
            "require_extra_approver",
            "high",
            ApprovalStatus::Pending,
            None,
            1,
            true,
        )
        .await;
        assert_eq!(metadata["required_approvers_override"], 3);
    }

    #[tokio::test]
    async fn the_extra_approver_is_capped_at_the_eligible_approver_count() {
        // Policy needs 2. (eligible approvers, override written)
        for (eligible, expected) in [(5, 3), (3, 3)] {
            let metadata = apply_enforcement_with_eligible(
                "require_extra_approver",
                "high",
                ApprovalStatus::Pending,
                None,
                eligible,
                expected,
                1,
                true,
            )
            .await;
            assert_eq!(
                metadata["required_approvers_override"], expected,
                "{eligible} eligible"
            );
            assert!(metadata.get("extra_approver_skipped").is_none());
        }
    }

    #[tokio::test]
    async fn no_override_when_no_additional_approver_is_eligible() {
        // 2 eligible for a policy that needs 2: the cap leaves the requirement
        // unchanged. 1 eligible: the cap must never lower it either.
        for eligible in [2, 1] {
            let metadata = apply_enforcement_with_eligible(
                "require_extra_approver",
                "high",
                ApprovalStatus::Pending,
                None,
                eligible,
                0,
                0,
                false,
            )
            .await;
            assert!(
                metadata.get("required_approvers_override").is_none(),
                "{eligible} eligible"
            );
            assert_eq!(
                metadata["extra_approver_skipped"], "no_additional_eligible_approver",
                "{eligible} eligible"
            );
        }
    }

    #[tokio::test]
    async fn a_request_without_an_eligible_list_gets_required_plus_one() {
        let metadata = apply_enforcement_with_eligible(
            "require_extra_approver",
            "high",
            ApprovalStatus::Pending,
            None,
            0,
            3,
            1,
            true,
        )
        .await;
        assert_eq!(metadata["required_approvers_override"], 3);
        assert!(metadata.get("extra_approver_skipped").is_none());
    }

    #[tokio::test]
    async fn other_modes_and_levels_leave_the_requirement_alone() {
        for (mode, level) in [
            ("advisory", "high"),
            ("gate_auto_approve", "high"),
            ("off", "high"),
            ("require_extra_approver", "medium"),
            ("require_extra_approver", "low"),
        ] {
            let metadata =
                apply_enforcement(mode, level, ApprovalStatus::Pending, None, 0, false).await;
            assert!(
                metadata.get("required_approvers_override").is_none(),
                "{mode} {level}"
            );
        }
    }

    #[tokio::test]
    async fn a_closed_request_is_never_given_an_override() {
        for status in [
            ApprovalStatus::Approved,
            ApprovalStatus::Rejected,
            ApprovalStatus::Cancelled,
        ] {
            let metadata =
                apply_enforcement("require_extra_approver", "high", status, None, 0, false).await;
            assert!(metadata.get("required_approvers_override").is_none());
        }
    }

    #[tokio::test]
    async fn a_second_apply_keeps_the_first_override() {
        let metadata = apply_enforcement(
            "require_extra_approver",
            "high",
            ApprovalStatus::Pending,
            Some(3),
            0,
            false,
        )
        .await;
        assert_eq!(metadata["required_approvers_override"], 3);
    }

    #[tokio::test]
    async fn a_lost_update_race_records_no_override() {
        let metadata = apply_enforcement(
            "require_extra_approver",
            "high",
            ApprovalStatus::Pending,
            None,
            1,
            false,
        )
        .await;
        assert!(metadata.get("required_approvers_override").is_none());
    }

    #[tokio::test]
    async fn apply_skips_a_deleted_request() {
        let mut approvals = MockApprovalRepository::new();
        approvals
            .expect_get_request_by_id()
            .times(1)
            .returning(|_| Ok(None));
        let mut activity = MockActivityLogRepository::new();
        activity.expect_create_activity().times(0);
        let handler = ApprovalRiskHandler::new(Box::new(activity), Box::new(approvals));
        let derived = json!({ "level": "low", "reasons": [], "signals": {} });

        let outcome = handler
            .apply(&judgment(Uuid::new_v4(), Some(derived)))
            .await;
        assert!(outcome.is_ok());
    }
}
