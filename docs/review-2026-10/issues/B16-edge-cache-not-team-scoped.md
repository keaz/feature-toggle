# B16: Edge feature cache is not scoped by team, so /evaluate and OFREP bulk can serve other teams' flags

| Field | Value |
|---|---|
| Type | Bug (correctness, tenant isolation, security) |
| Severity | High |
| Status | Verified (independent code trace) |
| Crate | `feature-edge-server` |
| Behavior change | Yes, intended: callers see only their own team's flags |
| Depends on | [B03](B03-stream-updates-cross-team.md) (backend fix removes the main source). This issue is edge defense in depth plus OFREP bulk filtering. |

## Problem

- The edge cache is keyed by feature key only: `by_key: Cache<String, Arc<Feature>>` (`feature-edge-server/src/main.rs:106-117`). `map_proto_to_engine` drops `team_id`, and `engine::Feature` has no team field.
- The edge first subscribes with empty `feature_keys` and `environment_id: ""` (`feature-edge-server/src/grpc_client/stream.rs:17-31`), so it gets `AllFeatures`. Because of B03, every team's Upserts land in this cache.
- `get_or_fetch_feature` (`handlers.rs:558-571`) returns any cached entry to any caller without a team check. On a cache miss it fetches with the request client's credentials and stores the result in the same shared cache.
- OFREP bulk (`handlers.rs:1269-1276`) returns every cached key:

  ```rust
  for key in app.mapped_cache.get_all_keys().await {
      if let Some(feature) = app.mapped_cache.get(&key).await {
          features.push(Arc::new(hydrate_feature_with_dependencies(&app, &feature).await));
  ```

- Bulk also does not filter by environment. Flags with no stage in the caller's environment are listed as `DISABLED`/false, which exposes their names.

Keys are unique per team only (`UNIQUE (key, team_id)`), so Team B's `checkout` overwrites Team A's `checkout`, and a Team B Delete removes it.

## How it fails

1. The edge is configured for a Team A client.
2. Team B updates `checkout`. The backend broadcasts it, and the edge's AllFeatures stream receives it (B03).
3. The edge's `checkout` entry now holds Team B's config.
4. Team A's `/evaluate` and OFREP serve Team B's rules, and OFREP bulk lists all of Team B's flags.

## Fix

1. Backend first: B03.
2. Edge defense in depth:
   - Keep `team_id` on cached entries, through a wrapper struct or a field on `engine::Feature`. Check whether adding a field to the engine struct affects backend serialization or contracts.
   - Drop stream updates whose `team_id` differs from the edge's configured client team, which the edge knows from `GetClientInfo`.
   - Single and bulk OFREP: filter features by `client_info.team_id`.
   - Optional: bulk skips flags with no stage in `client_info.environment_id`. That is a visible change to the bulk output (fewer `DISABLED` entries), so confirm with the maintainer.
3. If one edge must serve several teams, key the cache by `(team_id, key)` instead. That is a larger change; confirm the deployment model first.

## Behavior impact

Bulk returns fewer flags. Key collisions no longer produce another team's result.

## Tests

In the `feature-edge-server/src/handlers.rs` tests:
- Cache two features with different teams. Assert bulk returns only the caller team's flag.
- Send a stream Upsert with a foreign `team_id` to `handle_feature_update`, and assert the cache is unchanged.

## Acceptance criteria

- No edge response contains a flag from a team other than the caller's.
- `cargo test -p feature-edge-server` passes.
