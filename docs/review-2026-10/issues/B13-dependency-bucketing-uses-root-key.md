# B13: Dependencies are bucketed with the root flag's key, not their own

| Field | Value |
|---|---|
| Type | Bug (evaluation correctness) |
| Severity | High |
| Status | Fixed on branch review-followups |
| Crate | `evaluation-engine` (used by backend and edge) |
| Behavior change | **Yes, user-visible.** Some users' results for dependent flags change. Needs maintainer sign-off and a release note. |
| Related | [B19](B19-non-boolean-dependency-blocks-dependents.md) |

## Problem

`evaluate_with_memo` passes the root `evaluation_context` unchanged into the dependency evaluation. The percentage bucket hashes `ec.flag_key`, which is still the root key:

```rust
// evaluation-engine/src/lib.rs:699
let flag_key = evaluation_context.flag_key.clone();
// evaluation-engine/src/lib.rs:737-739
let dep_result =
    evaluate_with_memo(evaluation_context, dependency, memo, visiting, visiting_set);
if !dep_result.value.as_bool().unwrap_or(false) {
// evaluation-engine/src/lib.rs:520-524 (passes_stage_criteria)
hasher.update(ec.flag_key.as_bytes());
hasher.update(b":");
hasher.update(sticky_val.as_bytes());
```

Inside F, dependency D's bucket is `SHA256("F:" + targetingKey)`. When D is evaluated directly, the bucket is `SHA256("D:" + targetingKey)`. Every caller sets the root key (`feature-toggle-backend/src/rest/evaluation.rs:333-335`, `feature-toggle-backend/src/grpc/mod.rs:895-903`, `feature-edge-server/src/handlers.rs:506`, `:995`).

Bucketing matters only when D's matched criterion is a `WeightedSplit` with non-empty allocations and D's variants differ in truthiness. A missing variant counts as `true` (`lib.rs:575-583`).

## How it fails

- D is a 10% rollout, and F depends on D.
- About 90% of the users who get D = true directly fail the D check inside F.
- About 9% of all users get F although D is off for them.

There is also a correlation side effect. F's variant and D's pass/fail use the same hash. With D = {off: 50, on: 50} and F = {a: 50, b: 50}, only F-buckets of 50 and above pass D, so variant `a` is never served.

## Who changes if fixed

Let q be the truthy share of D's bucket range. Among users whose F evaluation reaches D's weighted check, D's check flips for 2q(1−q) of them:
- half (q(1−q)) lose F
- half gain F

Examples: q = 50% flips 50%; q = 10% flips 18%. Only dependent-flag results change. D itself and flags without such dependencies are unchanged.

Rollout timing:
- Backend REST and gRPC evaluation: immediate (no sticky cache).
- Edge: users with a cached truthy F keep it until F is purged. The purge happens on the next Upsert or Snapshot of F, so in practice at the next edge restart or reconnect.

## Fix

Pass the bucketing key explicitly:
- Add a `bucket_key: &str` parameter to `evaluate_with_memo` and `passes_stage_criteria`.
- Root call: `&evaluation_context.flag_key`. Root results stay byte-for-byte identical.
- Dependency recursion: `&dependency.key`.

Avoid cloning the context with a new `flag_key`, which copies the attributes map for every dependency.

## Tests

In `evaluation-engine/tests/evaluation_tests.rs` (next to `evaluate_dependency_failed`, about line 301; use `mk_ctx`/`mk_feature`):
- D: weighted {off: false 50, on: true 50}. F depends on D. For about 1000 targeting keys, assert `evaluate(F) == true` implies `evaluate(D with flag_key = "D") == true`.
- F with weighted variants {a, b}: assert both variants are served.
- Existing engine tests pass: `cargo test -p evaluation-engine`.

## Acceptance criteria

- A dependency's result inside a dependent flag equals evaluating the dependency directly for the same context.
- The maintainer approved the behavior change, and it is noted in the release notes.
