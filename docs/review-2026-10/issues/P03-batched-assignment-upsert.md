# P03: push_user_assignments does one DB upsert per row

| Field | Value |
|---|---|
| Type | Performance (DB round trips) |
| Severity | Medium |
| Status | Verified (code traced) |
| Crate | `feature-toggle-backend` |
| Behavior change | Minor (see "Behavior impact") |
| Depends on | [B11](B11-assignment-push-missing-team-check.md) (same code path; do B11 first) |

## Problem

Call path: `grpc/mod.rs:1095-1112`, then `UserFlagLogicImpl::upsert_after_auth` (`logic/user_flag.rs:121-139`), then `UserFlagAssignmentRepository::upsert` (`database/user_flag_assignment.rs:60-83`). That is one autocommit round trip per row. A 1000-row edge batch (`assignment_flush_batch_size = 1000`) means 1000 sequential round trips.

Current SQL (unchecked `sqlx::query`, so there is no `.sqlx` entry):

```sql
INSERT INTO user_flag_assignments (user_id, feature_id, environment_id, assigned, variant)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (user_id, feature_id, environment_id)
DO UPDATE SET assigned = EXCLUDED.assigned, variant = EXCLUDED.variant, assigned_at = now()
```

## Fix

Add `upsert_many` to the repository, the logic trait and `NoopUserFlagLogic`:

```sql
INSERT INTO user_flag_assignments (user_id, feature_id, environment_id, assigned, variant)
SELECT u.user_id, u.feature_id, u.environment_id, u.assigned, u.variant
FROM UNNEST($1::text[], $2::uuid[], $3::uuid[], $4::bool[], $5::text[])
     AS u(user_id, feature_id, environment_id, assigned, variant)
ON CONFLICT (user_id, feature_id, environment_id)
DO UPDATE SET assigned = EXCLUDED.assigned, variant = EXCLUDED.variant, assigned_at = now()
```

Keep today's semantics:
- Keep skipping rows with an empty `user_id`, `feature_id` or `environment_id`.
- Keep returning `InvalidInput` for bad UUIDs.
- **Dedupe by `(user_id, feature_id, environment_id)` in Rust, keeping the last occurrence.** Otherwise Postgres raises "ON CONFLICT DO UPDATE command cannot affect row a second time". Last-write-wins is the current behavior.
- Bind `variant` as `Vec<Option<String>>`. The 100-character VARCHAR limit still applies.
- Buffer the stream into chunks of 500–1000 rows and flush the remainder at end of stream.
- Run B11's team ownership check once per chunk (`id = ANY($2)` on `features` and on `environments`).
- Staying on `sqlx::query` needs no `.sqlx` change. Switching to `query!` needs `cargo sqlx prepare`.

## Behavior impact

- Rows in one chunk share one `assigned_at`, because `now()` is the transaction start time.
- A bad row now fails its whole chunk, while today the earlier rows are already committed. This is safe: the edge requeues the whole batch on error (`feature-edge-server/src/grpc_client/flush.rs:78-88`), and the upsert is idempotent.

## Tests

- Extend `tests/grpc_ingest_idempotency_integration.rs` (after B11 switches it to seeded IDs):
  - push the same key twice with different variants, and assert one row with the last variant
  - push about 1500 rows, and assert the count
- Repo test under `tests/database/` for `upsert_many` covering insert and update.

## Acceptance criteria

- One DB round trip per chunk instead of one per row.
- Stored results are identical to today's (last-write-wins).
- `cargo test -p feature-toggle-backend` passes.
