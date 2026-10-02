//! Justification check (AI-20): tells users when a free-text reason is vague.
//! Warn only; see `docs/ai-judgments/design.md` §5.2.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use log::warn;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use super::service::{JudgmentHandler, JudgmentService};
use super::types::{Answers, Question, RequestParts};
use super::{JudgmentKind, SubjectType};
use crate::database::activity_log::ActivityLogRepository;
use crate::database::ai::AiJudgment;

/// `placeholder_text` at or above this makes the verdict `weak`.
pub const PLACEHOLDER_WEAK_AT: f64 = 0.6;
/// `concrete_cause` below this makes the verdict `weak`.
pub const CONCRETE_WEAK_BELOW: f64 = 0.4;
/// Neutral value for a missing `concrete_cause` answer: crosses no threshold, so
/// an incomplete answer set never produces a warning.
const MISSING_CONCRETE: f64 = 0.5;
/// Longest reason sent to Jev. The REST pre-check rejects longer reasons.
pub const MAX_REASON_CHARS: usize = 1000;
/// Key under which the verdict is merged into the activity metadata.
pub const ACTIVITY_METADATA_KEY: &str = "ai_justification";

const Q_CONCRETE_CAUSE: &str = "concrete_cause";
const Q_PLACEHOLDER_TEXT: &str = "placeholder_text";

const HINT_PLACEHOLDER: &str = "This looks like placeholder text.";
const HINT_CONCRETE: &str = "Add the cause: an incident, ticket, bug, or customer impact.";

const FALLBACK_ACTION: &str = "Change a feature flag";

/// Where a free-text reason is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasonKind {
    EmergencyDisable,
    EmergencyEnable,
    FreezeOverride,
    ScheduledChange,
    ArchiveCleanup,
    FreezeWindow,
}

impl ReasonKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasonKind::EmergencyDisable => "emergency_disable",
            ReasonKind::EmergencyEnable => "emergency_enable",
            ReasonKind::FreezeOverride => "freeze_override",
            ReasonKind::ScheduledChange => "scheduled_change",
            ReasonKind::ArchiveCleanup => "archive_cleanup",
            ReasonKind::FreezeWindow => "freeze_window",
        }
    }
}

/// Fixed text describing what the reason is for. Lives in code, never user input.
pub fn action_description(kind: ReasonKind) -> &'static str {
    match kind {
        ReasonKind::EmergencyDisable => "Turn off a feature flag in an emergency (kill switch)",
        ReasonKind::EmergencyEnable => "Turn a feature flag back on after an emergency kill switch",
        ReasonKind::FreezeOverride => "Override an active change freeze to change a feature flag",
        ReasonKind::ScheduledChange => "Schedule a future change to a feature flag",
        ReasonKind::ArchiveCleanup => {
            "Archive a feature flag that is no longer needed, and describe its cleanup"
        }
        ReasonKind::FreezeWindow => {
            "Create or change a change-freeze window that blocks feature flag changes"
        }
    }
}

static TICKET_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]+-\d+\b").expect("valid ticket regex"));
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").expect("valid regex"));
static ISSUE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#\d+").expect("valid regex"));
static INCIDENT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bINC\d+\b").expect("valid regex"));

/// True when the reason names a ticket key, URL, issue reference, or incident id.
/// Such a reason is accepted without asking the model.
pub fn rule_check(reason: &str) -> bool {
    TICKET_KEY.is_match(reason)
        || URL.is_match(reason)
        || ISSUE_REF.is_match(reason)
        || INCIDENT_ID.is_match(reason)
}

/// The input snapshot stored with the judgment and sent (in part) to Jev.
pub fn build_input(kind: ReasonKind, reason: &str, feature_key: Option<&str>) -> Value {
    json!({
        "reason_kind": kind.as_str(),
        "reason": truncate_chars(reason.trim(), MAX_REASON_CHARS),
        "feature_key": feature_key,
    })
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn questions() -> BTreeMap<String, Question> {
    let concrete = Question::noul_with(
        "Does `reason` state a specific cause or purpose for `action`, such as an incident, a bug, a ticket, customer impact, a release plan, or a deadline?",
        "The reason names a specific cause or purpose that another engineer could check or act on.",
        "The reason is generic, contains no facts, or only repeats the action.",
    );
    let placeholder = Question::noul_with(
        "Is `reason` placeholder or filler text with no information, such as \"test\", \"asdf\", \"n/a\", \"urgent\", \"fix\", or \"as requested\"?",
        "The reason is placeholder or filler text.",
        "The reason contains real information.",
    );
    BTreeMap::from([
        (Q_CONCRETE_CAUSE.to_string(), concrete),
        (Q_PLACEHOLDER_TEXT.to_string(), placeholder),
    ])
}

/// State and questions for one reason (design §5.2).
pub fn build(kind: ReasonKind, feature_key: Option<&str>, reason: &str) -> RequestParts {
    parts(action_description(kind), feature_key, reason)
}

fn parts(action: &str, feature_key: Option<&str>, reason: &str) -> RequestParts {
    RequestParts {
        state: json!({
            "action": action,
            "feature_key": feature_key,
            "reason": reason,
        }),
        questions: questions(),
    }
}

/// Verdict, probability, and hints from the two answers.
/// `probability` is `concrete_cause`: how likely the reason names a real cause.
pub fn derive(answers: &Answers) -> Value {
    let placeholder = answers.noul(Q_PLACEHOLDER_TEXT).unwrap_or(0.0);
    let concrete = answers.noul(Q_CONCRETE_CAUSE).unwrap_or(MISSING_CONCRETE);

    let placeholder_fired = placeholder >= PLACEHOLDER_WEAK_AT;
    let concrete_fired = concrete < CONCRETE_WEAK_BELOW;
    let hints: Vec<&str> = [
        (placeholder_fired, HINT_PLACEHOLDER),
        (concrete_fired, HINT_CONCRETE),
    ]
    .into_iter()
    .filter_map(|(fired, hint)| fired.then_some(hint))
    .collect();

    json!({
        "verdict": if placeholder_fired || concrete_fired { "weak" } else { "ok" },
        "probability": concrete,
        "hints": hints,
        "source": "model",
    })
}

/// What a rule pass stores in the judgment row. No API call produced it.
pub fn rule_derived() -> Value {
    json!({ "verdict": "ok", "source": "rule" })
}

pub struct JustificationHandler {
    activity: Box<dyn ActivityLogRepository>,
}

impl JustificationHandler {
    pub fn new(activity: Box<dyn ActivityLogRepository>) -> Self {
        Self { activity }
    }
}

#[async_trait]
impl JudgmentHandler for JustificationHandler {
    fn kind(&self) -> JudgmentKind {
        JudgmentKind::Justification
    }

    fn build(&self, input: &Value) -> RequestParts {
        let action = input
            .get("reason_kind")
            .cloned()
            .and_then(|kind| serde_json::from_value::<ReasonKind>(kind).ok())
            .map(action_description)
            .unwrap_or(FALLBACK_ACTION);
        parts(
            action,
            input.get("feature_key").and_then(Value::as_str),
            input.get("reason").and_then(Value::as_str).unwrap_or(""),
        )
    }

    fn derive(&self, _input: &Value, answers: &Answers) -> Value {
        derive(answers)
    }

    /// Merges the verdict into the activity row's metadata. Other subject
    /// types have no place to show it. A missing row updates nothing.
    async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error> {
        if judgment.subject_type != SubjectType::Activity.as_str() {
            return Ok(());
        }
        let derived = judgment.derived.as_ref().unwrap_or(&Value::Null);
        let rule = derived.get("source").and_then(Value::as_str) == Some("rule");
        let probability = derived
            .get("probability")
            .cloned()
            .unwrap_or_else(|| if rule { json!(1.0) } else { Value::Null });
        let value = json!({
            "verdict": derived.get("verdict").cloned().unwrap_or(Value::Null),
            "probability": probability,
            "model": judgment.model,
        });
        self.activity
            .merge_activity_metadata(judgment.subject_id, ACTIVITY_METADATA_KEY, value)
            .await
            .map_err(crate::Error::DatabaseError)
    }
}

/// Records a free-text reason after the user's change has committed. Skips when
/// the subsystem is off, the team toggle is off, or the reason is blank. A rule
/// pass is stored as done without an API call. Never fails the caller: errors
/// are logged and dropped.
pub async fn record_justification(
    service: Option<&Arc<JudgmentService>>,
    team_id: Uuid,
    subject_type: SubjectType,
    subject_id: Uuid,
    kind: ReasonKind,
    reason: &str,
    feature_key: Option<&str>,
) {
    let Some(service) = service else {
        return;
    };
    if reason.trim().is_empty()
        || !service
            .team_enabled(team_id, JudgmentKind::Justification.feature())
            .await
    {
        return;
    }
    let input = build_input(kind, reason, feature_key);
    let outcome = if rule_check(reason) {
        service
            .record_rule_result(
                team_id,
                JudgmentKind::Justification,
                subject_type,
                subject_id,
                input,
                rule_derived(),
            )
            .await
            .map(|_| ())
    } else {
        service
            .submit(
                team_id,
                JudgmentKind::Justification,
                subject_type,
                subject_id,
                input,
            )
            .await
            .map(|_| ())
    };
    if let Err(err) = outcome {
        warn!(
            "Could not record justification check for {} {subject_id}: {err}",
            subject_type.as_str()
        );
    }
}

/// Shared by REST handler tests: an `AiRuntime` whose judgment store records
/// every submission instead of calling anything.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use serde_json::Value;
    use uuid::Uuid;

    use super::*;
    use crate::database::activity_log::MockActivityLogRepository;
    use crate::database::ai::{
        MockAiJudgmentRepository, MockTeamAiSettingsRepository, StoredTeamAiSettings,
        TeamAiSettings,
    };
    use crate::judgment::client::{JudgmentError, MockJudgmentClient};
    use crate::judgment::{AiRuntime, JudgmentClient};

    /// (subject type, subject id, stored input) per upsert, in call order.
    pub(crate) type Recorded = Arc<Mutex<Vec<(SubjectType, Uuid, Value)>>>;

    /// `on` is the team toggle. With `fail_store` every upsert errors, after
    /// being recorded. The API client always times out, so a submitted
    /// judgment ends as failed and nothing needs a real model.
    pub(crate) fn recording_runtime(on: bool, fail_store: bool, recorded: Recorded) -> AiRuntime {
        let mut settings = MockTeamAiSettingsRepository::new();
        settings.expect_get().returning(move |_| {
            Ok(StoredTeamAiSettings {
                settings: TeamAiSettings {
                    justification_check: on,
                    ..TeamAiSettings::default()
                },
                ..StoredTeamAiSettings::default()
            })
        });
        let mut judgments = MockAiJudgmentRepository::new();
        judgments.expect_upsert_pending().returning(move |new| {
            recorded
                .lock()
                .unwrap()
                .push((new.subject_type, new.subject_id, new.input.clone()));
            if fail_store {
                return Err(crate::Error::InvalidInput("db down".into()));
            }
            Ok(AiJudgment {
                id: Uuid::new_v4(),
                team_id: new.team_id,
                subject_type: new.subject_type.as_str().to_string(),
                subject_id: new.subject_id,
                kind: new.kind.as_str().to_string(),
                status: "pending".to_string(),
                attempts: 1,
                input: new.input,
                input_hash: new.input_hash,
                model: None,
                raw_answers: None,
                derived: None,
                input_tokens: None,
                error: None,
                created_at: Utc::now(),
                completed_at: None,
            })
        });
        judgments.expect_mark_failed().returning(|_, _, _| Ok(true));
        judgments.expect_mark_done().returning(|_, _, _| Ok(true));
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .returning(|_, _| Err(JudgmentError::Timeout));
        let client: Arc<dyn JudgmentClient> = Arc::new(client);
        let service = Arc::new(
            JudgmentService::new(client.clone(), Box::new(judgments), Box::new(settings))
                .with_handler(Arc::new(JustificationHandler::new(Box::new(
                    MockActivityLogRepository::new(),
                )))),
        );
        AiRuntime::new(Some(client), "jev-1.13.0").with_judgments(Some(service))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judgment::types::Answer;

    fn answers(concrete: f64, placeholder: f64) -> Answers {
        Answers(BTreeMap::from([
            (
                Q_CONCRETE_CAUSE.to_string(),
                Answer::Noul { noul: concrete },
            ),
            (
                Q_PLACEHOLDER_TEXT.to_string(),
                Answer::Noul { noul: placeholder },
            ),
        ]))
    }

    #[test]
    fn rule_check_accepts_ticket_url_issue_and_incident() {
        for reason in [
            "JIRA-123",
            "see https://x.y",
            "fixes #42",
            "INC12345",
            "rollback per OPS2-9 please",
        ] {
            assert!(rule_check(reason), "{reason} should pass the rule check");
        }
    }

    #[test]
    fn rule_check_rejects_vague_reasons() {
        for reason in [
            "urgent", "fix prod", "test", "jira-123", "INC", "# 42", "A-1",
        ] {
            assert!(!rule_check(reason), "{reason} should not pass");
        }
    }

    #[test]
    fn placeholder_high_is_weak_with_placeholder_hint() {
        let derived = derive(&answers(0.9, 0.8));
        assert_eq!(derived["verdict"], "weak");
        assert_eq!(derived["hints"], json!([HINT_PLACEHOLDER]));
        assert_eq!(derived["source"], "model");
    }

    #[test]
    fn concrete_low_is_weak_with_cause_hint() {
        let derived = derive(&answers(0.2, 0.1));
        assert_eq!(derived["verdict"], "weak");
        assert_eq!(derived["probability"], 0.2);
        assert_eq!(derived["hints"], json!([HINT_CONCRETE]));
    }

    #[test]
    fn both_signals_give_both_hints_in_order() {
        let derived = derive(&answers(0.1, 0.9));
        assert_eq!(derived["hints"], json!([HINT_PLACEHOLDER, HINT_CONCRETE]));
    }

    #[test]
    fn both_fine_is_ok_with_no_hints() {
        let derived = derive(&answers(0.9, 0.1));
        assert_eq!(derived["verdict"], "ok");
        assert_eq!(derived["probability"], 0.9);
        assert_eq!(derived["hints"], json!([]));
    }

    #[test]
    fn thresholds_are_inclusive_on_placeholder_and_exclusive_on_concrete() {
        assert_eq!(
            derive(&answers(0.9, PLACEHOLDER_WEAK_AT))["verdict"],
            "weak"
        );
        assert_eq!(derive(&answers(CONCRETE_WEAK_BELOW, 0.0))["verdict"], "ok");
    }

    #[test]
    fn missing_answers_never_warn() {
        let derived = derive(&Answers::default());
        assert_eq!(derived["verdict"], "ok");
    }

    #[test]
    fn every_reason_kind_has_its_own_description() {
        let kinds = [
            ReasonKind::EmergencyDisable,
            ReasonKind::EmergencyEnable,
            ReasonKind::FreezeOverride,
            ReasonKind::ScheduledChange,
            ReasonKind::ArchiveCleanup,
            ReasonKind::FreezeWindow,
        ];
        let texts: std::collections::BTreeSet<_> =
            kinds.iter().map(|kind| action_description(*kind)).collect();
        assert_eq!(texts.len(), kinds.len());
        assert_eq!(
            action_description(ReasonKind::EmergencyDisable),
            "Turn off a feature flag in an emergency (kill switch)"
        );
        for kind in kinds {
            let wire = serde_json::to_value(kind).unwrap();
            assert_eq!(wire, json!(kind.as_str()));
        }
    }

    /// The wording below is the contract with Jev (design §5.2). A change here
    /// changes every verdict, so it must be deliberate.
    #[test]
    fn request_wire_snapshot() {
        let parts = build(ReasonKind::EmergencyDisable, Some("checkout-v2"), "urgent");
        assert_eq!(
            parts.state,
            json!({
                "action": "Turn off a feature flag in an emergency (kill switch)",
                "feature_key": "checkout-v2",
                "reason": "urgent",
            })
        );
        assert_eq!(
            serde_json::to_value(&parts.questions).unwrap(),
            json!({
                "concrete_cause": {
                    "type": "noul",
                    "instructions": "Does `reason` state a specific cause or purpose for `action`, such as an incident, a bug, a ticket, customer impact, a release plan, or a deadline?",
                    "criteria": {
                        "true": "The reason names a specific cause or purpose that another engineer could check or act on.",
                        "false": "The reason is generic, contains no facts, or only repeats the action.",
                    },
                },
                "placeholder_text": {
                    "type": "noul",
                    "instructions": "Is `reason` placeholder or filler text with no information, such as \"test\", \"asdf\", \"n/a\", \"urgent\", \"fix\", or \"as requested\"?",
                    "criteria": {
                        "true": "The reason is placeholder or filler text.",
                        "false": "The reason contains real information.",
                    },
                },
            })
        );
    }

    mod recording {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        use chrono::Utc;
        use tokio::sync::Notify;

        use super::*;
        use crate::database::activity_log::MockActivityLogRepository;
        use crate::database::ai::{
            AiJudgment, MockAiJudgmentRepository, MockTeamAiSettingsRepository,
            StoredTeamAiSettings, TeamAiSettings,
        };
        use crate::judgment::client::{JudgmentError, MockJudgmentClient};
        use crate::judgment::service::RULE_MODEL;
        use crate::judgment::types::{SystemOneResponse, Usage};

        fn settings(on: bool) -> MockTeamAiSettingsRepository {
            let mut settings = MockTeamAiSettingsRepository::new();
            settings.expect_get().returning(move |_| {
                Ok(StoredTeamAiSettings {
                    settings: TeamAiSettings {
                        justification_check: on,
                        ..TeamAiSettings::default()
                    },
                    ..StoredTeamAiSettings::default()
                })
            });
            settings
        }

        fn pending_row(new: &crate::database::ai::NewJudgment) -> AiJudgment {
            AiJudgment {
                id: Uuid::new_v4(),
                team_id: new.team_id,
                subject_type: new.subject_type.as_str().to_string(),
                subject_id: new.subject_id,
                kind: new.kind.as_str().to_string(),
                status: "pending".to_string(),
                attempts: 1,
                input: new.input.clone(),
                input_hash: new.input_hash.clone(),
                model: None,
                raw_answers: None,
                derived: None,
                input_tokens: None,
                error: None,
                created_at: Utc::now(),
                completed_at: None,
            }
        }

        fn service(
            client: MockJudgmentClient,
            judgments: MockAiJudgmentRepository,
            activity: MockActivityLogRepository,
            on: bool,
        ) -> Arc<JudgmentService> {
            Arc::new(
                JudgmentService::new(
                    Arc::new(client),
                    Box::new(judgments),
                    Box::new(settings(on)),
                )
                .with_handler(Arc::new(JustificationHandler::new(Box::new(activity)))),
            )
        }

        /// Activity mock whose merge call signals `notify` and counts calls.
        fn activity_expecting_merge(
            activity_id: Uuid,
            expected: Value,
            notify: Arc<Notify>,
        ) -> MockActivityLogRepository {
            let mut activity = MockActivityLogRepository::new();
            activity
                .expect_merge_activity_metadata()
                .withf(move |id, key, value| {
                    *id == activity_id && key == ACTIVITY_METADATA_KEY && *value == expected
                })
                .times(1)
                .returning(move |_, _, _| {
                    notify.notify_one();
                    Ok(())
                });
            activity
        }

        async fn finished(notify: &Notify) {
            tokio::time::timeout(Duration::from_secs(2), notify.notified())
                .await
                .expect("background step did not finish");
        }

        #[tokio::test]
        async fn rule_pass_is_stored_done_without_an_api_call() {
            let team = Uuid::new_v4();
            let activity_id = Uuid::new_v4();
            let notify = Arc::new(Notify::new());
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            let mut judgments = MockAiJudgmentRepository::new();
            judgments
                .expect_upsert_pending()
                .withf(move |new| {
                    new.team_id == team
                        && new.kind == JudgmentKind::Justification
                        && new.subject_type == SubjectType::Activity
                        && new.subject_id == activity_id
                        && new.input
                            == json!({
                                "reason_kind": "emergency_disable",
                                "reason": "Checkout 500s, see INC12345",
                                "feature_key": "checkout-v2",
                            })
                })
                .times(1)
                .returning(|new| Ok(pending_row(&new)));
            judgments
                .expect_mark_done()
                .withf(|_, _, result| {
                    result.derived == rule_derived() && result.model == RULE_MODEL
                })
                .times(1)
                .returning(|_, _, _| Ok(true));
            let activity = activity_expecting_merge(
                activity_id,
                json!({ "verdict": "ok", "probability": 1.0, "model": "rule" }),
                notify.clone(),
            );
            let service = service(client, judgments, activity, true);

            record_justification(
                Some(&service),
                team,
                SubjectType::Activity,
                activity_id,
                ReasonKind::EmergencyDisable,
                "Checkout 500s, see INC12345",
                Some("checkout-v2"),
            )
            .await;
            finished(&notify).await;
        }

        #[tokio::test]
        async fn vague_reason_is_submitted_and_the_verdict_merged() {
            let team = Uuid::new_v4();
            let activity_id = Uuid::new_v4();
            let notify = Arc::new(Notify::new());
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(1).returning(|_, _| {
                Ok(SystemOneResponse {
                    model: "jev-1.13.0".to_string(),
                    answers: answers(0.1, 0.9),
                    usage: Usage::default(),
                })
            });
            let mut judgments = MockAiJudgmentRepository::new();
            judgments
                .expect_upsert_pending()
                .withf(move |new| {
                    new.subject_type == SubjectType::Activity && new.subject_id == activity_id
                })
                .times(1)
                .returning(|new| Ok(pending_row(&new)));
            judgments
                .expect_mark_done()
                .times(1)
                .returning(|_, _, _| Ok(true));
            let activity = activity_expecting_merge(
                activity_id,
                json!({ "verdict": "weak", "probability": 0.1, "model": "jev-1.13.0" }),
                notify.clone(),
            );
            let service = service(client, judgments, activity, true);

            record_justification(
                Some(&service),
                team,
                SubjectType::Activity,
                activity_id,
                ReasonKind::EmergencyDisable,
                "urgent",
                None,
            )
            .await;
            finished(&notify).await;
        }

        #[tokio::test]
        async fn nothing_is_recorded_when_the_team_setting_is_off() {
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            let mut judgments = MockAiJudgmentRepository::new();
            judgments.expect_upsert_pending().times(0);
            let mut activity = MockActivityLogRepository::new();
            activity.expect_merge_activity_metadata().times(0);
            let service = service(client, judgments, activity, false);

            for reason in ["urgent", "see JIRA-123"] {
                record_justification(
                    Some(&service),
                    Uuid::new_v4(),
                    SubjectType::Activity,
                    Uuid::new_v4(),
                    ReasonKind::EmergencyDisable,
                    reason,
                    None,
                )
                .await;
            }
        }

        #[tokio::test]
        async fn blank_reason_and_missing_service_are_skipped() {
            record_justification(
                None,
                Uuid::new_v4(),
                SubjectType::Activity,
                Uuid::new_v4(),
                ReasonKind::EmergencyDisable,
                "urgent",
                None,
            )
            .await;
            let mut judgments = MockAiJudgmentRepository::new();
            judgments.expect_upsert_pending().times(0);
            let service = service(
                MockJudgmentClient::new(),
                judgments,
                MockActivityLogRepository::new(),
                true,
            );
            record_justification(
                Some(&service),
                Uuid::new_v4(),
                SubjectType::FreezeWindow,
                Uuid::new_v4(),
                ReasonKind::FreezeWindow,
                "   ",
                None,
            )
            .await;
        }

        #[tokio::test]
        async fn store_errors_never_reach_the_caller() {
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = calls.clone();
            let mut judgments = MockAiJudgmentRepository::new();
            judgments
                .expect_upsert_pending()
                .times(2)
                .returning(move |_| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Err(crate::Error::InvalidInput("db down".into()))
                });
            let mut client = MockJudgmentClient::new();
            client.expect_evaluate().times(0);
            let service = service(client, judgments, MockActivityLogRepository::new(), true);

            for reason in ["urgent", "see JIRA-123"] {
                record_justification(
                    Some(&service),
                    Uuid::new_v4(),
                    SubjectType::Activity,
                    Uuid::new_v4(),
                    ReasonKind::EmergencyEnable,
                    reason,
                    None,
                )
                .await;
            }
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            let _ = JudgmentError::Timeout;
        }

        #[tokio::test]
        async fn apply_ignores_subjects_that_are_not_activities() {
            let mut activity = MockActivityLogRepository::new();
            activity.expect_merge_activity_metadata().times(0);
            let handler = JustificationHandler::new(Box::new(activity));
            let judgment = AiJudgment {
                subject_type: "freeze_window".to_string(),
                derived: Some(rule_derived()),
                ..pending_row(&crate::database::ai::NewJudgment {
                    team_id: Uuid::new_v4(),
                    kind: JudgmentKind::Justification,
                    subject_type: SubjectType::FreezeWindow,
                    subject_id: Uuid::new_v4(),
                    input: json!({}),
                    input_hash: "h".to_string(),
                })
            };
            handler.apply(&judgment).await.unwrap();
        }
    }

    #[test]
    fn input_trims_and_caps_the_reason() {
        let long = "x".repeat(MAX_REASON_CHARS + 50);
        let input = build_input(ReasonKind::FreezeWindow, &format!("  {long}  "), None);
        assert_eq!(
            input["reason"].as_str().unwrap().chars().count(),
            MAX_REASON_CHARS
        );
        assert_eq!(input["reason_kind"], "freeze_window");
        assert_eq!(input["feature_key"], Value::Null);
    }
}
