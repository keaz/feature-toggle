use super::{AppState, UserAssignment, assignment_key, backoff, is_transient, pb};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
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

/// Flush queued sticky user-assignment writes. Failed batches are requeued so
/// the edge does not drop locally observed assignments during transient outages.
/// Rows the backend permanently rejects (`PermissionDenied`) are dropped. The
/// queue is bounded: assignments that do not fit, new or requeued, are
/// dropped and reported here.
pub async fn run_flush_task(app: AppState) {
    let batch_size = app.assignment_flush_batch_size.max(1);
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

        let mut total_unique = 0usize;
        let mut total_drained = 0usize;
        let mut batches = 0usize;
        let mut failed = false;

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

            total_drained += drained;
            let assignments: Vec<UserAssignment> = dedup.into_values().collect();
            let assignment_count = assignments.len();

            let stream = assignment_stream(&app, assignments.clone());

            let mut client = {
                let guard = app.grpc.lock().await;
                guard.clone()
            };

            match client.push_user_assignments(stream).await {
                Ok(_) => {
                    total_unique += assignment_count;
                    batches += 1;
                }
                Err(e) if e.code() == tonic::Code::PermissionDenied => {
                    warn!(
                        "Backend rejected an assignment batch ({}); pushing rows individually",
                        e.message()
                    );
                    let (stored, requeued) =
                        push_rows_individually(&app, &mut client, assignments).await;
                    total_unique += stored;
                    batches += 1;
                    if requeued {
                        warn!(
                            "Will retry on next flush cycle ({}s)",
                            app.flush_interval.as_secs()
                        );
                        failed = true;
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
                    failed = true;
                    break;
                }
            }
        }

        if !failed && total_unique > 0 {
            info!(
                "Successfully pushed {} user assignments in {} batch(es) ({} drained)",
                total_unique, batches, total_drained
            );
        }
    }
}

/// Flush evaluation events with bounded local buffering. Retries preserve
/// original ordering and drop only the oldest events once the edge-side buffer
/// is already at capacity.
pub async fn run_evaluation_flush_task(
    app: AppState,
    mut event_rx: tokio::sync::mpsc::Receiver<crate::EvaluationEvent>,
) {
    let mut buffer = Vec::new();
    let flush_interval = app.evaluation_flush_interval;
    let max_buffered = app.evaluation_event_queue_capacity.max(1);
    let batch_size = app.evaluation_flush_batch_size.max(1);

    loop {
        tokio::time::sleep(flush_interval).await;

        let dropped = app.evaluation_event_dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            warn!(
                "Dropped {} evaluation events due to full queue (capacity={})",
                dropped, max_buffered
            );
        }

        while buffer.len() < max_buffered {
            match event_rx.try_recv() {
                Ok(event) => buffer.push(event),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        let mut dropped_in_flush = 0u64;
        if buffer.len() >= max_buffered {
            loop {
                match event_rx.try_recv() {
                    Ok(_) => dropped_in_flush += 1,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
        }
        if dropped_in_flush > 0 {
            warn!(
                "Dropped {} evaluation events while draining (buffer full, capacity={})",
                dropped_in_flush, max_buffered
            );
        }

        if buffer.is_empty() {
            continue;
        }

        let mut to_send = std::mem::take(&mut buffer);
        let mut total_sent = 0usize;
        let mut total_processed = 0usize;
        let mut batches = 0usize;
        let mut failed = false;

        while !to_send.is_empty() {
            let chunk = if to_send.len() > batch_size {
                let rest = to_send.split_off(batch_size);
                let chunk = to_send;
                to_send = rest;
                chunk
            } else {
                let chunk = to_send;
                to_send = Vec::new();
                chunk
            };

            let mut proto_events = Vec::with_capacity(chunk.len());
            for event in chunk.iter() {
                let evaluated_at_unix_ms = event
                    .evaluated_at
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                let mut proto_context =
                    Vec::with_capacity(1 + event.evaluation_context.attributes.len());
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
                    total_sent += chunk.len();
                    total_processed += resp.processed_count as usize;
                    batches += 1;
                }
                Err(e) => {
                    error!("Failed to push evaluation events after retries: {}", e);
                    warn!(
                        "Will retry on next flush cycle ({}s)",
                        flush_interval.as_secs()
                    );
                    // Keep original order when requeueing so retries preserve
                    // batch semantics as much as possible. If the local buffer
                    // is already full, we drop the oldest events first.
                    let mut requeue = chunk;
                    requeue.extend(to_send);
                    if requeue.len() > max_buffered {
                        let drop_count = requeue.len() - max_buffered;
                        buffer.extend(requeue.into_iter().skip(drop_count));
                        warn!(
                            "Dropped {} evaluation events while requeueing (buffer limit={})",
                            drop_count, max_buffered
                        );
                    } else {
                        buffer.extend(requeue);
                    }
                    failed = true;
                    break;
                }
            }
        }

        if !failed && total_sent > 0 {
            info!(
                "Successfully pushed {} evaluation events in {} batch(es) ({} processed)",
                total_sent, batches, total_processed
            );
        }
    }
}
