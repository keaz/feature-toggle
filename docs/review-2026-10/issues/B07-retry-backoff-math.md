# B07: Edge retry backoff grows as base^n (500 ms, 250 s, 34.7 h)

| Field | Value |
|---|---|
| Type | Bug (availability) |
| Severity | High |
| Status | Confirmed (code traced, checked against tokio-retry 0.3.0 source) |
| Crate | `feature-edge-server` |
| Behavior change | Retry waits become short and bounded. Retry count is unchanged. |
| Related | [B08](B08-retry-permanent-grpc-errors.md) (do together or right after) |

## Problem

Three production call sites build the retry strategy like this:

```rust
let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
    .take(app.retry_config.max_attempts);
```

| Call site | Location |
|---|---|
| Feature fetch (`fetch_feature_via_grpc`) | `feature-edge-server/src/grpc_client.rs:39` |
| Client info fetch (`fetch_client_info_via_grpc_uncached`) | `feature-edge-server/src/grpc_client.rs:91` |
| Evaluation event flush | `feature-edge-server/src/grpc_client/flush.rs:220` |

(`grpc_client.rs:210`, `:514` and `:559` are test code.)

In tokio-retry 0.3, `ExponentialBackoff::from_millis(base)` yields `base^1, base^2, base^3` ms. It does not double. From `exponential_backoff.rs`: `duration = current * factor; current = current * base`. With the default config (`base_delay_ms = 500`, `max_attempts = 3`):

| Retry | Wait |
|---|---|
| 1 | 500 ms |
| 2 | 250,000 ms (~4.2 min) |
| 3 | 125,000,000 ms (~34.7 h) |

## How it fails

- A request-path fetch (cache miss on `/evaluate` or OFREP) during a short backend outage hangs for minutes, and each hung request keeps an Actix worker busy.
- One failed evaluation flush blocks the flush loop for 4+ minutes while the event queue fills up and starts dropping the oldest events.

## Fix

Add one helper and use it at all three call sites:

```rust
/// Doubling backoff: base, 2*base, 4*base, ... capped at `max`.
pub(crate) fn backoff(cfg: &RetryConfig) -> impl Iterator<Item = Duration> {
    let base = cfg.base_delay_ms.max(1);
    let cap = Duration::from_millis(base.saturating_mul(16)); // or a new config field
    (0..cfg.max_attempts as u32)
        .map(move |i| Duration::from_millis(base.saturating_mul(1u64 << i.min(16))).min(cap))
}
```

- Keep `take(max_attempts)` semantics: `max_attempts` retries after the first call. Do not change the retry count.
- Optional: add jitter with `tokio_retry::strategy::jitter`.
- Place the helper on `RetryConfig` (`feature-edge-server/src/config.rs`) or in `grpc_client.rs`.

## Behavior impact

Waits become 500 ms, 1 s, 2 s with the default config. Retry count is unchanged.

## Tests

- Unit test in `config.rs` tests: `backoff(base=500, attempts=3)` yields `[500ms, 1000ms, 2000ms]`. `attempts=0` yields nothing. A large attempt count stays at or below the cap.
- Existing mock-backend retry tests in `grpc_client.rs` (around lines 514 and 559) must still pass.

## Acceptance criteria

- No production code calls `ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)`.
- `cargo test -p feature-edge-server` passes.
