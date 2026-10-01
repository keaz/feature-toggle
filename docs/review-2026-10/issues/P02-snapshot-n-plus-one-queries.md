# P02: Feature mapping runs 3–5 queries per stage (N+1) on snapshots and Evaluate

| Field | Value |
|---|---|
| Type | Performance (DB round trips) |
| Severity | High (snapshot time grows with feature × stage count) |
| Status | Verified (code traced) |
| Crate | `feature-toggle-backend` |
| Behavior change | No. Output must be identical. |
| Related | [B04](B04-stream-snapshot-deadlock-and-lost-updates.md) (slow snapshots widen its lost-update window) |

## Problem

`map_db_feature_to_full` and `map_db_feature_payload_to_engine` in `feature-toggle-backend/src/grpc/mod.rs` load child data one query at a time:

| Step | Location |
|---|---|
| `get_feature_stages` per feature | `grpc/mod.rs:691` |
| `get_stage_criteria` per stage | `grpc/mod.rs:700-704` |
| variants per feature | `grpc/mod.rs:782-786` |
| same pattern in the engine mapper | `grpc/mod.rs:508-517`, `:610-613` |

Each `get_stage_criteria` (`database/feature.rs:2033-2220`) is itself 3–5 queries:
1. team-id lookup (`:2038`)
2. load of **every context in the team** (`:2055`), repeated for every stage
3. criteria for the stage (`:2077`)
4. variant allocations (`= ANY` over criteria IDs, `:2099`), if there are criteria
5. rule groups and conditions (`= ANY`, `:2136`), if there are criteria

Per feature with S stages: 1 + 3S to 5S queries, plus 1 if the feature is Contextual.

Example: 50 features × 3 stages with criteria, 20 of them Contextual, is about **822 sequential round trips** for one snapshot. A batched version needs about 8.

The unary `Evaluate` path pays the same per call. `map_db_feature_to_engine` (`grpc/mod.rs:436-500`) also calls `get_feature_by_id` (2 queries) for each dependency.

## Fix

Add batched `FeatureRepository` methods, following the existing `get_feature_dependencies_batch` (`database/feature.rs:855-885`):

1. `get_feature_stages_batch(&[Uuid]) -> HashMap<Uuid, Vec<FeaturePipelineStage>>`: `.. FROM features_pipeline_stages WHERE feature_id = ANY($1)`
2. `get_stage_criteria_batch(team_id, &[Uuid]) -> HashMap<Uuid, Vec<StageCriterion>>`:
   - skip the team-id lookup, because the caller knows the team
   - load contexts once per call
   - criteria: `WHERE sc.stage_id = ANY($1) ORDER BY sc.stage_id, sc.priority ASC, sc.id`
   - run the allocation and rule-group `ANY($1)` queries once over all criteria IDs (SQL can stay as is)
3. `get_feature_variants_batch(&[Uuid])`: `WHERE feature_id = ANY($1) ORDER BY feature_id, created_at`

Then change the snapshot path to load all features, then batch-load stages, criteria and variants, then map in memory. Keep `map_db_feature_to_full` for single features, or have it call the batch methods with one ID.

Notes:
- The criteria-loading logic is copied in three places (`database/feature.rs:2033`, about `:4120`, about `:4780`). Factor it into one helper that both the single and batch paths use.
- Ordering: `get_feature_stages` has no ORDER BY today, and rule-group order already comes from a HashMap. Keep the same order the per-stage path produces, and where it is undefined today, do not rely on it in tests.
- New `query!`/`query_as!` strings need `cargo sqlx prepare` in `feature-toggle-backend/` and a commit of the `.sqlx/` changes.
- `FeatureRepository` is `#[automock]`. Add `expect_*_batch` expectations in `tests/grpc_tests.rs`.

## Tests

- DB test (`tests/database/criteria_test.rs`): for seeded stages, assert that `get_stage_criteria_batch` output equals the per-stage `get_stage_criteria` output.
- `cargo test -p feature-toggle-backend --test grpc_tests`: snapshot tests such as `stream_empty_subscription_sends_full_snapshot` (about `:1170`) produce identical messages.
- Optional: log the query count or snapshot duration before and after for the seeded team.

## Acceptance criteria

- A snapshot's query count no longer grows with the number of stages per feature.
- Snapshot and Evaluate output are unchanged.
- `cargo test -p feature-toggle-backend` passes, and Docker builds with `SQLX_OFFLINE=true`.
