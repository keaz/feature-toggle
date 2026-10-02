//! Flag kind (AI-30): classifies a feature flag as release, experiment, ops,
//! permission, or config. See `docs/ai-judgments/design.md` §5.3.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use log::warn;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::service::{JudgmentHandler, JudgmentService, input_hash};
use super::types::{Answers, Question, RequestParts};
use super::{JudgmentKind, SubjectType};
use crate::database::ai::AiJudgment;
use crate::database::entity::Feature as EntityFeature;
use crate::database::feature::FeatureRepository;
use crate::model::{Feature as ModelFeature, FlagKind, FlagKindSource};

/// An answer below this confidence is not stored or shown.
pub const MIN_KIND_CONFIDENCE: f64 = 0.5;
/// A tag below this probability is not suggested.
pub const MIN_TAG_PROBABILITY: f64 = 0.6;
pub const MAX_SUGGESTED_TAGS: usize = 5;
/// Candidate tags sent to Jev in one suggestions request.
pub const MAX_TAG_CANDIDATES: i64 = 50;
/// Features one backfill call submits.
pub const BACKFILL_LIMIT: i64 = 500;
/// Longest `description` / `purpose` sent to Jev.
const MAX_TEXT_CHARS: usize = 1000;

const Q_KIND: &str = "kind";
const UNKNOWN: &str = "unknown";

/// A kind answer that passed the confidence threshold.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindAnswer {
    pub value: FlagKind,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TagSuggestion {
    pub tag: String,
    pub probability: f64,
}

/// The feature fields a suggestions request sends.
#[derive(Debug, Clone, Default)]
pub struct SuggestionInput {
    pub key: String,
    pub description: Option<String>,
    pub purpose: Option<String>,
    pub tags: Vec<String>,
}

fn clean_text(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    (!text.is_empty()).then(|| text.chars().take(MAX_TEXT_CHARS).collect())
}

/// SHA-256 over a canonical JSON of the fields that decide the kind. A blank
/// description or purpose counts as absent. Owner, lifecycle, and the rest
/// never change it, so editing them does not re-classify.
pub fn content_hash(
    key: &str,
    description: Option<&str>,
    purpose: Option<&str>,
    tags: &[String],
) -> String {
    input_hash(&json!({
        "key": key,
        "description": clean_text(description),
        "purpose": clean_text(purpose),
        "tags": tags,
    }))
}

pub fn build_input(
    key: &str,
    description: Option<&str>,
    purpose: Option<&str>,
    tags: &[String],
    feature_type: &str,
) -> Value {
    json!({
        "feature": {
            "key": key,
            "description": clean_text(description),
            "purpose": clean_text(purpose),
            "tags": tags,
            "feature_type": feature_type,
        },
        "content_hash": content_hash(key, description, purpose, tags),
    })
}

pub fn input_from_model(feature: &ModelFeature) -> Value {
    let feature_type = match feature.feature_type {
        crate::model::FeatureType::Simple => "simple",
        crate::model::FeatureType::Contextual => "contextual",
    };
    build_input(
        &feature.key,
        feature.description.as_deref(),
        feature.purpose.as_deref(),
        &feature.tags,
        feature_type,
    )
}

pub fn input_from_entity(feature: &EntityFeature) -> Value {
    let feature_type = match feature.feature_type {
        crate::database::entity::FeatureType::Simple => "simple",
        crate::database::entity::FeatureType::Contextual => "contextual",
    };
    build_input(
        &feature.key,
        feature.description.as_deref(),
        feature.purpose.as_deref(),
        &feature.tags,
        feature_type,
    )
}

fn stored_hash(input: &Value) -> Option<&str> {
    input.get("content_hash").and_then(Value::as_str)
}

fn kind_question() -> Question {
    let options = [
        (
            "release",
            "Temporary flag that hides or gradually rolls out new or changed functionality. Removed after the rollout finishes.",
        ),
        (
            "experiment",
            "Temporary flag for an A/B test or experiment that compares variants and measures results.",
        ),
        (
            "ops",
            "Long-lived operational control: kill switch, circuit breaker, maintenance mode, load shedding, or fallback toggle.",
        ),
        (
            "permission",
            "Long-lived entitlement: turns functionality on for specific plans, customers, roles, or beta programs.",
        ),
        (
            "config",
            "Long-lived configuration value that tunes behavior, such as a limit, timeout, or text, instead of gating a feature.",
        ),
        (
            UNKNOWN,
            "The key, description, purpose, and tags are not enough to tell.",
        ),
    ];
    Question::choice(
        "What kind of feature flag is `feature`, based on its key, description, purpose, and tags?",
        options
            .into_iter()
            .map(|(name, text)| (name, Some(Value::from(text)))),
    )
    .expect("six options are within the choice limits")
}

/// State and question for a stored input snapshot.
pub fn build(input: &Value) -> RequestParts {
    RequestParts {
        state: json!({ "feature": input.get("feature").cloned().unwrap_or(Value::Null) }),
        questions: BTreeMap::from([(Q_KIND.to_string(), kind_question())]),
    }
}

/// The kind with its probabilities, or `None` for `unknown`, a low
/// confidence, a missing answer, or a name that is not a kind.
pub fn classify(answers: &Answers) -> Option<KindAnswer> {
    let answer = answers.choice(Q_KIND)?;
    if answer.confidence < MIN_KIND_CONFIDENCE {
        return None;
    }
    let value = answer.choice.parse::<FlagKind>().ok()?;
    Some(KindAnswer {
        value,
        probabilities: answer.probabilities.clone(),
        confidence: answer.confidence,
    })
}

/// `{ kind: "ops" | null, confidence, probabilities }`.
pub fn derive(answers: &Answers) -> Value {
    let kind = classify(answers);
    let raw = answers.choice(Q_KIND);
    json!({
        "kind": kind.as_ref().map(|answer| answer.value.as_str()),
        "confidence": raw.map(|answer| answer.confidence),
        "probabilities": raw.map(|answer| answer.probabilities.clone()).unwrap_or_default(),
    })
}

/// Whether to submit: nothing is stored yet, or the stored input is for other content.
pub fn needs_submit(existing: Option<&AiJudgment>, hash: &str) -> bool {
    existing.is_none_or(|row| stored_hash(&row.input) != Some(hash))
}

fn feature_state(input: &SuggestionInput) -> Value {
    json!({
        "feature": {
            "key": input.key,
            "description": clean_text(input.description.as_deref()),
            "purpose": clean_text(input.purpose.as_deref()),
            "tags": input.tags,
        }
    })
}

fn tag_id(index: usize) -> String {
    format!("t{index}")
}

/// One request: the kind question plus one yes/no question per candidate tag.
pub fn suggestion_parts(input: &SuggestionInput, candidates: &[String]) -> RequestParts {
    let mut questions = BTreeMap::from([(Q_KIND.to_string(), kind_question())]);
    for (index, tag) in candidates.iter().enumerate() {
        questions.insert(
            tag_id(index),
            Question::noul(json!({
                "candidate_tag": tag,
                "question": "Does the tag `candidate_tag` describe `feature`?",
            })),
        );
    }
    RequestParts {
        state: feature_state(input),
        questions,
    }
}

/// At most five tags with probability at least 0.6, best first.
pub fn suggested_tags(answers: &Answers, candidates: &[String]) -> Vec<TagSuggestion> {
    let mut suggested: Vec<TagSuggestion> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, tag)| {
            let probability = answers.noul(&tag_id(index))?;
            (probability >= MIN_TAG_PROBABILITY).then(|| TagSuggestion {
                tag: tag.clone(),
                probability,
            })
        })
        .collect();
    suggested.sort_by(|a, b| b.probability.total_cmp(&a.probability));
    suggested.truncate(MAX_SUGGESTED_TAGS);
    suggested
}

/// Stores the AI kind when the feature still matches what was judged.
pub struct FlagKindHandler {
    features: Box<dyn FeatureRepository>,
}

impl FlagKindHandler {
    pub fn new(features: Box<dyn FeatureRepository>) -> Self {
        Self { features }
    }
}

#[async_trait]
impl JudgmentHandler for FlagKindHandler {
    fn kind(&self) -> JudgmentKind {
        JudgmentKind::FlagKind
    }

    fn build(&self, input: &Value) -> RequestParts {
        build(input)
    }

    fn derive(&self, _input: &Value, answers: &Answers) -> Value {
        derive(answers)
    }

    /// Writes the kind only when all of these hold: the answer names a kind,
    /// no user chose or cleared one, and the feature's content still hashes to
    /// the judged input. A stale answer is dropped; the edit that made it stale
    /// already queued a newer judgment.
    async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error> {
        let derived = judgment.derived.as_ref().unwrap_or(&Value::Null);
        let Some(kind) = derived
            .get("kind")
            .and_then(Value::as_str)
            .and_then(|name| name.parse::<FlagKind>().ok())
        else {
            return Ok(());
        };
        let confidence = derived
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;

        let feature = match self.features.get_feature_by_id(judgment.subject_id).await {
            Ok(feature) => feature,
            Err(crate::Error::NotFound(_)) => return Ok(()),
            Err(err) => return Err(err),
        };
        if feature.flag_kind_source == Some(FlagKindSource::User) {
            return Ok(());
        }
        let current = content_hash(
            &feature.key,
            feature.description.as_deref(),
            feature.purpose.as_deref(),
            &feature.tags,
        );
        if stored_hash(&judgment.input) != Some(current.as_str()) {
            return Ok(());
        }
        self.features
            .set_ai_flag_kind(feature.id, kind, confidence)
            .await?;
        Ok(())
    }
}

async fn submit(
    service: &Arc<JudgmentService>,
    team_id: Uuid,
    feature_id: Uuid,
    input: Value,
) -> Result<(), crate::Error> {
    service
        .submit(
            team_id,
            JudgmentKind::FlagKind,
            SubjectType::Feature,
            feature_id,
            input,
        )
        .await
        .map(|_| ())
}

/// Queues a classification after a feature was created or edited. Skips when
/// the subsystem or the team toggle is off, when a user chose the kind, or when
/// the stored judgment already covers this content. Never fails the caller:
/// errors are logged and dropped.
pub async fn record_flag_kind(
    service: Option<&Arc<JudgmentService>>,
    team_id: Uuid,
    feature: &ModelFeature,
) {
    let Some(service) = service else {
        return;
    };
    if feature.flag_kind_source == Some(FlagKindSource::User) {
        return;
    }
    let Ok(feature_id) = feature.id.parse() else {
        return;
    };
    if !service
        .team_enabled(team_id, JudgmentKind::FlagKind.feature())
        .await
    {
        return;
    }
    let input = input_from_model(feature);
    let hash = stored_hash(&input).unwrap_or_default().to_string();
    let existing = match service
        .judgment_for(SubjectType::Feature, feature_id, JudgmentKind::FlagKind)
        .await
    {
        Ok(existing) => existing,
        Err(err) => {
            warn!("Could not read flag kind judgment for feature {feature_id}: {err}");
            return;
        }
    };
    if !needs_submit(existing.as_ref(), &hash) {
        return;
    }
    if let Err(err) = submit(service, team_id, feature_id, input).await {
        warn!("Could not queue flag kind judgment for feature {feature_id}: {err}");
    }
}

/// Queues a judgment for each feature without a `done` judgment for its
/// current content. Returns how many it queued. The client semaphore bounds how
/// many run at once.
pub async fn backfill(
    service: &Arc<JudgmentService>,
    team_id: Uuid,
    features: &[EntityFeature],
) -> usize {
    let ids: Vec<Uuid> = features.iter().map(|feature| feature.id).collect();
    let existing = match service
        .judgments_for(SubjectType::Feature, ids, JudgmentKind::FlagKind)
        .await
    {
        Ok(rows) => rows,
        Err(err) => {
            warn!("Could not read flag kind judgments for backfill: {err}");
            return 0;
        }
    };
    let by_subject: std::collections::HashMap<Uuid, AiJudgment> = existing
        .into_iter()
        .map(|row| (row.subject_id, row))
        .collect();

    let mut queued = 0;
    for feature in features {
        let input = input_from_entity(feature);
        let hash = stored_hash(&input).unwrap_or_default();
        let current = by_subject
            .get(&feature.id)
            .filter(|row| row.status == "done");
        if !needs_submit(current, hash) {
            continue;
        }
        match submit(service, team_id, feature.id, input).await {
            Ok(()) => queued += 1,
            Err(err) => warn!(
                "Could not queue flag kind judgment for feature {}: {err}",
                feature.id
            ),
        }
    }
    queued
}

/// Shared by REST handler tests: a judgment service whose store returns canned
/// rows and records every submission instead of calling a model.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    use chrono::Utc;

    use super::*;
    use crate::database::ai::{
        MockAiJudgmentRepository, MockTeamAiSettingsRepository, StoredTeamAiSettings,
        TeamAiSettings,
    };
    use crate::database::feature::MockFeatureRepository;
    use crate::judgment::JudgmentClient;
    use crate::judgment::client::{JudgmentError, MockJudgmentClient};

    pub(crate) fn judgment(
        subject_id: Uuid,
        status: &str,
        input: Value,
        derived: Value,
    ) -> AiJudgment {
        AiJudgment {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            subject_type: "feature".into(),
            subject_id,
            kind: "flag_kind".into(),
            status: status.into(),
            attempts: 1,
            input_hash: input_hash(&input),
            input,
            model: None,
            raw_answers: None,
            derived: Some(derived),
            input_tokens: None,
            error: None,
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    /// An unclassified entity feature with the given key and no judgment yet.
    pub(crate) fn entity_feature(key: &str) -> EntityFeature {
        EntityFeature {
            id: Uuid::new_v4(),
            key: key.into(),
            description: Some(format!("Description of {key}")),
            feature_type: crate::database::entity::FeatureType::Simple,
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
            flag_kind: None,
            flag_kind_source: None,
            flag_kind_confidence: None,
            dependencies: vec![],
        }
    }

    pub(crate) type Upserts = Arc<Mutex<Vec<(Uuid, Value)>>>;

    /// A service whose judgment store returns `stored` for every lookup and
    /// records each upsert. Nothing reaches a model: the client times out.
    pub(crate) fn recording_service(
        on: bool,
        stored: Vec<AiJudgment>,
    ) -> (Arc<JudgmentService>, Upserts) {
        let upserts: Upserts = Arc::new(Mutex::new(Vec::new()));
        let recorded = upserts.clone();
        let mut judgments = MockAiJudgmentRepository::new();
        let by_subject = stored.clone();
        judgments
            .expect_get_for_subject()
            .returning(move |_, id, _| {
                Ok(by_subject.iter().find(|row| row.subject_id == id).cloned())
            });
        judgments
            .expect_get_for_subjects()
            .returning(move |_, ids, _| {
                Ok(stored
                    .iter()
                    .filter(|row| ids.contains(&row.subject_id))
                    .cloned()
                    .collect())
            });
        judgments.expect_upsert_pending().returning(move |new| {
            recorded
                .lock()
                .unwrap()
                .push((new.subject_id, new.input.clone()));
            Ok(AiJudgment {
                status: "pending".into(),
                input_hash: new.input_hash,
                input: new.input,
                subject_id: new.subject_id,
                team_id: new.team_id,
                ..judgment(new.subject_id, "pending", json!({}), json!({}))
            })
        });
        judgments.expect_mark_failed().returning(|_, _, _| Ok(true));
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
        let mut client = MockJudgmentClient::new();
        client
            .expect_evaluate()
            .returning(|_, _| Err(JudgmentError::Timeout));
        let client: Arc<dyn JudgmentClient> = Arc::new(client);
        let service = JudgmentService::new(client, Box::new(judgments), Box::new(settings))
            .with_handler(Arc::new(FlagKindHandler::new(Box::new(
                MockFeatureRepository::new(),
            ))));
        (Arc::new(service), upserts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judgment::types::{Answer, ChoiceAnswer};

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(|tag| tag.to_string()).collect()
    }

    fn kind_answers(choice: &str, confidence: f64) -> Answers {
        Answers(BTreeMap::from([(
            Q_KIND.to_string(),
            Answer::Choice(ChoiceAnswer {
                choice: choice.to_string(),
                probabilities: BTreeMap::from([
                    (choice.to_string(), confidence),
                    ("config".to_string(), 1.0 - confidence),
                ]),
                confidence,
            }),
        )]))
    }

    #[test]
    fn content_hash_is_stable_and_sensitive_to_each_field() {
        let base = content_hash("k", Some("d"), Some("p"), &tags(&["a", "b"]));
        assert_eq!(base.len(), 64);
        assert_eq!(
            base,
            content_hash("k", Some("d"), Some("p"), &tags(&["a", "b"]))
        );
        assert_ne!(
            base,
            content_hash("k2", Some("d"), Some("p"), &tags(&["a", "b"]))
        );
        assert_ne!(
            base,
            content_hash("k", Some("d2"), Some("p"), &tags(&["a", "b"]))
        );
        assert_ne!(
            base,
            content_hash("k", Some("d"), Some("p2"), &tags(&["a", "b"]))
        );
        assert_ne!(base, content_hash("k", Some("d"), Some("p"), &tags(&["a"])));
    }

    #[test]
    fn content_hash_treats_blank_text_as_absent() {
        assert_eq!(
            content_hash("k", None, None, &[]),
            content_hash("k", Some("  "), Some(""), &[])
        );
        assert_eq!(
            content_hash("k", Some("d"), None, &[]),
            content_hash("k", Some(" d "), None, &[])
        );
    }

    #[test]
    fn derive_stores_a_confident_kind() {
        let derived = derive(&kind_answers("ops", 0.9));
        assert_eq!(derived["kind"], "ops");
        assert_eq!(derived["confidence"], 0.9);
        assert_eq!(derived["probabilities"]["ops"], 0.9);
    }

    #[test]
    fn derive_returns_null_for_unknown_low_confidence_and_bad_answers() {
        for answers in [
            kind_answers("unknown", 0.95),
            kind_answers("ops", 0.49),
            kind_answers("banana", 0.9),
            Answers::default(),
        ] {
            assert_eq!(derive(&answers)["kind"], Value::Null);
            assert!(classify(&answers).is_none());
        }
    }

    #[test]
    fn derive_keeps_confidence_and_probabilities_when_the_kind_is_null() {
        let derived = derive(&kind_answers("unknown", 0.95));
        assert_eq!(derived["confidence"], 0.95);
        assert_eq!(derived["probabilities"]["unknown"], 0.95);
    }

    #[test]
    fn the_confidence_threshold_is_inclusive() {
        assert_eq!(
            derive(&kind_answers("ops", MIN_KIND_CONFIDENCE))["kind"],
            "ops"
        );
    }

    fn row_with_hash(hash: Option<&str>) -> AiJudgment {
        AiJudgment {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            subject_type: "feature".into(),
            subject_id: Uuid::new_v4(),
            kind: "flag_kind".into(),
            status: "done".into(),
            attempts: 1,
            input: match hash {
                Some(hash) => json!({ "content_hash": hash }),
                None => json!({}),
            },
            input_hash: "x".into(),
            model: None,
            raw_answers: None,
            derived: None,
            input_tokens: None,
            error: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
        }
    }

    #[test]
    fn needs_submit_only_when_nothing_is_stored_or_the_hash_differs() {
        assert!(needs_submit(None, "h1"));
        assert!(!needs_submit(Some(&row_with_hash(Some("h1"))), "h1"));
        assert!(needs_submit(Some(&row_with_hash(Some("h0"))), "h1"));
        assert!(needs_submit(Some(&row_with_hash(None)), "h1"));
    }

    #[test]
    fn suggested_tags_apply_threshold_limit_and_order() {
        let candidates = tags(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let probabilities = [0.95, 0.6, 0.59, 0.7, 0.99, 0.8, 0.61, 0.1];
        let answers = Answers(
            probabilities
                .iter()
                .enumerate()
                .map(|(index, probability)| (tag_id(index), Answer::Noul { noul: *probability }))
                .collect(),
        );
        let suggested = suggested_tags(&answers, &candidates);
        let names: Vec<&str> = suggested.iter().map(|item| item.tag.as_str()).collect();
        assert_eq!(names, ["e", "a", "f", "d", "g"]);
        assert_eq!(suggested[0].probability, 0.99);

        let only_threshold = Answers(BTreeMap::from([(
            tag_id(1),
            Answer::Noul {
                noul: MIN_TAG_PROBABILITY,
            },
        )]));
        assert_eq!(suggested_tags(&only_threshold, &candidates).len(), 1);
        assert!(suggested_tags(&Answers::default(), &candidates).is_empty());
    }

    #[test]
    fn build_input_has_the_designed_shape() {
        let input = build_input(
            "kill-checkout",
            Some("  Kill switch  "),
            Some(""),
            &tags(&["payments"]),
            "simple",
        );
        assert_eq!(
            input,
            json!({
                "feature": {
                    "key": "kill-checkout",
                    "description": "Kill switch",
                    "purpose": null,
                    "tags": ["payments"],
                    "feature_type": "simple",
                },
                "content_hash": content_hash(
                    "kill-checkout", Some("Kill switch"), None, &tags(&["payments"])
                ),
            })
        );
    }

    /// The wording below is the contract with Jev (design §5.3). A change here
    /// changes every classification, so it must be deliberate.
    #[test]
    fn kind_request_wire_snapshot() {
        let input = build_input("kill-checkout", Some("Kill switch"), None, &[], "simple");
        let parts = build(&input);
        assert_eq!(
            parts.state,
            json!({ "feature": {
                "key": "kill-checkout",
                "description": "Kill switch",
                "purpose": null,
                "tags": [],
                "feature_type": "simple",
            } })
        );
        assert_eq!(
            serde_json::to_value(&parts.questions).unwrap(),
            json!({
                "kind": {
                    "type": "choice",
                    "instructions": "What kind of feature flag is `feature`, based on its key, description, purpose, and tags?",
                    "criteria": {
                        "release": "Temporary flag that hides or gradually rolls out new or changed functionality. Removed after the rollout finishes.",
                        "experiment": "Temporary flag for an A/B test or experiment that compares variants and measures results.",
                        "ops": "Long-lived operational control: kill switch, circuit breaker, maintenance mode, load shedding, or fallback toggle.",
                        "permission": "Long-lived entitlement: turns functionality on for specific plans, customers, roles, or beta programs.",
                        "config": "Long-lived configuration value that tunes behavior, such as a limit, timeout, or text, instead of gating a feature.",
                        "unknown": "The key, description, purpose, and tags are not enough to tell.",
                    },
                },
            })
        );
    }

    #[test]
    fn suggestion_request_wire_snapshot() {
        let input = SuggestionInput {
            key: "kill-checkout".into(),
            description: Some("Kill switch".into()),
            purpose: None,
            tags: tags(&["payments"]),
        };
        let parts = suggestion_parts(&input, &tags(&["billing", "ops"]));
        assert_eq!(
            parts.state,
            json!({ "feature": {
                "key": "kill-checkout",
                "description": "Kill switch",
                "purpose": null,
                "tags": ["payments"],
            } })
        );
        let questions = serde_json::to_value(&parts.questions).unwrap();
        assert_eq!(questions.as_object().unwrap().len(), 3);
        assert_eq!(questions["kind"]["type"], "choice");
        assert_eq!(
            questions["t0"],
            json!({
                "type": "noul",
                "instructions": {
                    "candidate_tag": "billing",
                    "question": "Does the tag `candidate_tag` describe `feature`?",
                },
            })
        );
        assert_eq!(questions["t1"]["instructions"]["candidate_tag"], "ops");
    }

    mod pipeline {
        use chrono::Utc;

        use super::*;
        use crate::database::entity::FeatureType as EntityFeatureType;
        use crate::database::feature::MockFeatureRepository;
        use crate::judgment::flag_kind::test_support::{judgment, recording_service as service};
        use crate::model::{FeatureType, ID, LifecycleStage};

        fn entity_feature(
            id: Uuid,
            description: &str,
            kind: Option<FlagKind>,
            source: Option<FlagKindSource>,
        ) -> EntityFeature {
            EntityFeature {
                id,
                key: "kill-checkout".into(),
                description: Some(description.into()),
                feature_type: EntityFeatureType::Simple,
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
                owner: Some("alice".into()),
                purpose: None,
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
                flag_kind: kind,
                flag_kind_source: source,
                flag_kind_confidence: None,
                dependencies: vec![],
            }
        }

        fn model_feature(
            id: Uuid,
            description: &str,
            owner: &str,
            source: Option<FlagKindSource>,
        ) -> ModelFeature {
            ModelFeature {
                id: ID::from(id),
                key: "kill-checkout".into(),
                description: Some(description.into()),
                feature_type: FeatureType::Simple,
                enabled: true,
                created_at: Utc::now(),
                kill_switch_enabled: false,
                kill_switch_activated_at: None,
                rollback_scheduled_at: None,
                emergency_override_reason: None,
                emergency_override_expires_at: None,
                emergency_override_actor_id: None,
                emergency_override_applied_at: None,
                lifecycle_stage: LifecycleStage::Active,
                owner: Some(owner.into()),
                purpose: None,
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
                is_stale: false,
                stale_reasons: vec![],
                dependencies: vec![],
                team_id: ID::from(Uuid::new_v4()),
                pending_approval_request_id: None,
                flag_kind: source.map(|_| FlagKind::Ops),
                flag_kind_source: source,
                flag_kind_confidence: None,
            }
        }

        fn done_ops(feature: &EntityFeature) -> AiJudgment {
            judgment(
                feature.id,
                "done",
                input_from_entity(feature),
                json!({ "kind": "ops", "confidence": 0.9, "probabilities": {} }),
            )
        }

        fn handler(repo: MockFeatureRepository) -> FlagKindHandler {
            FlagKindHandler::new(Box::new(repo))
        }

        #[tokio::test]
        async fn apply_writes_the_kind_for_current_content() {
            let id = Uuid::new_v4();
            let feature = entity_feature(id, "Kill switch for checkout", None, None);
            let judged = done_ops(&feature);
            let mut repo = MockFeatureRepository::new();
            let loaded = feature.clone();
            repo.expect_get_feature_by_id()
                .returning(move |_| Ok(loaded.clone()));
            repo.expect_set_ai_flag_kind()
                .withf(move |wanted, kind, confidence| {
                    *wanted == id && *kind == FlagKind::Ops && (*confidence - 0.9).abs() < 1e-6
                })
                .times(1)
                .returning(|_, _, _| Ok(true));
            handler(repo).apply(&judged).await.unwrap();
        }

        #[tokio::test]
        async fn apply_rewrites_an_earlier_ai_kind() {
            let id = Uuid::new_v4();
            let feature = entity_feature(
                id,
                "Kill switch for checkout",
                Some(FlagKind::Release),
                Some(FlagKindSource::Ai),
            );
            let judged = done_ops(&feature);
            let mut repo = MockFeatureRepository::new();
            repo.expect_get_feature_by_id()
                .returning(move |_| Ok(feature.clone()));
            repo.expect_set_ai_flag_kind()
                .times(1)
                .returning(|_, _, _| Ok(true));
            handler(repo).apply(&judged).await.unwrap();
        }

        #[tokio::test]
        async fn apply_skips_a_user_chosen_kind() {
            let id = Uuid::new_v4();
            let feature = entity_feature(
                id,
                "Kill switch for checkout",
                Some(FlagKind::Config),
                Some(FlagKindSource::User),
            );
            let judged = done_ops(&feature);
            let mut repo = MockFeatureRepository::new();
            repo.expect_get_feature_by_id()
                .returning(move |_| Ok(feature.clone()));
            repo.expect_set_ai_flag_kind().times(0);
            handler(repo).apply(&judged).await.unwrap();
        }

        #[tokio::test]
        async fn apply_skips_a_user_who_cleared_the_kind() {
            let id = Uuid::new_v4();
            let feature = entity_feature(id, "Kill switch for checkout", None, None);
            let judged = done_ops(&feature);
            let mut cleared = feature.clone();
            cleared.flag_kind_source = Some(FlagKindSource::User);
            let mut repo = MockFeatureRepository::new();
            repo.expect_get_feature_by_id()
                .returning(move |_| Ok(cleared.clone()));
            repo.expect_set_ai_flag_kind().times(0);
            handler(repo).apply(&judged).await.unwrap();
        }

        #[tokio::test]
        async fn apply_skips_when_the_content_changed_since_the_judgment() {
            let id = Uuid::new_v4();
            let judged_feature = entity_feature(id, "Kill switch for checkout", None, None);
            let judged = done_ops(&judged_feature);
            let edited = entity_feature(id, "Gradual rollout of the new checkout", None, None);
            let mut repo = MockFeatureRepository::new();
            repo.expect_get_feature_by_id()
                .returning(move |_| Ok(edited.clone()));
            repo.expect_set_ai_flag_kind().times(0);
            handler(repo).apply(&judged).await.unwrap();
        }

        #[tokio::test]
        async fn apply_skips_unknown_and_low_confidence_answers() {
            let id = Uuid::new_v4();
            let feature = entity_feature(id, "Kill switch for checkout", None, None);
            let judged = judgment(
                id,
                "done",
                input_from_entity(&feature),
                json!({ "kind": null, "confidence": 0.95, "probabilities": {} }),
            );
            let mut repo = MockFeatureRepository::new();
            repo.expect_get_feature_by_id()
                .returning(move |_| Ok(feature.clone()));
            repo.expect_set_ai_flag_kind().times(0);
            handler(repo).apply(&judged).await.unwrap();
        }

        #[tokio::test]
        async fn apply_ignores_a_deleted_feature() {
            let id = Uuid::new_v4();
            let judged = done_ops(&entity_feature(id, "Kill switch for checkout", None, None));
            let mut repo = MockFeatureRepository::new();
            repo.expect_get_feature_by_id()
                .returning(|id| Err(crate::Error::NotFound(id)));
            repo.expect_set_ai_flag_kind().times(0);
            handler(repo).apply(&judged).await.unwrap();
        }

        fn team() -> Uuid {
            Uuid::new_v4()
        }

        #[tokio::test]
        async fn a_new_feature_is_submitted_with_its_content_hash() {
            let id = Uuid::new_v4();
            let (service, upserts) = service(true, vec![]);
            let feature = model_feature(id, "Kill switch for checkout", "alice", None);
            record_flag_kind(Some(&service), team(), &feature).await;

            let upserts = upserts.lock().unwrap();
            assert_eq!(upserts.len(), 1);
            assert_eq!(upserts[0].0, id);
            assert_eq!(
                upserts[0].1["content_hash"],
                content_hash(
                    "kill-checkout",
                    Some("Kill switch for checkout"),
                    None,
                    &tags(&["payments"])
                )
            );
        }

        #[tokio::test]
        async fn a_content_change_submits_again() {
            let id = Uuid::new_v4();
            let old = entity_feature(id, "Kill switch for checkout", None, None);
            let (service, upserts) = service(true, vec![done_ops(&old)]);
            let edited = model_feature(id, "Gradual rollout of the new checkout", "alice", None);
            record_flag_kind(Some(&service), team(), &edited).await;
            assert_eq!(upserts.lock().unwrap().len(), 1);
        }

        #[tokio::test]
        async fn changing_only_the_owner_does_not_submit() {
            let id = Uuid::new_v4();
            let old = entity_feature(id, "Kill switch for checkout", None, None);
            let (service, upserts) = service(true, vec![done_ops(&old)]);
            let new_owner = model_feature(id, "Kill switch for checkout", "bob", None);
            record_flag_kind(Some(&service), team(), &new_owner).await;
            assert!(upserts.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn a_user_chosen_kind_never_submits() {
            let (service, upserts) = service(true, vec![]);
            let feature = model_feature(
                Uuid::new_v4(),
                "Kill switch for checkout",
                "alice",
                Some(FlagKindSource::User),
            );
            record_flag_kind(Some(&service), team(), &feature).await;
            assert!(upserts.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn an_ai_kind_is_reclassified_after_an_edit() {
            let id = Uuid::new_v4();
            let (service, upserts) = service(true, vec![]);
            let feature = model_feature(id, "Kill switch", "alice", Some(FlagKindSource::Ai));
            record_flag_kind(Some(&service), team(), &feature).await;
            assert_eq!(upserts.lock().unwrap().len(), 1);
        }

        #[tokio::test]
        async fn nothing_is_submitted_with_the_team_toggle_off_or_without_a_service() {
            let feature = model_feature(Uuid::new_v4(), "Kill switch", "alice", None);
            let (service, upserts) = service(false, vec![]);
            record_flag_kind(Some(&service), team(), &feature).await;
            record_flag_kind(None, team(), &feature).await;
            assert!(upserts.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn backfill_skips_features_with_a_done_judgment_for_the_same_content() {
            let current = entity_feature(Uuid::new_v4(), "Kill switch for checkout", None, None);
            let edited_since =
                entity_feature(Uuid::new_v4(), "Rollout of the new page", None, None);
            let never_judged = entity_feature(Uuid::new_v4(), "Max items per page", None, None);
            let pending = entity_feature(Uuid::new_v4(), "Beta for enterprise plans", None, None);

            let stale = judgment(
                edited_since.id,
                "done",
                input_from_entity(&entity_feature(edited_since.id, "Older text", None, None)),
                json!({}),
            );
            let stored = vec![
                done_ops(&current),
                stale,
                judgment(
                    pending.id,
                    "pending",
                    input_from_entity(&pending),
                    json!({}),
                ),
            ];
            let (service, upserts) = service(true, stored);
            let features = vec![
                current,
                edited_since.clone(),
                never_judged.clone(),
                pending.clone(),
            ];

            let queued = backfill(&service, team(), &features).await;

            assert_eq!(queued, 3);
            let submitted: Vec<Uuid> = upserts.lock().unwrap().iter().map(|(id, _)| *id).collect();
            assert_eq!(submitted, [edited_since.id, never_judged.id, pending.id]);
        }
    }
}
