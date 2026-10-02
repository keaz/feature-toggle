//! Natural-language feature search (AI-40): turns a plain-English query into
//! list filters, then reranks the candidates. Both calls are synchronous. See
//! `docs/ai-judgments/design.md` §5.4.

use std::collections::BTreeMap;

use log::{info, warn};
use serde_json::{Value, json};
use uuid::Uuid;

use super::JudgmentClient;
use super::types::{Answers, Question, RequestParts};
use crate::database::entity::ApprovalStatus;
use crate::database::feature::FeatureRepository;
use crate::logic::feature::FeatureLogic;
use crate::model::{Feature, FeatureType, FlagKind, FlagKindFilter, ID, LifecycleStage};

pub use crate::model::FeatureSearchFilters as AppliedFilters;

/// A filter answer below this confidence is ignored.
pub const MIN_FILTER_CONFIDENCE: f64 = 0.6;
/// A candidate below this relevance is dropped.
pub const MIN_RELEVANCE: f64 = 0.3;
/// `has_topic` must be above this for the rerank call to run.
pub const TOPIC_THRESHOLD: f64 = 0.5;
/// Tag and owner options per Choice; one more slot is left for `none`.
pub const MAX_FILTER_VALUES: usize = 254;
/// Candidates fetched for the rerank call.
pub const MAX_CANDIDATES: i64 = 50;
pub const MIN_QUERY_CHARS: usize = 3;
pub const MAX_QUERY_CHARS: usize = 300;
pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 20;
/// Longest `description` / `purpose` sent per candidate.
const MAX_CANDIDATE_TEXT_CHARS: usize = 300;
const MAX_CANDIDATE_TAGS: usize = 20;

pub const Q_LIFECYCLE_STAGE: &str = "lifecycle_stage";
pub const Q_STALE: &str = "stale";
pub const Q_EXPIRED: &str = "expired";
pub const Q_FEATURE_TYPE: &str = "feature_type";
pub const Q_DEPENDENCY_STATUS: &str = "dependency_status";
pub const Q_APPROVAL_STATUS: &str = "approval_status";
pub const Q_FLAG_KIND: &str = "flag_kind";
pub const Q_TAG: &str = "tag";
pub const Q_OWNER: &str = "owner";
pub const Q_HAS_TOPIC: &str = "has_topic";

pub const UNSPECIFIED: &str = "unspecified";
pub const NONE: &str = "none";

/// State and questions for call 1.
pub fn build_filter_parts(
    query: &str,
    tags: &[String],
    owners: &[String],
    with_flag_kind: bool,
) -> RequestParts {
    RequestParts {
        state: json!({ "query": query }),
        questions: build_filter_questions(tags, owners, with_flag_kind),
    }
}

/// One Choice per list filter (every one has an `unspecified` or `none`
/// option) plus the `has_topic` Noul. `tags` and `owners` should list the most
/// used values first; at most 254 of each are sent. A Choice for tags or owners
/// is left out when the team has none, since a Choice needs two options.
pub fn build_filter_questions(
    tags: &[String],
    owners: &[String],
    with_flag_kind: bool,
) -> BTreeMap<String, Question> {
    let mut questions = BTreeMap::new();
    let mut add = |id: &str, question: Question| {
        questions.insert(id.to_string(), question);
    };

    add(
        Q_LIFECYCLE_STAGE,
        fixed_choice(
            "Does `query` name a lifecycle stage for the flags, and which one? Choose `unspecified` unless the query says draft, active, deprecated, archived, or a close synonym such as retired.",
            &[
                (
                    "draft",
                    "Flags that are still being prepared and are not live yet.",
                ),
                ("active", "Flags in normal use."),
                ("deprecated", "Flags marked for removal."),
                ("archived", "Flags that were retired and archived."),
            ],
        ),
    );
    add(
        Q_STALE,
        fixed_choice(
            "Does `query` say the flags must be stale or must not be stale? Choose `unspecified` unless the query uses a word such as stale, unused, abandoned, forgotten, cleanup, or still in use. Do not choose `stale` only because the query says expired.",
            &[
                (
                    "stale",
                    "Flags that are unused or forgotten and are candidates for cleanup.",
                ),
                ("not_stale", "Flags that are still in use."),
            ],
        ),
    );
    add(
        Q_EXPIRED,
        fixed_choice(
            "Does `query` say the flags must be expired or must not be expired? Choose `unspecified` unless the query mentions expiry or expiration. Being stale or unused is not the same as being expired.",
            &[
                ("expired", "Flags whose expiry date has passed."),
                (
                    "not_expired",
                    "Flags with no expiry date or an expiry date in the future.",
                ),
            ],
        ),
    );
    add(
        Q_FEATURE_TYPE,
        fixed_choice(
            "Does `query` name a feature type, and which one? Choose `unspecified` unless the query says simple, contextual, or describes targeting by context.",
            &[
                ("simple", "Plain on/off flags without targeting context."),
                (
                    "contextual",
                    "Flags evaluated against user or request context, with targeting rules.",
                ),
            ],
        ),
    );
    add(
        Q_DEPENDENCY_STATUS,
        fixed_choice(
            "Does `query` ask about dependencies between flags? Choose `unspecified` unless the query mentions dependencies or flags that depend on or block other flags.",
            &[
                ("has_dependencies", "Flags that depend on other flags."),
                (
                    "blocked_by_dependencies",
                    "Flags that other flags depend on, so changing them affects those flags.",
                ),
                (
                    "independent",
                    "Flags with no dependencies in either direction.",
                ),
            ],
        ),
    );
    let approval: Vec<(&str, &str)> = ApprovalStatus::ALL
        .iter()
        .map(|status| {
            (
                status.as_str(),
                "Flags that have an approval request in this status.",
            )
        })
        .collect();
    add(
        Q_APPROVAL_STATUS,
        fixed_choice(
            "Does `query` ask for flags that have an approval request in a given status? Choose `unspecified` unless the query mentions approvals or approval requests.",
            &approval,
        ),
    );
    if with_flag_kind {
        let mut kinds: Vec<(&str, &str)> = FlagKind::ALL
            .iter()
            .map(|kind| (kind.as_str(), kind_description(*kind)))
            .collect();
        kinds.push(("unclassified", "Flags that have no kind assigned yet."));
        add(
            Q_FLAG_KIND,
            fixed_choice(
                "Does `query` name a kind of flag, and which one? Choose `unspecified` unless the query says release, experiment, ops, permission, config, or unclassified (or a clear synonym such as kill switch, A/B test, or entitlement).",
                &kinds,
            ),
        );
    }
    if let Some(question) = value_choice(
        "Which of these tags does `query` ask for, for example by naming the tag or its product area? Choose `none` when the query names no tag.",
        "The query names none of these tags.",
        tags,
    ) {
        add(Q_TAG, question);
    }
    if let Some(question) = value_choice(
        "Which of these owners does `query` ask for? Choose `none` when the query names no owner.",
        "The query names none of these owners.",
        owners,
    ) {
        add(Q_OWNER, question);
    }
    add(
        Q_HAS_TOPIC,
        Question::noul(
            "Does `query` describe what the flags are about, such as a product area, feature, or behavior, beyond status filters like stale, expired, archived, owner, tag, flag kind, feature type, dependencies, or approvals?",
        ),
    );
    questions
}

fn kind_description(kind: FlagKind) -> &'static str {
    match kind {
        FlagKind::Release => "Temporary flags that roll out new or changed functionality.",
        FlagKind::Experiment => "Temporary flags for A/B tests and experiments.",
        FlagKind::Ops => {
            "Long-lived operational controls such as kill switches and circuit breakers."
        }
        FlagKind::Permission => {
            "Long-lived entitlements for plans, customers, roles, or beta programs."
        }
        FlagKind::Config => "Long-lived configuration values such as limits, timeouts, or text.",
    }
}

/// A Choice over fixed options plus `unspecified` ("The query does not mention this.").
fn fixed_choice(instructions: &str, options: &[(&str, &str)]) -> Question {
    Question::choice(
        instructions,
        options
            .iter()
            .map(|(name, text)| (name.to_string(), Some(Value::from(*text))))
            .chain(std::iter::once((
                UNSPECIFIED.to_string(),
                Some(Value::from("The query does not mention this.")),
            ))),
    )
    .expect("fixed filter options are within the choice limits")
}

/// A Choice over team values (at most 254, first listed wins) plus `none`.
/// Blank values, repeats, and any value spelled like the `none` option are skipped.
fn value_choice(instructions: &str, none_text: &str, values: &[String]) -> Option<Question> {
    let mut seen = std::collections::BTreeSet::new();
    let kept: Vec<&str> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case(NONE))
        .filter(|value| seen.insert(*value))
        .take(MAX_FILTER_VALUES)
        .collect();
    if kept.is_empty() {
        return None;
    }
    Some(
        Question::choice(
            instructions,
            kept.into_iter()
                .map(|value| (value.to_string(), None))
                .chain(std::iter::once((
                    NONE.to_string(),
                    Some(Value::from(none_text)),
                ))),
        )
        .expect("at most 254 values plus none are within the choice limits"),
    )
}

/// The chosen option, when it is not a "nothing chosen" option and its
/// confidence reaches the threshold.
fn confident_choice<'a>(answers: &'a Answers, id: &str) -> Option<&'a str> {
    let answer = answers.choice(id)?;
    if answer.confidence < MIN_FILTER_CONFIDENCE {
        return None;
    }
    let value = answer.choice.as_str();
    (value != UNSPECIFIED && value != NONE).then_some(value)
}

/// The filters to apply: only confident answers that name an option. A value
/// the code does not recognise is ignored.
pub fn derive_filters(answers: &Answers) -> AppliedFilters {
    AppliedFilters {
        lifecycle_stage: confident_choice(answers, Q_LIFECYCLE_STAGE).and_then(
            |value| match value {
                "draft" => Some(LifecycleStage::Draft),
                "active" => Some(LifecycleStage::Active),
                "deprecated" => Some(LifecycleStage::Deprecated),
                "archived" => Some(LifecycleStage::Archived),
                _ => None,
            },
        ),
        stale: confident_choice(answers, Q_STALE).and_then(|value| match value {
            "stale" => Some(true),
            "not_stale" => Some(false),
            _ => None,
        }),
        expired: confident_choice(answers, Q_EXPIRED).and_then(|value| match value {
            "expired" => Some(true),
            "not_expired" => Some(false),
            _ => None,
        }),
        feature_type: confident_choice(answers, Q_FEATURE_TYPE).and_then(|value| match value {
            "simple" => Some(FeatureType::Simple),
            "contextual" => Some(FeatureType::Contextual),
            _ => None,
        }),
        dependency_status: confident_choice(answers, Q_DEPENDENCY_STATUS)
            .filter(|value| {
                matches!(
                    *value,
                    "has_dependencies" | "blocked_by_dependencies" | "independent"
                )
            })
            .map(str::to_string),
        approval_status: confident_choice(answers, Q_APPROVAL_STATUS)
            .filter(|value| {
                ApprovalStatus::ALL
                    .iter()
                    .any(|status| status.as_str() == *value)
            })
            .map(str::to_string),
        flag_kind: confident_choice(answers, Q_FLAG_KIND)
            .and_then(|value| value.parse::<FlagKindFilter>().ok()),
        tag: confident_choice(answers, Q_TAG).map(str::to_string),
        owner: confident_choice(answers, Q_OWNER).map(str::to_string),
    }
}

/// Whether the query is about a topic, so the candidates are worth reranking.
pub fn wants_rerank(answers: &Answers) -> bool {
    answers
        .noul(Q_HAS_TOPIC)
        .is_some_and(|p| p > TOPIC_THRESHOLD)
}

fn truncated(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    (!text.is_empty()).then(|| text.chars().take(MAX_CANDIDATE_TEXT_CHARS).collect())
}

/// State and questions for call 2. Candidate `i` is answered by `c{i}`.
pub fn build_rerank_parts(query: &str, candidates: &[Feature]) -> RequestParts {
    let candidates_state: Vec<Value> = candidates
        .iter()
        .map(|feature| {
            json!({
                "key": feature.key,
                "description": truncated(feature.description.as_deref()),
                "purpose": truncated(feature.purpose.as_deref()),
                "tags": feature.tags.iter().take(MAX_CANDIDATE_TAGS).collect::<Vec<_>>(),
            })
        })
        .collect();
    RequestParts {
        state: json!({ "query": query, "candidates": candidates_state }),
        questions: build_rerank_questions(candidates.len()),
    }
}

fn candidate_id(index: usize) -> String {
    format!("c{index}")
}

pub fn build_rerank_questions(n: usize) -> BTreeMap<String, Question> {
    (0..n)
        .map(|index| {
            (
                candidate_id(index),
                Question::noul(format!(
                    "Is `candidates[{index}]` a flag that `query` is looking for?"
                )),
            )
        })
        .collect()
}

/// Candidates with relevance at least [`MIN_RELEVANCE`], best first (ties keep
/// the candidate order), at most `limit`. A candidate without an answer is dropped.
pub fn rank(candidates: Vec<Feature>, answers: &Answers, limit: usize) -> Vec<(Feature, f64)> {
    let mut ranked: Vec<(Feature, f64)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, feature)| {
            let relevance = answers.noul(&candidate_id(index))?;
            (relevance >= MIN_RELEVANCE).then_some((feature, relevance))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(limit);
    ranked
}

/// The result of one search.
#[derive(Debug)]
pub struct SearchOutcome {
    pub filters: AppliedFilters,
    /// The raw answers to call 1, for tuning.
    pub filter_answers: Answers,
    /// Relevance is `None` when the query named no topic and nothing was reranked.
    pub results: Vec<(Feature, Option<f64>)>,
    /// TypeSafe calls made: 1 or 2.
    pub calls: u32,
    pub input_tokens: u64,
}

#[derive(Debug)]
pub enum SearchError {
    /// A TypeSafe call failed, or the tag and owner values could not be read.
    /// Callers answer `available: false`.
    Unavailable,
    /// The candidate query failed. A server error, not an AI one.
    Candidates(crate::Error),
}

/// Runs both calls: map the query to filters, fetch candidates busiest first,
/// and rerank them when the query names a topic. Logs call and token counts,
/// never the query.
pub async fn search(
    client: &dyn JudgmentClient,
    features: &dyn FeatureRepository,
    logic: &dyn FeatureLogic,
    team_id: Uuid,
    query: &str,
    limit: usize,
) -> Result<SearchOutcome, SearchError> {
    let value_limit = MAX_FILTER_VALUES as i64;
    let (tags, owners) = match (
        features
            .get_team_tag_candidates(team_id, Vec::new(), value_limit)
            .await,
        features
            .get_team_owner_candidates(team_id, value_limit)
            .await,
    ) {
        (Ok(tags), Ok(owners)) => (tags, owners),
        _ => {
            warn!("Could not read tag and owner values for team {team_id}");
            return Err(SearchError::Unavailable);
        }
    };

    let mut calls = 1;
    let parts = build_filter_parts(query, &tags, &owners, true);
    let filter_response = client
        .evaluate(parts.state, parts.questions)
        .await
        .map_err(|err| {
            warn!("Natural-language search failed: {}", err.log_label());
            SearchError::Unavailable
        })?;
    let mut input_tokens = u64::from(filter_response.usage.input_tokens);
    let filters = derive_filters(&filter_response.answers);

    let candidates = logic
        .search_features_by_usage(ID::from(team_id), filters.clone(), MAX_CANDIDATES)
        .await
        .map_err(SearchError::Candidates)?;

    let results = if wants_rerank(&filter_response.answers) && !candidates.is_empty() {
        let parts = build_rerank_parts(query, &candidates);
        calls += 1;
        let rerank_response = client
            .evaluate(parts.state, parts.questions)
            .await
            .map_err(|err| {
                warn!("Natural-language search failed: {}", err.log_label());
                SearchError::Unavailable
            })?;
        input_tokens += u64::from(rerank_response.usage.input_tokens);
        rank(candidates, &rerank_response.answers, limit)
            .into_iter()
            .map(|(feature, relevance)| (feature, Some(relevance)))
            .collect()
    } else {
        candidates
            .into_iter()
            .take(limit)
            .map(|feature| (feature, None))
            .collect()
    };
    info!("Natural-language search used {calls} TypeSafe call(s) and {input_tokens} input tokens");

    Ok(SearchOutcome {
        filters,
        filter_answers: filter_response.answers,
        results,
        calls,
        input_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judgment::types::{Answer, ChoiceAnswer};
    use chrono::Utc;
    use uuid::Uuid;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn options(question: &Question) -> Vec<String> {
        match question {
            Question::Choice { criteria, .. } => criteria.keys().cloned().collect(),
            other => panic!("expected a choice, got {other:?}"),
        }
    }

    fn sorted(values: &[&str]) -> Vec<String> {
        let mut values = names(values);
        values.sort();
        values
    }

    fn choice(value: &str, confidence: f64) -> Answer {
        Answer::Choice(ChoiceAnswer {
            choice: value.to_string(),
            probabilities: BTreeMap::new(),
            confidence,
        })
    }

    fn answers(items: Vec<(&str, Answer)>) -> Answers {
        Answers(
            items
                .into_iter()
                .map(|(id, answer)| (id.to_string(), answer))
                .collect(),
        )
    }

    fn feature(key: &str) -> Feature {
        Feature {
            id: Uuid::new_v4().into(),
            key: key.to_string(),
            description: Some(format!("About {key}")),
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
            owner: None,
            purpose: Some("Purpose".to_string()),
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: names(&["a", "b"]),
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
            team_id: Uuid::new_v4().into(),
            pending_approval_request_id: None,
            flag_kind: None,
            flag_kind_source: None,
            flag_kind_confidence: None,
        }
    }

    #[test]
    fn filter_questions_have_the_documented_options() {
        let questions = build_filter_questions(&names(&["payments", "ui"]), &names(&["ann"]), true);
        let option_set = |id: &str| options(&questions[id]);
        assert_eq!(
            option_set(Q_LIFECYCLE_STAGE),
            sorted(&["draft", "active", "deprecated", "archived", "unspecified"])
        );
        assert_eq!(
            option_set(Q_STALE),
            sorted(&["stale", "not_stale", "unspecified"])
        );
        assert_eq!(
            option_set(Q_EXPIRED),
            sorted(&["expired", "not_expired", "unspecified"])
        );
        assert_eq!(
            option_set(Q_FEATURE_TYPE),
            sorted(&["simple", "contextual", "unspecified"])
        );
        assert_eq!(
            option_set(Q_DEPENDENCY_STATUS),
            sorted(&[
                "has_dependencies",
                "blocked_by_dependencies",
                "independent",
                "unspecified"
            ])
        );
        assert_eq!(
            option_set(Q_APPROVAL_STATUS),
            sorted(&[
                "pending",
                "approved",
                "rejected",
                "cancelled",
                "auto_approved",
                "unspecified"
            ])
        );
        assert_eq!(
            option_set(Q_FLAG_KIND),
            sorted(&[
                "release",
                "experiment",
                "ops",
                "permission",
                "config",
                "unclassified",
                "unspecified"
            ])
        );
        assert_eq!(option_set(Q_TAG), sorted(&["none", "payments", "ui"]));
        assert_eq!(option_set(Q_OWNER), sorted(&["none", "ann"]));
        assert!(matches!(questions[Q_HAS_TOPIC], Question::Noul { .. }));
        assert_eq!(questions.len(), 10);
    }

    #[test]
    fn approval_options_cover_every_status_the_filter_accepts() {
        // Exhaustive match: a new status fails to compile here until it is handled.
        for status in ApprovalStatus::ALL {
            match status {
                ApprovalStatus::Pending
                | ApprovalStatus::Approved
                | ApprovalStatus::Rejected
                | ApprovalStatus::Cancelled
                | ApprovalStatus::AutoApproved => {}
            }
        }
        let questions = build_filter_questions(&[], &[], false);
        let listed = options(&questions[Q_APPROVAL_STATUS]);
        for status in ApprovalStatus::ALL {
            assert!(listed.contains(&status.as_str().to_string()));
        }
    }

    #[test]
    fn flag_kind_question_is_left_out_when_not_requested() {
        let questions = build_filter_questions(&names(&["a"]), &names(&["b"]), false);
        assert!(!questions.contains_key(Q_FLAG_KIND));
        assert_eq!(questions.len(), 9);
    }

    #[test]
    fn every_filter_choice_has_a_none_or_unspecified_option() {
        let questions = build_filter_questions(&names(&["a"]), &names(&["b"]), true);
        for (id, question) in &questions {
            if let Question::Choice { criteria, .. } = question {
                assert!(
                    criteria.contains_key(UNSPECIFIED) || criteria.contains_key(NONE),
                    "{id} has no unspecified/none option"
                );
                assert!(criteria.len() <= 255, "{id} exceeds 255 options");
            }
        }
    }

    #[test]
    fn tags_and_owners_are_capped_at_254_most_used_first() {
        let tags: Vec<String> = (0..400).map(|i| format!("tag-{i:03}")).collect();
        let owners: Vec<String> = (0..300).map(|i| format!("owner-{i:03}")).collect();
        let questions = build_filter_questions(&tags, &owners, true);
        for (id, first, dropped) in [
            (Q_TAG, "tag-000", "tag-254"),
            (Q_OWNER, "owner-000", "owner-254"),
        ] {
            let listed = options(&questions[id]);
            assert_eq!(listed.len(), 255, "{id}: 254 values plus none");
            assert!(listed.contains(&first.to_string()));
            assert!(!listed.contains(&dropped.to_string()));
            assert!(listed.contains(&NONE.to_string()));
        }
    }

    #[test]
    fn blank_duplicate_and_sentinel_values_are_skipped() {
        let questions = build_filter_questions(
            &names(&["payments", " ", "payments", "None", "ui"]),
            &[],
            false,
        );
        assert_eq!(
            options(&questions[Q_TAG]),
            sorted(&["none", "payments", "ui"])
        );
    }

    #[test]
    fn tag_and_owner_questions_are_left_out_without_values() {
        let questions = build_filter_questions(&[], &[], true);
        assert!(!questions.contains_key(Q_TAG));
        assert!(!questions.contains_key(Q_OWNER));
    }

    #[test]
    fn filter_instructions_name_the_query() {
        let questions = build_filter_questions(&names(&["a"]), &names(&["b"]), true);
        for (id, question) in &questions {
            let instructions = match question {
                Question::Choice { instructions, .. } | Question::Noul { instructions, .. } => {
                    instructions.as_str().unwrap().to_string()
                }
                other => panic!("{id}: unexpected {other:?}"),
            };
            assert!(instructions.contains("`query`"), "{id}: {instructions}");
        }
    }

    #[test]
    fn filter_parts_state_carries_only_the_query() {
        let parts = build_filter_parts("stale flags", &[], &[], false);
        assert_eq!(parts.state, json!({ "query": "stale flags" }));
    }

    #[test]
    fn derive_applies_confident_answers() {
        let filters = derive_filters(&answers(vec![
            (Q_LIFECYCLE_STAGE, choice("archived", 0.9)),
            (Q_STALE, choice("stale", 0.8)),
            (Q_EXPIRED, choice("not_expired", 0.7)),
            (Q_FEATURE_TYPE, choice("contextual", 0.6)),
            (Q_DEPENDENCY_STATUS, choice("independent", 0.9)),
            (Q_APPROVAL_STATUS, choice("auto_approved", 0.9)),
            (Q_FLAG_KIND, choice("ops", 0.9)),
            (Q_TAG, choice("payments", 0.95)),
            (Q_OWNER, choice("ann", 0.95)),
        ]));
        assert_eq!(
            filters,
            AppliedFilters {
                lifecycle_stage: Some(LifecycleStage::Archived),
                stale: Some(true),
                expired: Some(false),
                feature_type: Some(FeatureType::Contextual),
                dependency_status: Some("independent".into()),
                approval_status: Some("auto_approved".into()),
                flag_kind: Some(FlagKindFilter::Kind(FlagKind::Ops)),
                tag: Some("payments".into()),
                owner: Some("ann".into()),
            }
        );
    }

    #[test]
    fn derive_maps_the_negative_booleans() {
        let filters = derive_filters(&answers(vec![
            (Q_STALE, choice("not_stale", 0.9)),
            (Q_EXPIRED, choice("expired", 0.9)),
            (Q_FLAG_KIND, choice("unclassified", 0.9)),
        ]));
        assert_eq!(filters.stale, Some(false));
        assert_eq!(filters.expired, Some(true));
        assert_eq!(filters.flag_kind, Some(FlagKindFilter::Unclassified));
    }

    #[test]
    fn derive_ignores_low_confidence_unspecified_none_and_unknown_values() {
        let filters = derive_filters(&answers(vec![
            (Q_LIFECYCLE_STAGE, choice("archived", 0.59)),
            (Q_STALE, choice("unspecified", 0.99)),
            (Q_TAG, choice("none", 0.99)),
            (Q_OWNER, choice("none", 0.99)),
            (Q_FEATURE_TYPE, choice("hybrid", 0.99)),
            (Q_FLAG_KIND, choice("mystery", 0.99)),
            (Q_EXPIRED, Answer::Noul { noul: 0.99 }),
        ]));
        assert_eq!(filters, AppliedFilters::default());
        assert_eq!(
            derive_filters(&Answers::default()),
            AppliedFilters::default()
        );
    }

    #[test]
    fn derive_applies_a_filter_at_exactly_the_threshold() {
        let filters = derive_filters(&answers(vec![(Q_STALE, choice("stale", 0.6))]));
        assert_eq!(filters.stale, Some(true));
    }

    #[test]
    fn rerank_runs_only_above_the_topic_threshold() {
        let with = |value: f64| answers(vec![(Q_HAS_TOPIC, Answer::Noul { noul: value })]);
        assert!(wants_rerank(&with(0.51)));
        assert!(!wants_rerank(&with(0.5)));
        assert!(!wants_rerank(&with(0.1)));
        assert!(!wants_rerank(&Answers::default()));
    }

    #[test]
    fn rerank_questions_are_one_noul_per_candidate() {
        let questions = build_rerank_questions(3);
        assert_eq!(
            questions.keys().cloned().collect::<Vec<_>>(),
            names(&["c0", "c1", "c2"])
        );
        match &questions["c2"] {
            Question::Noul { instructions, .. } => assert_eq!(
                instructions.as_str().unwrap(),
                "Is `candidates[2]` a flag that `query` is looking for?"
            ),
            other => panic!("expected a noul, got {other:?}"),
        }
        assert!(build_rerank_questions(0).is_empty());
    }

    #[test]
    fn rerank_state_lists_key_description_purpose_and_tags() {
        let mut long = feature("long");
        long.description = Some("x".repeat(1000));
        let parts = build_rerank_parts("checkout", &[feature("a"), long]);
        assert_eq!(parts.state["query"], "checkout");
        assert_eq!(
            parts.state["candidates"][0],
            json!({
                "key": "a",
                "description": "About a",
                "purpose": "Purpose",
                "tags": ["a", "b"]
            })
        );
        assert_eq!(
            parts.state["candidates"][1]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            MAX_CANDIDATE_TEXT_CHARS
        );
        assert_eq!(parts.questions.len(), 2);
    }

    fn relevance(values: &[f64]) -> Answers {
        answers(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    (
                        // leak is fine in a test: ids must be &str
                        Box::leak(format!("c{index}").into_boxed_str()) as &str,
                        Answer::Noul { noul: *value },
                    )
                })
                .collect(),
        )
    }

    fn keys(ranked: &[(Feature, f64)]) -> Vec<String> {
        ranked
            .iter()
            .map(|(feature, _)| feature.key.clone())
            .collect()
    }

    #[test]
    fn rank_drops_below_threshold_sorts_descending_and_truncates() {
        let candidates = vec![feature("a"), feature("b"), feature("c"), feature("d")];
        let ranked = rank(candidates.clone(), &relevance(&[0.31, 0.2, 0.9, 0.3]), 10);
        assert_eq!(keys(&ranked), names(&["c", "a", "d"]));
        assert_eq!(ranked[0].1, 0.9);

        let truncated = rank(candidates, &relevance(&[0.31, 0.2, 0.9, 0.3]), 2);
        assert_eq!(keys(&truncated), names(&["c", "a"]));
    }

    #[test]
    fn rank_keeps_candidate_order_on_ties_and_skips_missing_answers() {
        let candidates = vec![feature("a"), feature("b"), feature("c")];
        let mut partial = relevance(&[0.7, 0.7]);
        partial.0.remove("c1");
        let ranked = rank(candidates, &partial, 10);
        assert_eq!(keys(&ranked), names(&["a"]));

        let candidates = vec![feature("a"), feature("b")];
        let ranked = rank(candidates, &relevance(&[0.7, 0.7]), 10);
        assert_eq!(keys(&ranked), names(&["a", "b"]));
    }
}
