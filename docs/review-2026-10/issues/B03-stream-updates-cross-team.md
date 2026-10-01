# B03: Stream updates are not filtered by team, so edges cache other teams' flags

| Field | Value |
|---|---|
| Type | Bug (correctness, tenant isolation, security) |
| Severity | Critical |
| Status | Fixed on branch fix/b03-stream-team-filter |
| Crate | `feature-toggle-backend` (edge hardening optional) |
| Behavior change | Yes, intended: edges stop receiving other teams' flags |

## Problem

There is one process-wide `tokio::sync::broadcast` channel of `pb::FeatureUpdate` (`feature-toggle-backend/src/lib.rs:79-80`). Every writer sends updates for every team into it.

The per-stream task in `stream_updates` filters updates only by feature key:
- `SubscriptionFilter::matches` (`feature-toggle-backend/src/grpc/mod.rs:157-162`): `AllFeatures` matches everything.
- `stream_allows_feature` (`feature-toggle-backend/src/grpc/mod.rs:196-221`): checks keys only.

```rust
// grpc/mod.rs:157
fn matches(&self, feature_key: &str) -> bool {
    match self { Self::AllFeatures => true, Self::Keys(keys) => keys.contains(feature_key) }
}
```

`team_id` is in scope (`grpc/mod.rs:1242`), but the spawned forwarding task (from about `:1326`) never uses it. Every Upsert carries `FeatureFull.team_id` (set at `grpc/mod.rs:804`).

The edge does not filter either:
- `handle_feature_update` (`feature-edge-server/src/grpc_client/stream.rs:80-95`) inserts every Upsert and Snapshot.
- `MappedFeatureCache` is keyed by feature key only (`feature-edge-server/src/main.rs:194-205`).

Keys are unique only per team (`UNIQUE (key, team_id)`), so two teams can share a key.

## How it fails

1. Teams A and B both have a flag `checkout`.
2. Team B disables its flag.
3. Team A's edge (subscribed with `AllFeatures`, the default and the state after any lag resync) receives Team B's Upsert.
4. The edge overwrites its `checkout` entry with Team B's config and serves Team B's rules to Team A's users.

Even without a key collision, Team A's edge receives and stores Team B's full flag configs. That leaks configuration across tenants.

## Fix

1. In the forwarding task, capture `team_id` and drop any Upsert whose `feature.team_id != team_id.to_string()`. Do this before `stream_allows_feature`, at about `grpc/mod.rs:1328`.
2. Delete updates have no `team_id` (only `feature_key`). The backend sends no Delete today (see B15). If B15 adds Deletes, use one of these:
   - Preferred: wrap the internal broadcast payload as `(Uuid /* team_id */, pb::FeatureUpdate)`. This is internal only, with no proto or contract change, but every `updates_tx.send(..)` call site must change. Find them with `grep -rn "updates_tx" feature-toggle-backend/src`.
   - Minimal: forward Deletes only for keys this stream's team owns, or that are in its snapshot.
3. Optional defense on the edge: ignore any feature whose `team_id` differs from the client's team. The edge knows the team from `GetClientInfo`.

## Behavior impact

Edges stop receiving other teams' flags, so caches get smaller and there are fewer "Sending feature update" log lines. Correct behavior is unchanged within a team.

## Tests

In `feature-toggle-backend/tests/grpc_tests.rs`, copy the pattern of `stream_empty_subscription_sends_full_snapshot` (about line 1170):
1. Subscribe as Team A with empty keys and drain the snapshot.
2. Send an Upsert on `updates_tx` with `FeatureFull { team_id: <Team B>, key: "shared", .. }`.
3. Assert `recv_update_with_timeout(.., 300ms)` returns `None`.
4. Control: send a Team A Upsert and assert it arrives.

## Acceptance criteria

- A stream never forwards a feature whose `team_id` differs from the subscribing client's team.
- `cargo test -p feature-toggle-backend --test grpc_tests` passes.
