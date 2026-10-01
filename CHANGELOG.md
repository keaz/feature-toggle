# Changelog

All notable user-visible changes to FluxGate are recorded here.

## Unreleased

### Added

- **Edges drop flags renamed or removed while they were disconnected (backend + edge, B15).** The gRPC stream protocol has a new `FeatureUpdate.Action` value, `SNAPSHOT_COMPLETE = 5`. The backend sends it once after the initial snapshot of each `StreamUpdates` stream, and not when the snapshot fails. On that marker the edge removes cached features of its team that the snapshot did not contain and drops their cached and pending assignments, so evaluating such a key falls back to a backend fetch and then to not-found. Before, these entries kept evaluating until LRU eviction. The change is additive and backward compatible: older edges ignore the new action, and new edges connected to an older backend never sweep. The proto contract hashes changed, so the contract baseline was updated.
- **Configurable edge client-info cache size (edge).** New `[cache] client_max_capacity` setting (env `EDGE_CACHE__CLIENT_MAX_CAPACITY`) sets how many client credentials' info the edge caches. The default stays 1000, so existing deployments are unchanged.

### Changed

- **Dependencies on non-boolean flags are rejected (backend).** A dependency passes only when it evaluates to `true`, so a dependency on a contextual flag with string, number or object variant values always blocked its dependents. Creating or updating a feature to add such a dependency, changing a depended-on flag to non-boolean values (or from `SIMPLE` to `CONTEXTUAL` with non-boolean stored variants), and version rollbacks that would do either now return `400 invalid_input`. Evaluation results do not change. Existing configurations are not migrated and stay editable; their non-boolean dependencies keep blocking until removed.

- **Dependencies are now bucketed with their own key (evaluation engine).** When flag `F` depends on flag `D`, the engine used to evaluate `D` inside `F` with `F`'s key in the bucketing hash (`SHA256("F:" + targetingKey)`). It now uses `D`'s own key (`SHA256("D:" + targetingKey)`), so the `D` check inside `F` gives the same result as evaluating `D` directly for the same user. `F`'s own weighted split is no longer correlated with `D`'s split, so all of `F`'s variants are served again.

  **Who is affected:** only flags that depend on a flag whose matching criterion is a weighted split with variants of different truthiness (for example a percentage rollout `{off: false, on: true}`). Flags evaluated directly, flags without dependencies, and dependencies without weighted splits keep exactly the same results.

  **How many users change:** let `q` be the share of `D`'s bucket range that is truthy. Among users whose evaluation of `F` reaches `D`'s weighted check, `2q(1−q)` see a different `D` result: half of them (`q(1−q)`) lose `F` and half gain it. For example, a 50% rollout flips 50% of those users and a 10% rollout flips 18%.

  **When it takes effect:** backend REST and gRPC evaluations change immediately after the backend is upgraded. Edge servers change once they run the new engine; users who already have a cached truthy result for `F` on an edge keep it until `F` is re-synced (the next upsert or snapshot of `F`, in practice the next edge restart or reconnect).

### Fixed

- **Live updates no longer send variants for Simple flags (backend).** Live `FeatureUpdate` upserts from REST feature and criteria changes and from approvals included the stored variants of `SIMPLE` features, and edge servers applied them. The stream snapshot, `GetFeatureByKey` and REST evaluation omit variants for `SIMPLE` features. So a Simple flag with non-boolean stored variants could act non-boolean on an edge after a live update until the next snapshot. Live updates now use the snapshot mapping: only `CONTEXTUAL` features carry variants.
