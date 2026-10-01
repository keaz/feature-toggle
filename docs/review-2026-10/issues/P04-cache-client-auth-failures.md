# P04: Failed client-info lookups are not cached, so every bad request hits the backend

| Field | Value |
|---|---|
| Type | Performance (backend load, abuse resistance) |
| Severity | Low |
| Status | Fixed on branch `perf/p04-cache-auth-failures` |
| Crate | `feature-edge-server` |
| Behavior change | No for valid clients. Bad credentials are rejected from cache for a short TTL. |
| Depends on | [B06](B06-ofrep-auth-error-status.md), [B08](B08-retry-permanent-grpc-errors.md) |

## Problem

`get_or_fetch_client_info` (`feature-edge-server/src/grpc_client.rs:124`) caches only successful lookups:

```rust
let client_info = fetch_client_info_via_grpc_uncached(app, client_id, client_secret).await?;
app.client_info_cache.insert(cache_key, client_info.clone()).await;
```

On failure it returns `None` early. Each OFREP request with bad credentials makes a new backend call (plus retries until B08 is fixed). Anyone who can reach the edge can turn bad requests into backend gRPC load.

## Fix

After B06 makes the result a `Result<_, tonic::Status>`:
- Add a small moka cache `client_info_failures: Cache<String, tonic::Code>` with a short TTL (for example 30 s) and a capacity limit.
- Cache only permanent auth failures (`Unauthenticated`, `InvalidArgument`, `NotFound`, `PermissionDenied`). Do not cache transient errors.
- Check the failure cache before calling the backend.
- Keep the key as `client_id:client_secret`, matching the success cache, so a corrected secret is not blocked.
- Do not log secrets.

## Behavior impact

A newly enabled or created client can be rejected for up to one TTL after an earlier failed attempt. Keep the TTL short and document it.

## Tests

With the mock backend:
- Two calls with a bad secret: assert one backend call.
- Unavailable twice: assert no caching (the second call reaches the backend).

## Acceptance criteria

- Repeated bad-credential requests within the TTL make no backend call.
- `cargo test -p feature-edge-server` passes.

## Update (2026-10-01): reverted

The failure cache is removed. The edge now caches successful client authentications only (keyed by client ID and a SHA-256 hash of the secret), so every rejected credential reaches the backend again. Auth failures are still not retried. This follows the SDK key authentication requirements (P9); see `docs/edge-server-api.md`.
