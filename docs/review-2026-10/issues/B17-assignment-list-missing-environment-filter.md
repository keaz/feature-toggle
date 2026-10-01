# B17: Listing assignments by environment returns rows from every environment

| Field | Value |
|---|---|
| Type | Bug (correctness) |
| Severity | Medium |
| Status | Fixed on branch `fix/b17-assignment-env-filter` |
| Crate | `feature-toggle-backend` |
| Behavior change | Yes, intended: only rows for the requested environment are returned |

## Problem

`UserFlagAssignmentRepository::list` in `feature-toggle-backend/src/database/user_flag_assignment.rs` has four branches. The `(None, Some(eid))` branch (feature not given, environment given) never filters on `ufa.environment_id`:

```sql
-- user_flag_assignment.rs, about lines 121-133
SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned, ufa.variant
FROM user_flag_assignments ufa
JOIN features f ON f.id = ufa.feature_id
WHERE f.team_id = $1 AND EXISTS (
    SELECT 1 FROM features_pipeline_stages s
    WHERE s.feature_id = f.id AND s.environment_id = $2
)
```

It returns assignments from **all** environments for any feature that has a stage in `eid`.

## How it fails

Feature F has stages in `dev` and `prod`. A caller lists assignments for `dev`. The result includes F's `prod` assignments. If an edge loads them (`ListUserFlagAssignments` with `environment_id` set), it caches prod assignments under their prod keys. That is harmless for the cache key, but wrong for any consumer that trusts the filter, such as admin UIs and exports.

Note: the edge's startup load sends an empty `environment_id` (`feature-edge-server/src/grpc_client.rs:153`), so it uses the `(None, None)` branch and is not affected. Find other callers with `grep -rn "list_user_assignments\|\.list(" feature-toggle-backend/src`.

## Fix

Add `AND ufa.environment_id = $2` to the WHERE clause. Keep or drop the `EXISTS` clause: with the direct filter it only removes rows whose feature has no stage in that environment. Keep it to avoid changing that subset.

This is a `query_as!` macro, so run `cargo sqlx prepare` in `feature-toggle-backend/` (with `DATABASE_URL` set) and commit the `.sqlx/` changes.

## Tests

DB test under `feature-toggle-backend/tests/database/`: insert assignments for one feature in two environments, list with `(None, Some(env_a))`, and assert only `env_a` rows come back.

## Acceptance criteria

- Every row returned by `list(.., None, Some(eid))` has `environment_id == eid`.
- `cargo test -p feature-toggle-backend` passes, and Docker builds with `SQLX_OFFLINE=true`.
