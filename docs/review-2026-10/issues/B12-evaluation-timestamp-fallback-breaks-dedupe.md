# B12: Evaluation events with a missing timestamp get `now()`, which breaks ingest deduplication

| Field | Value |
|---|---|
| Type | Bug (data accuracy) |
| Severity | Low |
| Status | Fixed on branch `fix/b12-evaluation-timestamp-dedupe` |
| Crate | `feature-toggle-backend` |
| Behavior change | Small: events with invalid timestamps are deduplicated correctly |

## Background: original claim was false

The first review claimed that an ingest ack timeout (`ResourceExhausted`) followed by an edge retry double-counts evaluations. **That is false.** Two guards stop it:
- The in-memory deduper (`grpc/mod.rs:1577-1580`) records a request only on success.
- Every row also carries an `ingest_fingerprint` (`database/feature_evaluation.rs:74-92`). The column has a unique index (`migrations/20260326030000_add_ingest_fingerprint_to_feature_evaluations.sql:8-9`), and the insert uses `ON CONFLICT (ingest_fingerprint) DO NOTHING RETURNING *` (`feature_evaluation.rs:290`).
- The edge retry is byte-identical (`feature-edge-server/src/grpc_client/flush.rs:174-178`, `:220-233`, `:249-259`), so the second insert writes nothing.

Do not "fix" the timeout path.

## Remaining problem

The fingerprint includes the evaluation time. When the incoming timestamp is missing or invalid, the backend substitutes the current time:

```rust
// feature-toggle-backend/src/grpc/mod.rs:1497-1502
let evaluated_at = if event.evaluated_at_unix_ms > 0 {
    DateTime::from_timestamp_millis(event.evaluated_at_unix_ms).unwrap_or_else(Utc::now)
} else {
    Utc::now()
};
```

Each retry of such an event gets a different timestamp and a different fingerprint, so it is stored again.

## How it fails

A producer sends `evaluated_at_unix_ms = 0`, for example a third-party client or an edge with a clock before the epoch. Under backend slowness the edge retries, and each retry inserts a duplicate row, inflating evaluation counts.

## Fix

Make the substitute deterministic for a given request. Options, in preferred order:
1. Reject events with `evaluated_at_unix_ms <= 0` or out of range (`InvalidArgument` for that event or batch). This is a behavior change for producers that send 0; check the SDKs first.
2. Or compute the fingerprint from the raw `evaluated_at_unix_ms` value, not the substituted time, and keep storing `now()` as the display time.

## Tests

In the `grpc/mod.rs` test module (gated writer pattern at about `:1778`), push the same batch twice with `evaluated_at_unix_ms = 0`. Assert one stored row (option 2) or `InvalidArgument` (option 1).

## Acceptance criteria

- Retrying a batch never creates duplicate rows, whatever its timestamps.
- `cargo test -p feature-toggle-backend` passes.
