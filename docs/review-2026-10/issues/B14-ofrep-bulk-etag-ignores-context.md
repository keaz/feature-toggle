# B14: OFREP bulk ETag ignores the evaluation context, so 304 returns stale results

| Field | Value |
|---|---|
| Type | Bug (correctness, HTTP caching) |
| Severity | Medium (OFREP is unusable until B05 is fixed, so this is hidden today) |
| Status | Verified (independent code trace) |
| Crate | `feature-edge-server` |
| Behavior change | Yes, intended: 304 only when config **and** context match |
| Depends on | [B05](B05-ofrep-empty-client-secret.md) (to observe it) |

## Problem

`ofrep_bulk_etag` hashes only the serialized cached flag configs:

```rust
// feature-edge-server/src/handlers.rs:1076-1091
// sha256 over serde_json::to_string(feature) for each feature
```

It is checked against `If-None-Match` before any evaluation:

```rust
// feature-edge-server/src/handlers.rs:1269-1287
let etag = ofrep_bulk_etag(&features);
if let Some(if_none_match) = http_req.headers().get(header::IF_NONE_MATCH)...
    && if_none_match_contains(if_none_match, &etag)
{ return Ok(HttpResponse::NotModified().finish()); }
```

The bulk response also depends on things the ETag does not include:
- `targetingKey` and the attributes
- the calling client's `environment_id` (from client info)

Two more defects:
- `if_none_match_contains` (`handlers.rs:1093-1098`) treats `*` as a match. `If-None-Match: *` therefore returns 304 even on the first request. That is wrong for POST.
- The 304 response does not include the `ETag` header.

## How it fails

1. A browser provider evaluates anonymous user `anon-1` and stores ETag E.
2. The user logs in. The provider re-posts with `targetingKey = u-42` and `If-None-Match: E`.
3. No config changed, so the edge returns 304, and the provider keeps the anonymous user's flags.

## Fix

- `etag = sha256(config_hash ‖ environment_id ‖ targetingKey ‖ canonical(attributes))`.
- Canonicalize by copying the top-level `attributes` HashMap into a `BTreeMap`. Nested `serde_json::Map` is already sorted, because the `preserve_order` feature is not enabled; verify that in `Cargo.lock` features.
- Remove the `*` match, or return 412 for it.
- Include the `ETag` header on 304.

Sticky assignments do not need to be in the hash. They are added only for truthy results, never change in place, and are purged on every Upsert/Snapshot, which already changes the config hash.

Do not hash the evaluated body. It is precise, but it removes the CPU saving and makes every poll emit evaluation events.

## Behavior impact

More 200 responses after context changes, which is correct. Identical repeat requests still get 304.

## Tests

Extend `ofrep_bulk_etag_is_stable_and_matchable` (`feature-edge-server/src/handlers.rs:1429`):
- same features, different `targetingKey`: ETags differ
- different attributes, or different environment: ETags differ
- same attributes inserted in a different order: ETags are equal
- `If-None-Match: *` does not return 304

## Acceptance criteria

- A 304 is returned only when the config, environment and context all match.
- `cargo test -p feature-edge-server` passes.
