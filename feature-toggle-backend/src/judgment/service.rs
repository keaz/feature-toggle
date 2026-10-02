//! Runs async judgments: persist input, call Jev, store answers, apply.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use log::{error, warn};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::client::JudgmentClient;
use super::types::{Answers, RequestParts};
use super::{JudgmentKind, SubjectType};
use crate::database::ai::{
    AiFeature, AiJudgment, AiJudgmentRepository, JudgmentResult, NewJudgment, RETRY_BATCH,
    TeamAiSettingsRepository,
};

/// Kind-specific logic. `build` and `derive` are pure so thresholds can be
/// re-derived from stored `raw_answers` without new API calls.
#[async_trait]
pub trait JudgmentHandler: Send + Sync {
    fn kind(&self) -> JudgmentKind;
    /// Build state and questions from the stored input snapshot.
    fn build(&self, input: &Value) -> RequestParts;
    /// Turn raw answers into the derived result.
    fn derive(&self, input: &Value, answers: &Answers) -> Value;
    /// Side effects after `done`. Must check that its subject is still fresh.
    async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Stored as done and `apply` ran.
    Applied,
    /// A newer submission replaced the input; nothing stored or applied.
    Stale,
    /// Stored as failed (or could not be stored); the sweep may retry it.
    Failed,
}

/// SHA-256 hex of the input JSON. `serde_json::Value` objects are
/// `BTreeMap`-backed, so key order does not change the hash.
pub fn input_hash(input: &Value) -> String {
    let bytes = serde_json::to_vec(input).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `model` stored for a result decided by a rule instead of the API.
pub const RULE_MODEL: &str = "rule";
const TEAM_FEATURE_OFF: &str = "skipped: AI feature turned off for this team";

pub struct JudgmentService {
    client: Arc<dyn JudgmentClient>,
    judgments: Box<dyn AiJudgmentRepository>,
    settings: Box<dyn TeamAiSettingsRepository>,
    handlers: HashMap<JudgmentKind, Arc<dyn JudgmentHandler>>,
}

impl JudgmentService {
    pub fn new(
        client: Arc<dyn JudgmentClient>,
        judgments: Box<dyn AiJudgmentRepository>,
        settings: Box<dyn TeamAiSettingsRepository>,
    ) -> Self {
        Self {
            client,
            judgments,
            settings,
            handlers: HashMap::new(),
        }
    }

    /// Registers the handler for its kind. Call before wrapping in `Arc`.
    pub fn with_handler(mut self, handler: Arc<dyn JudgmentHandler>) -> Self {
        self.handlers.insert(handler.kind(), handler);
        self
    }

    /// Whether `feature` is on for the team. Fails closed on a read error.
    pub async fn team_enabled(&self, team_id: Uuid, feature: AiFeature) -> bool {
        match self.settings.get(team_id).await {
            Ok(stored) => stored.settings.is_enabled(feature),
            Err(err) => {
                warn!("Could not read AI settings for team {team_id}: {err}");
                false
            }
        }
    }

    /// Persists a pending judgment and runs it in the background. Returns the
    /// row id right after the write; never waits for the API.
    pub async fn submit(
        self: &Arc<Self>,
        team_id: Uuid,
        kind: JudgmentKind,
        subject_type: SubjectType,
        subject_id: Uuid,
        input: Value,
    ) -> Result<Uuid, crate::Error> {
        let input_hash = input_hash(&input);
        let row = self
            .judgments
            .upsert_pending(NewJudgment {
                team_id,
                kind,
                subject_type,
                subject_id,
                input,
                input_hash,
            })
            .await?;
        let id = row.id;
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.run(row).await;
        });
        Ok(id)
    }

    /// Stores a result decided by a rule, with no API call: upserts the row,
    /// then (in the background, like `submit`) marks it done with `derived`
    /// and runs the handler's `apply`. Like `submit`, it does not check the
    /// team toggle; callers do. Returns the row id right after the write.
    pub async fn record_rule_result(
        self: &Arc<Self>,
        team_id: Uuid,
        kind: JudgmentKind,
        subject_type: SubjectType,
        subject_id: Uuid,
        input: Value,
        derived: Value,
    ) -> Result<Uuid, crate::Error> {
        let input_hash = input_hash(&input);
        let row = self
            .judgments
            .upsert_pending(NewJudgment {
                team_id,
                kind,
                subject_type,
                subject_id,
                input,
                input_hash,
            })
            .await?;
        let id = row.id;
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.finish_rule_result(row, derived).await;
        });
        Ok(id)
    }

    async fn finish_rule_result(&self, row: AiJudgment, derived: Value) {
        let result = JudgmentResult {
            model: RULE_MODEL.to_string(),
            raw_answers: Value::Object(Default::default()),
            derived: derived.clone(),
            input_tokens: None,
        };
        match self
            .judgments
            .mark_done(row.id, row.input_hash.clone(), result)
            .await
        {
            Ok(true) => {
                let done = AiJudgment {
                    status: "done".to_string(),
                    model: Some(RULE_MODEL.to_string()),
                    raw_answers: Some(Value::Object(Default::default())),
                    derived: Some(derived),
                    error: None,
                    completed_at: Some(Utc::now()),
                    ..row
                };
                let handler = done
                    .kind
                    .parse::<JudgmentKind>()
                    .ok()
                    .and_then(|kind| self.handlers.get(&kind).cloned());
                if let Some(handler) = handler
                    && let Err(err) = handler.apply(&done).await
                {
                    error!(
                        "AI judgment {} ({}) apply step failed: {err}",
                        done.id, done.kind
                    );
                }
            }
            // A newer submission replaced the input: nothing to store or apply.
            Ok(false) => {}
            Err(err) => error!("Could not store AI judgment {} result: {err}", row.id),
        }
    }

    pub async fn run(&self, row: AiJudgment) -> RunOutcome {
        let handler = row
            .kind
            .parse::<JudgmentKind>()
            .ok()
            .and_then(|kind| self.handlers.get(&kind).cloned());
        let Some(handler) = handler else {
            self.fail(&row, format!("no handler registered for kind {}", row.kind))
                .await;
            return RunOutcome::Failed;
        };

        let parts = handler.build(&row.input);
        let response = match self.client.evaluate(parts.state, parts.questions).await {
            Ok(response) => response,
            Err(err) => {
                self.fail(&row, err.to_string()).await;
                return RunOutcome::Failed;
            }
        };

        let derived = handler.derive(&row.input, &response.answers);
        let raw_answers = serde_json::to_value(&response.answers).unwrap_or(Value::Null);
        let input_tokens = i32::try_from(response.usage.input_tokens).ok();
        let result = JudgmentResult {
            model: response.model.clone(),
            raw_answers: raw_answers.clone(),
            derived: derived.clone(),
            input_tokens,
        };

        match self
            .judgments
            .mark_done(row.id, row.input_hash.clone(), result)
            .await
        {
            Ok(true) => {
                let done = AiJudgment {
                    status: "done".to_string(),
                    model: Some(response.model),
                    raw_answers: Some(raw_answers),
                    derived: Some(derived),
                    input_tokens,
                    error: None,
                    completed_at: Some(Utc::now()),
                    ..row
                };
                if let Err(err) = handler.apply(&done).await {
                    error!(
                        "AI judgment {} ({}) apply step failed: {err}",
                        done.id, done.kind
                    );
                }
                RunOutcome::Applied
            }
            Ok(false) => RunOutcome::Stale,
            Err(err) => {
                error!("Could not store AI judgment {} result: {err}", row.id);
                RunOutcome::Failed
            }
        }
    }

    /// Re-runs claimed rows one by one. A row whose team has since turned the
    /// feature off is marked failed without an API call, so no team data is
    /// sent after the toggle goes off. Returns how many rows it claimed.
    pub async fn retry_tick(&self) -> usize {
        let rows = match self.judgments.claim_retryable(RETRY_BATCH).await {
            Ok(rows) => rows,
            Err(err) => {
                error!("Could not claim retryable AI judgments: {err}");
                return 0;
            }
        };
        let count = rows.len();
        let mut enabled: HashMap<(Uuid, JudgmentKind), bool> = HashMap::new();
        for row in rows {
            let Ok(kind) = row.kind.parse::<JudgmentKind>() else {
                self.run(row).await;
                continue;
            };
            let key = (row.team_id, kind);
            let on = match enabled.get(&key) {
                Some(on) => *on,
                None => {
                    let on = self.team_enabled(row.team_id, kind.feature()).await;
                    enabled.insert(key, on);
                    on
                }
            };
            if on {
                self.run(row).await;
            } else {
                self.fail(&row, TEAM_FEATURE_OFF.to_string()).await;
            }
        }
        count
    }

    /// Stores the error text in the row (never in a log line).
    async fn fail(&self, row: &AiJudgment, message: String) {
        match self
            .judgments
            .mark_failed(row.id, row.input_hash.clone(), message)
            .await
        {
            Ok(_) => warn!("AI judgment {} ({}) failed", row.id, row.kind),
            Err(err) => error!("Could not mark AI judgment {} failed: {err}", row.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use tokio::sync::Notify;
    use uuid::Uuid;

    use super::*;
    use crate::database::ai::{
        AiFeature, AiJudgment, MockAiJudgmentRepository, MockTeamAiSettingsRepository,
        StoredTeamAiSettings, TeamAiSettings,
    };
    use crate::judgment::client::{JudgmentError, MockJudgmentClient};
    use crate::judgment::types::{
        Answer, Answers, Question, RequestParts, SystemOneResponse, Usage,
    };
    use crate::judgment::{JudgmentKind, SubjectType};

    struct FakeHandler {
        applied: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl JudgmentHandler for FakeHandler {
        fn kind(&self) -> JudgmentKind {
            JudgmentKind::FlagKind
        }

        fn build(&self, input: &Value) -> RequestParts {
            RequestParts {
                state: input.clone(),
                questions: BTreeMap::from([("q".to_string(), Question::noul("Is it?"))]),
            }
        }

        fn derive(&self, _input: &Value, answers: &Answers) -> Value {
            json!({ "q": answers.noul("q") })
        }

        async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error> {
            assert_eq!(judgment.status, "done");
            assert_eq!(judgment.derived, Some(json!({ "q": 0.9 })));
            self.applied.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(())
        }
    }

    fn row(hash: &str) -> AiJudgment {
        AiJudgment {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            subject_type: "feature".into(),
            subject_id: Uuid::new_v4(),
            kind: "flag_kind".into(),
            status: "pending".into(),
            attempts: 0,
            input: json!({ "feature": { "key": "checkout-v2" } }),
            input_hash: hash.into(),
            model: None,
            raw_answers: None,
            derived: None,
            input_tokens: None,
            error: None,
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    fn ok_response() -> SystemOneResponse {
        SystemOneResponse {
            model: "jev-1.13.0".into(),
            answers: Answers(BTreeMap::from([(
                "q".to_string(),
                Answer::Noul { noul: 0.9 },
            )])),
            usage: Usage {
                input_tokens: 42,
                output_tokens: 1,
            },
        }
    }

    fn service(
        client: MockJudgmentClient,
        repo: MockAiJudgmentRepository,
        settings: MockTeamAiSettingsRepository,
    ) -> (Arc<JudgmentService>, Arc<AtomicUsize>, Arc<Notify>) {
        let applied = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(Notify::new());
        let handler = FakeHandler {
            applied: applied.clone(),
            notify: notify.clone(),
        };
        let service = JudgmentService::new(Arc::new(client), Box::new(repo), Box::new(settings))
            .with_handler(Arc::new(handler));
        (Arc::new(service), applied, notify)
    }

    #[test]
    fn input_hash_ignores_key_order() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":2,"x":3}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":3,"y":2},"b":1}"#).unwrap();
        assert_eq!(input_hash(&a), input_hash(&b));
        assert_ne!(input_hash(&a), input_hash(&json!({ "b": 2 })));
        assert_eq!(input_hash(&a).len(), 64);
    }

    #[tokio::test]
    async fn success_writes_done_and_applies_once() {
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .times(1)
            .returning(|_, _| Ok(ok_response()));
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_done()
            .withf(|_, hash, result| {
                hash == "h1"
                    && result.model == "jev-1.13.0"
                    && result.derived == json!({ "q": 0.9 })
                    && result.input_tokens == Some(42)
            })
            .times(1)
            .returning(|_, _, _| Ok(true));
        repo.expect_mark_failed().times(0);
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.run(row("h1")).await, RunOutcome::Applied);
        assert_eq!(applied.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn client_error_writes_failed_and_skips_apply() {
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .times(1)
            .returning(|_, _| Err(JudgmentError::Timeout));
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_done().times(0);
        repo.expect_mark_failed()
            .withf(|_, hash, error| hash == "h1" && error.contains("timed out"))
            .times(1)
            .returning(|_, _, _| Ok(true));
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.run(row("h1")).await, RunOutcome::Failed);
        assert_eq!(applied.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn newer_submission_makes_old_run_stale() {
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .times(1)
            .returning(|_, _| Ok(ok_response()));
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_done()
            .times(1)
            .returning(|_, _, _| Ok(false));
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.run(row("old")).await, RunOutcome::Stale);
        assert_eq!(applied.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unknown_kind_is_marked_failed_without_a_call() {
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(0);
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_failed()
            .withf(|_, _, error| error.contains("no handler"))
            .times(1)
            .returning(|_, _, _| Ok(true));
        let (service, _, _) = service(client, repo, MockTeamAiSettingsRepository::new());
        let mut judgment = row("h1");
        judgment.kind = "approval_risk".into();

        assert_eq!(service.run(judgment).await, RunOutcome::Failed);
    }

    #[tokio::test]
    async fn submit_persists_then_runs_in_background() {
        let pending = row("ignored");
        let pending_id = pending.id;
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_upsert_pending()
            .withf(|new| new.kind == JudgmentKind::FlagKind && new.input_hash.len() == 64)
            .times(1)
            .returning(move |new| {
                let mut stored = pending.clone();
                stored.input = new.input;
                stored.input_hash = new.input_hash;
                Ok(stored)
            });
        repo.expect_mark_done()
            .times(1)
            .returning(|_, _, _| Ok(true));
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .times(1)
            .returning(|_, _| Ok(ok_response()));
        let (service, applied, notify) = service(client, repo, MockTeamAiSettingsRepository::new());

        let id = service
            .submit(
                Uuid::new_v4(),
                JudgmentKind::FlagKind,
                SubjectType::Feature,
                Uuid::new_v4(),
                json!({ "feature": { "key": "checkout-v2" } }),
            )
            .await
            .unwrap();
        assert_eq!(id, pending_id);
        tokio::time::timeout(Duration::from_secs(2), notify.notified())
            .await
            .expect("background run did not apply");
        assert_eq!(applied.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn team_enabled_reads_settings_and_fails_closed() {
        let mut settings = MockTeamAiSettingsRepository::new();
        let on = Uuid::new_v4();
        settings.expect_get().returning(move |team_id| {
            if team_id == on {
                Ok(StoredTeamAiSettings {
                    settings: TeamAiSettings {
                        flag_kind: true,
                        ..TeamAiSettings::default()
                    },
                    ..StoredTeamAiSettings::default()
                })
            } else {
                Err(crate::Error::InvalidInput("db down".into()))
            }
        });
        let (service, _, _) = service(
            MockJudgmentClient::new(),
            MockAiJudgmentRepository::new(),
            settings,
        );

        assert!(service.team_enabled(on, AiFeature::FlagKind).await);
        assert!(!service.team_enabled(on, AiFeature::NlSearch).await);
        assert!(
            !service
                .team_enabled(Uuid::new_v4(), AiFeature::FlagKind)
                .await
        );
    }

    fn settings_with_flag_kind(on: bool) -> MockTeamAiSettingsRepository {
        let mut settings = MockTeamAiSettingsRepository::new();
        settings.expect_get().returning(move |_| {
            Ok(StoredTeamAiSettings {
                settings: TeamAiSettings {
                    flag_kind: on,
                    ..TeamAiSettings::default()
                },
                ..StoredTeamAiSettings::default()
            })
        });
        settings
    }

    #[tokio::test]
    async fn retry_tick_runs_each_retryable_row() {
        let rows = vec![row("a"), row("b")];
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_claim_retryable()
            .withf(|limit| *limit == crate::database::ai::RETRY_BATCH)
            .times(1)
            .returning(move |_| Ok(rows.clone()));
        repo.expect_mark_done()
            .times(2)
            .returning(|_, _, _| Ok(true));
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .times(2)
            .returning(|_, _| Ok(ok_response()));
        let (service, applied, _) = service(client, repo, settings_with_flag_kind(true));

        assert_eq!(service.retry_tick().await, 2);
        assert_eq!(applied.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_tick_skips_teams_that_turned_the_feature_off() {
        let rows = vec![row("a")];
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_claim_retryable()
            .times(1)
            .returning(move |_| Ok(rows.clone()));
        repo.expect_mark_done().times(0);
        repo.expect_mark_failed()
            .withf(|_, hash, error| hash == "a" && error.contains("turned off"))
            .times(1)
            .returning(|_, _, _| Ok(true));
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(0);
        let (service, applied, _) = service(client, repo, settings_with_flag_kind(false));

        assert_eq!(service.retry_tick().await, 1);
        assert_eq!(applied.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn each_kind_maps_to_its_team_toggle() {
        assert_eq!(
            JudgmentKind::ApprovalRisk.feature(),
            AiFeature::ApprovalRisk
        );
        assert_eq!(
            JudgmentKind::Justification.feature(),
            AiFeature::JustificationCheck
        );
        assert_eq!(JudgmentKind::FlagKind.feature(), AiFeature::FlagKind);
    }
}
