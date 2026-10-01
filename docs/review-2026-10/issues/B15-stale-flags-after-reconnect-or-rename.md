# B15: Edge keeps stale flags after reconnect or key rename, and misses new flags

| Field | Value |
|---|---|
| Type | Bug (correctness, cache staleness) |
| Severity | High |
| Status | Verified (independent code trace) |
| Crates | `feature-toggle-backend`, `feature-edge-server` |
| Behavior change | Yes, intended: edges receive Delete messages and drop stale keys |
| Depends on | [B01](B01-feature-key-lookup-substring-match.md) (exact key match), [B03](B03-stream-updates-cross-team.md) (team scoping of Deletes) |

## Problem

The edge clears its cache only on a forced resync:
- `prepare_for_full_resync` (`feature-edge-server/src/grpc_client/stream.rs:38-43`, calls `clear_all`) runs only when `force_full_resync` is set (`stream.rs:128-130`).
- That flag is set only after a "lagged" error marker (`stream.rs:163-165`).

On a normal stream error or disconnect (`stream.rs:168-176`), the cache is kept, and the reconnect subscribes with the cached keys:

```rust
// feature-edge-server/src/grpc_client/stream.rs:17-24
let cached_keys = if force_full_snapshot { Vec::new() }
    else { app.mapped_cache.get_all_keys().await };
```

The backend keys-filtered snapshot (`feature-toggle-backend/src/grpc/mod.rs:1284-1295`) runs one lookup per key. A key with no rows produces no message. The backend never sends `Action::Delete`; a grep of `feature-toggle-backend/src` finds only Upsert, Snapshot, Error and Heartbeat. The edge already handles Delete (`stream.rs:97-103`). `by_key` has no TTL (`feature-edge-server/src/main.rs:125`), so stale entries stay until LRU eviction.

Note: no REST route hard-deletes a feature. `FeatureLogic::delete_feature` is called only from tests, and archived features stay in snapshots (`include_archived = true`, `database/feature.rs:2290`). Stale entries come from:

1. **Key rename** (`feature-toggle-backend/src/rest/feature.rs:1603`, update path). The Upsert carries the new key, and the edge inserts it. The old key entry stays and keeps evaluating, with no disconnect needed.
2. **Team deletion**, which cascades to the team's features.

A second effect: after the first normal reconnect, the edge subscription shrinks to its cached keys, so flags created later never reach it over the stream. It learns of them only through a cache-miss `GetFeatureByKey`, which the 60 s negative cache (`feature-edge-server/src/main.rs:129`) can hide.

## How it fails

- Rename `beta-ui` to `beta-ui-v2`: the edge keeps serving the old `beta-ui` config indefinitely.
- The edge disconnects briefly, and a new flag is created. The edge resubscribes with only its old keys, so live updates for the new flag never arrive.

## Fix

1. **Backend keys snapshot:** filter results to the exact key (after B01, use `get_feature_by_key`). For each requested key with no exact match, send `FeatureUpdate { action: Delete, feature_key: key, .. }`. There is no proto change.
2. **Rename:** when the REST update changes a feature's key, broadcast a Delete for the old key, in addition to the Upsert for the new key. Respect B03 team scoping.
3. **New flags after reconnect:** have the edge resubscribe with empty keys (full snapshot) on every reconnect, not only after lag. Combined with the B04 fix (no hang on large snapshots), this costs one snapshot per reconnect.
   - Optional: clear or mark-and-sweep the edge cache on full snapshot. A clean sweep needs a "snapshot complete" marker, which is a proto change and changes the contract hash. Agree with the maintainer first.

## Behavior impact

- Edges receive Delete messages and drop stale keys. Evaluating those keys falls back to fetch, then not-found.
- Reconnects send a full snapshot (more data per reconnect).

## Tests

- Backend (`feature-toggle-backend/tests/grpc_tests.rs`): subscribe with `feature_keys = ["gone"]`, have the mock return nothing for `"gone"`, and assert a Delete with `feature_key == "gone"` arrives.
- Backend: rename a feature through the REST update, then assert a Delete for the old key is broadcast.
- Edge (test module in `feature-edge-server/src/grpc_client.rs`, about lines 540-700): put `"gone"` in the cache, call `handle_feature_update` with that Delete, and assert `mapped_cache.get("gone")` returns `None`.

## Acceptance criteria

- After a rename, the old key stops evaluating on the edge within one update.
- After a reconnect, flags created during the disconnect reach the edge.
- `cargo test -p feature-toggle-backend` and `cargo test -p feature-edge-server` pass.
