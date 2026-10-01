# Changelog

All notable user-visible changes to FluxGate are recorded here.

## Unreleased

### Changed

- **Dependencies are now bucketed with their own key (evaluation engine).** When flag `F` depends on flag `D`, the engine used to evaluate `D` inside `F` with `F`'s key in the bucketing hash (`SHA256("F:" + targetingKey)`). It now uses `D`'s own key (`SHA256("D:" + targetingKey)`), so the `D` check inside `F` gives the same result as evaluating `D` directly for the same user. `F`'s own weighted split is no longer correlated with `D`'s split, so all of `F`'s variants are served again.

  **Who is affected:** only flags that depend on a flag whose matching criterion is a weighted split with variants of different truthiness (for example a percentage rollout `{off: false, on: true}`). Flags evaluated directly, flags without dependencies, and dependencies without weighted splits keep exactly the same results.

  **How many users change:** let `q` be the share of `D`'s bucket range that is truthy. Among users whose evaluation of `F` reaches `D`'s weighted check, `2q(1−q)` see a different `D` result: half of them (`q(1−q)`) lose `F` and half gain it. For example, a 50% rollout flips 50% of those users and a 10% rollout flips 18%.

  **When it takes effect:** backend REST and gRPC evaluations change immediately after the backend is upgraded. Edge servers change once they run the new engine; users who already have a cached truthy result for `F` on an edge keep it until `F` is re-synced (the next upsert or snapshot of `F`, in practice the next edge restart or reconnect).
