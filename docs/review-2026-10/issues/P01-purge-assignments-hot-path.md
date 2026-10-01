# P01: purge_assignments_for_feature is O(all assignments) and runs on hot paths

| Field | Value |
|---|---|
| Type | Performance (latency, lock contention). Also data loss of pending assignments. |
| Severity | High under load |
| Status | Fixed: steps 1–2 on branch `perf/p01-purge-assignments`, step 3 on `review-followups` |
| Crate | `feature-edge-server` |
| Behavior change | None for the structural fix (steps 1–2). Step 3 is optional and changes persisted data. |
| Depends on | [B18](B18-assignment-warmup-caches-true-for-all-variants.md) if you do step 3 |

## Problem

```rust
// feature-edge-server/src/main.rs:283-299
self.assigned_cache.retain(|key, _| key.split('|').nth(1) != Some(feature_id));
let mut to_keep = Vec::new();
while let Some(assignment) = self.pending_assignments.pop() {
    if assignment.feature_id != feature_id { to_keep.push(assignment); }
}
for assignment in to_keep { self.pending_assignments.push(assignment); }
```

Each call:
- walks every `assigned_cache` entry, splitting each key string and write-locking each shard in turn, which stalls concurrent lookups
- drains and re-pushes the whole `pending_assignments` `SegQueue`

The cost is O(A + P) per call (A = sticky entries, P = pending assignments).

| Call site | When |
|---|---|
| `handlers.rs:671` | `/evaluate` of a kill-switched (disabled) flag, every request |
| `handlers.rs:948` (bulk loop at `:1290-1300`) | OFREP, once per disabled flag per request: O(D·(A+P)) |
| `grpc_client/stream.rs:80-95` | every stream Upsert and Snapshot, unconditionally: O(N·(A+P)) per reconnect |
| `grpc_client/stream.rs:101` | Delete |

At startup, `load_user_assignments` warms A, and then the full snapshot purges it feature by feature. This is the worst case, and it erases the whole warm-up.

**Data loss:** an Upsert or Snapshot drops that feature's pending (not yet flushed) assignments, even when the feature is still enabled and unchanged, as on a reconnect re-snapshot. The default flush interval is 10 s (`config.rs:159-161`).

## Do not use "purge only on enabled→disabled transition"

That changes evaluation results:
- Today every rule or weight edit to an enabled flag clears sticky results. With transition-only purging, users keep stale results after targeting changes.
- Snapshot purges currently hide B18 (warm-up caches `true` for every variant).

## Fix (results unchanged)

1. **Index `assigned_cache` by feature:** `DashMap<feature_id, DashMap<"user|env", CachedAssignment>>`. A purge becomes one `remove(feature_id)`, and a lookup is two hash lookups. Update every reader and writer of `assigned_cache` (`grep -rn assigned_cache feature-edge-server/src`).
2. **Generation counters instead of draining the queue:**
   - Keep `DashMap<feature_id, u64>`. A purge increments the feature's counter.
   - Each `UserAssignment` stores the counter value at push time.
   - `run_flush_task` drops entries with a stale counter value.
   - Purge becomes O(1), and the flushed set equals today's.
3. **Optional, changes persisted data:** stop dropping pending assignments on Upsert/Snapshot while the feature stays enabled. This fixes the persistence loss. It changes which backend rows exist and therefore what warm-up loads, so do B18 first.

## Tests

- Keep `test_purge_assignments_for_feature` (`feature-edge-server/src/main.rs:460`), adapted to the new structures.
- Push pending entries for features X and Y, purge X, and run one flush against the mock backend (`start_mock_backend` in `grpc_client.rs` tests). Assert only Y was sent.
- Assert `/evaluate` returns the same results for a cached user before and after the refactor.
- Complexity check: 100k assignments across 1000 features, then 1000 purges, finishes in well under a second.

## Acceptance criteria

- Purge cost no longer depends on the total number of assignments.
- Evaluation results are unchanged.
- `cargo test -p feature-edge-server` passes.
