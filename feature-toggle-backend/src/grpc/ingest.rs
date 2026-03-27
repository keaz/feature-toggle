use super::pb;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(super) type EvaluationBatch = Vec<crate::database::feature_evaluation::CreateFeatureEvaluation>;

pub(super) struct EvaluationWriteJob {
    pub evaluations: EvaluationBatch,
    pub completion: tokio::sync::oneshot::Sender<Result<(), String>>,
}

pub(super) const EVALUATION_WRITER_QUEUE_CAPACITY: usize = 2048;
pub(super) const EVALUATION_DURABILITY_ACK_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(5);
pub(super) const PUSH_EVALUATION_DEDUP_TTL: std::time::Duration =
    std::time::Duration::from_secs(60);
pub(super) const PUSH_EVALUATION_DEDUP_MAX_ENTRIES: usize = 4096;

#[derive(Clone)]
pub(super) enum PushEvaluationRequestContext {
    Empty,
    Values(std::collections::BTreeMap<String, String>),
}

impl PushEvaluationRequestContext {
    pub(super) fn from_proto(context: &[pb::Context]) -> Self {
        if context.is_empty() {
            return Self::Empty;
        }

        // Canonicalize by key so the fingerprint is stable even when the wire order changes.
        Self::Values(
            context
                .iter()
                .map(|entry| (entry.key.clone(), entry.value.clone()))
                .collect(),
        )
    }

    pub(super) fn to_json_value(&self) -> Option<serde_json::Value> {
        match self {
            Self::Empty => None,
            Self::Values(entries) => {
                Some(serde_json::to_value(entries).unwrap_or(serde_json::Value::Null))
            }
        }
    }
}

#[derive(serde::Serialize)]
struct PushEvaluationRequestFingerprintEvent {
    feature_key: String,
    environment_id: String,
    client_id: String,
    client_secret: String,
    evaluation_result: bool,
    evaluation_context: std::collections::BTreeMap<String, String>,
    user_context: String,
    evaluated_at_unix_ms: i64,
    prior_assignment: bool,
    variant: String,
    variant_value: String,
}

pub(super) fn push_evaluation_request_fingerprint(
    request: &pb::PushEvaluationEventsRequest,
) -> Result<String, serde_json::Error> {
    let events = request
        .events
        .iter()
        .map(|event| PushEvaluationRequestFingerprintEvent {
            feature_key: event.feature_key.clone(),
            environment_id: event.environment_id.clone(),
            client_id: event.client_id.clone(),
            client_secret: event.client_secret.clone(),
            evaluation_result: event.evaluation_result,
            evaluation_context: match PushEvaluationRequestContext::from_proto(
                &event.evaluation_context,
            ) {
                PushEvaluationRequestContext::Empty => std::collections::BTreeMap::new(),
                PushEvaluationRequestContext::Values(entries) => entries,
            },
            user_context: event.user_context.clone(),
            evaluated_at_unix_ms: event.evaluated_at_unix_ms,
            prior_assignment: event.prior_assignment,
            variant: event.variant.clone(),
            variant_value: event.variant_value.clone(),
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&events)?;
    let mut hasher = Sha256::new();
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

struct PushEvaluationDedupeEntries {
    by_fingerprint: std::collections::HashMap<String, std::time::Instant>,
    order: std::collections::VecDeque<(String, std::time::Instant)>,
}

impl PushEvaluationDedupeEntries {
    fn new() -> Self {
        Self {
            by_fingerprint: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    fn prune_expired(&mut self, now: std::time::Instant, ttl: std::time::Duration) {
        loop {
            let Some((fingerprint, seen_at)) = self.order.front() else {
                break;
            };

            let is_current = matches!(self.by_fingerprint.get(fingerprint), Some(current) if *current == *seen_at);
            if !is_current {
                self.order.pop_front();
                continue;
            }

            if now.saturating_duration_since(*seen_at) < ttl {
                break;
            }

            let stale_fingerprint = fingerprint.clone();
            self.by_fingerprint.remove(&stale_fingerprint);
            self.order.pop_front();
        }
    }

    fn enforce_bound(&mut self, max_entries: usize) {
        while self.by_fingerprint.len() > max_entries {
            let Some((fingerprint, seen_at)) = self.order.pop_front() else {
                break;
            };
            let is_current = matches!(self.by_fingerprint.get(&fingerprint), Some(current) if *current == seen_at);
            if is_current {
                self.by_fingerprint.remove(&fingerprint);
            }
        }
    }
}

/// Dedupes accepted evaluation push payloads for a short TTL window.
pub(super) struct PushEvaluationRequestDeduper {
    ttl: std::time::Duration,
    max_entries: usize,
    entries: tokio::sync::Mutex<PushEvaluationDedupeEntries>,
}

impl PushEvaluationRequestDeduper {
    pub(super) fn new(ttl: std::time::Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries,
            entries: tokio::sync::Mutex::new(PushEvaluationDedupeEntries::new()),
        }
    }

    pub(super) async fn contains_recent(&self, fingerprint: &str) -> bool {
        let mut guard = self.entries.lock().await;
        let now = std::time::Instant::now();
        guard.prune_expired(now, self.ttl);
        guard.by_fingerprint.contains_key(fingerprint)
    }

    pub(super) async fn remember(&self, fingerprint: String) {
        let mut guard = self.entries.lock().await;
        let now = std::time::Instant::now();
        guard.prune_expired(now, self.ttl);
        guard.by_fingerprint.insert(fingerprint.clone(), now);
        guard.order.push_back((fingerprint, now));
        guard.enforce_bound(self.max_entries);
    }
}

pub(super) type RequestedKeyMap =
    std::collections::HashMap<Uuid, std::collections::HashSet<String>>;
