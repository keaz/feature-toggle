# B08: Edge retries permanent gRPC errors

| Field | Value |
|---|---|
| Type | Bug (availability, wasted load) |
| Severity | Medium |
| Status | Confirmed (code traced) |
| Crate | `feature-edge-server` |
| Behavior change | Permanent errors fail fast instead of after retries. Final results are unchanged. |
| Related | [B07](B07-retry-backoff-math.md), [P04](P04-cache-client-auth-failures.md), [B12](B12-evaluation-timestamp-fallback-breaks-dedupe.md) |

## Problem

`Retry::spawn` retries every `Err(tonic::Status)`, including errors that cannot succeed on retry:
- `feature-edge-server/src/grpc_client.rs:39` (feature fetch)
- `feature-edge-server/src/grpc_client.rs:91` (client info)
- `feature-edge-server/src/grpc_client/flush.rs:220` (evaluation flush)

Example: the backend returns `InvalidArgument("client_secret is required")` or `Unauthenticated("invalid client_secret")`. The edge repeats the same request `max_attempts` more times.

## Fix

Replace `Retry::spawn(strategy, action)` with `RetryIf::spawn(strategy, action, is_transient)`:

```rust
fn is_transient(status: &tonic::Status) -> bool {
    use tonic::Code::*;
    matches!(
        status.code(),
        Unavailable | DeadlineExceeded | ResourceExhausted | Aborted | Internal | Unknown
    )
}
```

- Do not retry: `InvalidArgument`, `Unauthenticated`, `PermissionDenied`, `NotFound`, `FailedPrecondition`, `AlreadyExists`, `Unimplemented`, `OutOfRange`, `Cancelled`.
- Keep `ResourceExhausted` retryable. The backend uses it for the ingest ack timeout. Retrying there is safe: the DB unique index on `ingest_fingerprint` (`ON CONFLICT DO NOTHING`) drops the duplicate. See B12 for the one case where the fingerprint is not stable.
- Keep `Unknown` and `Internal` retryable. Transport failures can appear as these.

## Behavior impact

Permanent failures return after one call instead of `max_attempts + 1` calls. Callers see the same final error.

## Tests

Use the mock backend in `feature-edge-server/src/grpc_client.rs` tests, which already counts attempts (`MockBackendState`):
- The mock returns `Unauthenticated`: assert exactly one attempt.
- The mock returns `Unavailable` twice, then OK: assert three attempts and success.

## Acceptance criteria

- All three call sites use the transient-error predicate.
- `cargo test -p feature-edge-server` passes.
