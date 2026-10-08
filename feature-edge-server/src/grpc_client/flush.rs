use super::{AppState, UserAssignment, assignment_key, backoff, is_transient, pb};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio_retry::RetryIf;
use tracing::{error, info, warn};

/// Build the client-streaming request body for an assignment push. Only the
/// first message carries the edge credentials.
fn assignment_stream(
    app: &AppState,
    assignments: Vec<UserAssignment>,
) -> impl tokio_stream::Stream<Item = pb::UserFlagAssignment> + use<> {
    let client_id = app.client_id.clone();
    let client_secret = app.client_secret.clone();
    tokio_stream::iter(assignments.into_iter().enumerate().map(move |(idx, a)| {
        pb::UserFlagAssignment {
            user_id: a.user_id,
            feature_id: a.feature_id,
            environment_id: a.environment_id,
            assigned: a.assigned,
            client_id: if idx == 0 {
                client_id.clone()
            } else {
                String::new()
            },
            client_secret: if idx == 0 {
                client_secret.clone()
            } else {
                String::new()
            },
            variant: a.variant.unwrap_or_default(),
        }
    }))
}

/// The backend rejected a batch with `PermissionDenied`: at least one row names
/// a feature or environment outside the edge client's team (for example an
/// assignment of an SDK client from another team, or of a deleted feature).
/// Retrying the same batch can never succeed, so push the rows one by one,
/// drop the rows that are rejected again and requeue rows that hit any other
/// error. Returns the number of rows stored and whether any row was requeued.
async fn push_rows_individually(
    app: &AppState,
    client: &mut pb::feature_evaluation_client::FeatureEvaluationClient<tonic::transport::Channel>,
    assignments: Vec<UserAssignment>,
) -> (usize, bool) {
    let mut stored = 0usize;
    let mut requeued = false;
    for assignment in assignments {
        match client
            .push_user_assignments(assignment_stream(app, vec![assignment.clone()]))
            .await
        {
            Ok(_) => stored += 1,
            Err(e) if e.code() == tonic::Code::PermissionDenied => {
                warn!(
                    "Dropping user assignment (user '{}', feature '{}', environment '{}') rejected by backend: {}",
                    assignment.user_id,
                    assignment.feature_id,
                    assignment.environment_id,
                    e.message()
                );
            }
            Err(e) => {
                error!("Failed to push user assignment: {}", e);
                app.pending_assignments.push(assignment);
                requeued = true;
            }
        }
    }
    (stored, requeued)
}

/// Outcome of one assignment flush cycle.
#[derive(Debug, Default)]
pub(crate) struct AssignmentFlush {
    /// Unique assignments the backend stored.
    pub pushed: usize,
    /// Assignments popped from the queue, before deduplication.
    pub drained: usize,
    pub batches: usize,
    /// A batch failed and was requeued for the next cycle.
    pub failed: bool,
}

/// Push everything queued now in batches of `assignment_flush_batch_size`.
/// Failed batches are requeued and end the cycle.
pub(crate) async fn flush_assignments_once(app: &AppState) -> AssignmentFlush {
    let batch_size = app.assignment_flush_batch_size.max(1);
    let mut flush = AssignmentFlush::default();

    loop {
        let mut drained = 0usize;
        let mut dedup: HashMap<String, UserAssignment> = HashMap::new();

        while drained < batch_size {
            match app.pending_assignments.pop() {
                Some(assignment) => {
                    drained += 1;
                    let key = assignment_key(
                        &assignment.user_id,
                        &assignment.feature_id,
                        &assignment.environment_id,
                    );
                    dedup.insert(key, assignment);
                }
                None => break,
            }
        }

        if dedup.is_empty() {
            break;
        }

        flush.drained += drained;
        let assignments: Vec<UserAssignment> = dedup.into_values().collect();
        let assignment_count = assignments.len();

        let stream = assignment_stream(app, assignments.clone());

        let mut client = {
            let guard = app.grpc.lock().await;
            guard.clone()
        };

        match client.push_user_assignments(stream).await {
            Ok(_) => {
                flush.pushed += assignment_count;
                flush.batches += 1;
            }
            Err(e) if e.code() == tonic::Code::PermissionDenied => {
                warn!(
                    "Backend rejected an assignment batch ({}); pushing rows individually",
                    e.message()
                );
                let (stored, requeued) =
                    push_rows_individually(app, &mut client, assignments).await;
                flush.pushed += stored;
                flush.batches += 1;
                if requeued {
                    warn!(
                        "Will retry on next flush cycle ({}s)",
                        app.flush_interval.as_secs()
                    );
                    flush.failed = true;
                    break;
                }
            }
            Err(e) => {
                error!("Failed to push user assignments: {}", e);
                warn!(
                    "Will retry on next flush cycle ({}s)",
                    app.flush_interval.as_secs()
                );
                // Requeued under the current generation: a purge while
                // the batch was in flight does not drop it.
                for assignment in assignments {
                    app.pending_assignments.push(assignment);
                }
                flush.failed = true;
                break;
            }
        }
    }
    flush
}

/// Flush queued sticky user-assignment writes. Failed batches are requeued so
/// the edge does not drop locally observed assignments during transient outages.
/// Rows the backend permanently rejects (`PermissionDenied`) are dropped. The
/// queue is bounded: assignments that do not fit, new or requeued, are
/// dropped and reported here.
pub async fn run_flush_task(app: AppState) {
    loop {
        tokio::time::sleep(app.flush_interval).await;

        let dropped = app.pending_assignments.take_dropped();
        if dropped > 0 {
            warn!(
                "Dropped {} user assignments due to full queue (capacity={})",
                dropped,
                app.pending_assignments.capacity()
            );
        }

        let started = Instant::now();
        let flush = flush_assignments_once(&app).await;
        if !flush.failed && flush.pushed > 0 {
            info!(
                "Successfully pushed {} user assignments in {} batch(es) ({} drained) in {} ms",
                flush.pushed,
                flush.batches,
                flush.drained,
                started.elapsed().as_millis()
            );
        }
    }
}

/// Outcome of one or more evaluation pushes.
#[derive(Debug, Default)]
pub(crate) struct EvaluationFlush {
    pub sent: usize,
    /// Events the backend reported as processed.
    pub processed: usize,
    pub batches: usize,
    /// A batch failed after retries; it and the rest were put back in the buffer.
    pub failed: bool,
    /// Events dropped because a failed batch did not fit back in the buffer.
    pub dropped: usize,
    /// Time spent pushing, including building the requests.
    pub elapsed: Duration,
    /// Time spent converting events to request messages, on the runtime thread.
    pub build: Duration,
}

impl EvaluationFlush {
    fn add(&mut self, other: EvaluationFlush) {
        self.sent += other.sent;
        self.processed += other.processed;
        self.batches += other.batches;
        self.failed |= other.failed;
        self.dropped += other.dropped;
        self.elapsed += other.elapsed;
        self.build += other.build;
    }
}

/// Which buffered events a push sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PushScope {
    /// Only whole batches; a partial batch stays buffered.
    FullBatches,
    /// Everything buffered, including a final partial batch.
    All,
}

/// Convert events to the request message. Runs synchronously on the caller's
/// thread.
fn evaluation_events_to_proto(
    app: &AppState,
    events: &[crate::EvaluationEvent],
) -> Vec<pb::FeatureEvaluationEvent> {
    let mut proto_events = Vec::with_capacity(events.len());
    for event in events {
        let evaluated_at_unix_ms = event
            .evaluated_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let mut proto_context = Vec::with_capacity(1 + event.evaluation_context.attributes.len());
        proto_context.push(pb::Context {
            key: "bucketingKey".to_string(),
            value: event.evaluation_context.bucketing_key.clone(),
        });

        for (key, value) in &event.evaluation_context.attributes {
            let value_str = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
                _ => value.to_string(),
            };
            proto_context.push(pb::Context {
                key: key.clone(),
                value: value_str,
            });
        }

        proto_events.push(pb::FeatureEvaluationEvent {
            feature_key: event.feature_key.clone(),
            environment_id: event.environment_id.clone(),
            client_id: app.client_id.clone(),
            client_secret: app.client_secret.clone(),
            evaluation_result: event.evaluation_result,
            evaluation_context: proto_context,
            user_context: event.user_context.clone().unwrap_or_default(),
            evaluated_at_unix_ms,
            prior_assignment: event.prior_assignment,
            variant: event.variant.clone().unwrap_or_default(),
            variant_value: event
                .variant_value
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default(),
        });
    }
    proto_events
}

/// Evaluation events per gRPC request. A batch never exceeds the buffer
/// capacity, so a full buffer is always at least one whole batch.
fn evaluation_batch_size(app: &AppState) -> usize {
    app.evaluation_flush_batch_size
        .max(1)
        .min(app.evaluation_event_queue_capacity.max(1))
}

/// Push buffered events in batches of `evaluation_batch_size`, oldest first.
/// With [`PushScope::FullBatches`] a trailing partial batch stays in `buffer`.
/// A batch that still fails after retries goes back to the front of `buffer`
/// with every event after it, and the push stops. If they no longer fit in
/// `evaluation_event_queue_capacity`, the oldest are dropped.
pub(crate) async fn push_evaluation_batches(
    app: &AppState,
    buffer: &mut Vec<crate::EvaluationEvent>,
    scope: PushScope,
) -> EvaluationFlush {
    let max_buffered = app.evaluation_event_queue_capacity.max(1);
    let batch_size = evaluation_batch_size(app);
    let mut flush = EvaluationFlush::default();

    let sendable = match scope {
        PushScope::All => buffer.len(),
        PushScope::FullBatches => buffer.len() - buffer.len() % batch_size,
    };
    if sendable == 0 {
        return flush;
    }
    let started = Instant::now();
    let rest = buffer.split_off(sendable);
    let mut to_send = std::mem::replace(buffer, rest).into_iter();

    loop {
        let chunk: Vec<crate::EvaluationEvent> = to_send.by_ref().take(batch_size).collect();
        if chunk.is_empty() {
            break;
        }

        let build_started = Instant::now();
        let proto_events = evaluation_events_to_proto(app, &chunk);
        flush.build += build_started.elapsed();

        let retry_strategy = backoff(&app.retry_config);
        let action = || async {
            let mut client = {
                let guard = app.grpc.lock().await;
                guard.clone()
            };
            let req = pb::PushEvaluationEventsRequest {
                events: proto_events.clone(),
            };
            client.push_evaluation_events(req).await
        };

        match RetryIf::spawn(retry_strategy, action, is_transient).await {
            Ok(response) => {
                let resp = response.into_inner();
                flush.sent += chunk.len();
                flush.processed += resp.processed_count as usize;
                flush.batches += 1;
            }
            Err(e) => {
                error!("Failed to push evaluation events after retries: {}", e);
                warn!(
                    "Will retry on next flush cycle ({}s)",
                    app.evaluation_flush_interval.as_secs()
                );
                // Requeue in the original order, ahead of the events buffered
                // after them. Drop the oldest when they no longer fit.
                let mut requeue = chunk;
                requeue.extend(to_send);
                requeue.append(buffer);
                let drop_count = requeue.len().saturating_sub(max_buffered);
                if drop_count > 0 {
                    requeue.drain(..drop_count);
                    warn!(
                        "Dropped {} evaluation events while requeueing (buffer limit={})",
                        drop_count, max_buffered
                    );
                }
                *buffer = requeue;
                flush.dropped = drop_count;
                flush.failed = true;
                break;
            }
        }
    }
    flush.elapsed = started.elapsed();
    flush
}

/// Send evaluation events to the backend as they arrive, so the volume the
/// edge can record is limited by push throughput, not by the queue capacity
/// per flush interval.
///
/// Events are received into a local buffer and pushed as soon as a whole
/// batch of `evaluation_flush_batch_size` is buffered. Every
/// `evaluation_flush_interval` the task also pushes a partial batch, so an
/// event waits at most about one interval, and logs what it pushed and
/// dropped since the last interval. After a push fails, the task waits for
/// the next interval before pushing again; the buffer keeps filling up to
/// `evaluation_event_queue_capacity`, then the channel fills and handlers
/// drop new events (counted in `evaluation_event_dropped`).
///
/// Memory stays bounded: at most `evaluation_event_queue_capacity` events in
/// the channel plus as many in the buffer and the batches in flight.
///
/// Returns when every sender is dropped, after pushing what is buffered.
pub async fn run_evaluation_flush_task(
    app: AppState,
    mut event_rx: tokio::sync::mpsc::Receiver<crate::EvaluationEvent>,
) {
    let max_buffered = app.evaluation_event_queue_capacity.max(1);
    let batch_size = evaluation_batch_size(&app);
    // `sleep_until` with a zero period would fire on every loop iteration
    // and starve the receive branch.
    let interval = app.evaluation_flush_interval.max(Duration::from_millis(1));
    let mut buffer: Vec<crate::EvaluationEvent> = Vec::with_capacity(batch_size);
    let mut next_report = tokio::time::Instant::now() + interval;
    let mut report_started = Instant::now();
    let mut report = EvaluationFlush::default();
    let mut dropped_total = 0u64;
    // Set after a failed push: hold whole-batch pushes until the next interval.
    let mut backing_off = false;

    loop {
        let room = max_buffered.saturating_sub(buffer.len());
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(next_report) => {
                let flush = push_evaluation_batches(&app, &mut buffer, PushScope::All).await;
                backing_off = flush.failed;
                report.add(flush);

                let dropped = app.evaluation_event_dropped.swap(0, Ordering::Relaxed)
                    + report.dropped as u64;
                dropped_total += dropped;
                if dropped > 0 {
                    warn!(
                        "Dropped {} evaluation events due to full queue (capacity={}, {} dropped since start)",
                        dropped, max_buffered, dropped_total
                    );
                }
                if report.sent > 0 {
                    info!(
                        "Successfully pushed {} evaluation events in {} batch(es) ({} processed) in the last {} ms: {} ms pushing ({} ms building requests), {} buffered",
                        report.sent,
                        report.batches,
                        report.processed,
                        report_started.elapsed().as_millis(),
                        report.elapsed.as_millis(),
                        report.build.as_millis(),
                        buffer.len()
                    );
                }
                report = EvaluationFlush::default();
                report_started = Instant::now();
                next_report = tokio::time::Instant::now() + interval;
            }
            received = event_rx.recv_many(&mut buffer, room), if room > 0 => {
                if received == 0 {
                    // Every sender is gone: the edge is shutting down.
                    let flush = push_evaluation_batches(&app, &mut buffer, PushScope::All).await;
                    if flush.failed {
                        warn!(
                            "Dropped {} evaluation events at shutdown after a failed push",
                            buffer.len()
                        );
                    }
                    return;
                }
                if !backing_off && buffer.len() >= batch_size {
                    let flush =
                        push_evaluation_batches(&app, &mut buffer, PushScope::FullBatches).await;
                    backing_off = flush.failed;
                    report.add(flush);
                }
            }
        }
    }
}
