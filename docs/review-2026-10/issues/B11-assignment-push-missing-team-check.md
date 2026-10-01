# B11: push_user_assignments lets any client write assignments for other teams' features

| Field | Value |
|---|---|
| Type | Bug (security, tenant isolation) |
| Severity | High |
| Status | Verified (independent code trace) |
| Crate | `feature-toggle-backend` |
| Behavior change | Yes, intended: pushes for unknown or other-team feature/environment IDs are rejected |
| Related | [P03](P03-batched-assignment-upsert.md) (same code path; do B11 first or together) |

## Problem

`push_user_assignments` authenticates the first stream message, then **discards** the client's team:

```rust
// feature-toggle-backend/src/grpc/mod.rs:1040-1045
match self.user_flag_logic
    .authenticate_client(&first_msg.client_id, &first_msg.client_secret).await
{
    Ok(_) => {}          // team_id dropped
```

Every row then goes to `upsert_after_auth` (`grpc/mod.rs:1073-1081`, `:1095-1112`), which parses the UUIDs and upserts with no ownership check:

```rust
// feature-toggle-backend/src/logic/user_flag.rs:133-136
let fid = Self::parse_uuid("feature_id", feature_id)?;
let eid = Self::parse_uuid("environment_id", environment_id)?;
self.user_flag_repo.upsert(user_id, fid, eid, assigned, variant)
```

The DB does not protect either. `user_flag_assignments` has no foreign keys and no team column (`migrations/20250824203500_user_flag_assignments.sql:2-9`). The only key is the primary key `(user_id, feature_id, environment_id)`.

## How it fails

1. An attacker holds any enabled client credential in Team A.
2. They push rows with Team B's `feature_id`/`environment_id`, `assigned = true`, and a variant of their choice.
3. The read path (`database/user_flag_assignment.rs:139-145`, `JOIN features f .. WHERE f.team_id = $1`) returns these rows to Team B.
4. Team B's edge loads them at startup (`load_user_assignments`, `feature-edge-server/src/grpc_client.rs:150-175`) into `assigned_cache`.
5. The edge serves the cached value and variant before local evaluation (`feature-edge-server/src/handlers.rs:722-741`). Targeted Team B users get the flag on, with the attacker's variant.

The attacker needs valid credentials plus Team B's feature and environment UUIDs. Within one team, a client bound to one environment (dev) can also write rows for another environment (prod).

## Fix

1. Keep the team: `let team_id = self.user_flag_logic.authenticate_client(..).await?;` (adjust to the actual return type).
2. Pass `team_id` into `upsert_after_auth(team_id, ..)`.
3. Before upserting, check that the feature and environment belong to the team:
   - `SELECT 1 FROM features WHERE id = $1 AND team_id = $2`
   - `SELECT 1 FROM environments WHERE id = $1 AND team_id = $2` (`environments.team_id` exists, see `migrations/20250607151431_initial_db_setup.sql:13`)

   Return `PermissionDenied` if either check fails. With P03 batching, check once per chunk with `id = ANY($2)`.
4. Do **not** require `environment_id == client.environment_id`. The edge pushes with its own credentials but uses each SDK client's environment (`feature-edge-server/src/handlers.rs:680`, `:792`). That check would break edges that serve several environments.
5. Update the trait signature on the `#[automock]` logic trait and on the `NoopUserFlagLogic` stub (`grpc/mod.rs:42`).

## Behavior impact

- Pushes with unknown or other-team UUIDs fail with `PermissionDenied`.
- An edge serving SDK clients from several teams loses those cross-team writes. They were never read back by that edge anyway.
- The edge requeues the batch on error (`feature-edge-server/src/grpc_client/flush.rs:78-88`). Make sure a permanently rejected batch does not loop forever: drop it, or split out the bad rows. Coordinate with B08's non-retryable codes.

## Tests

- **Fix the existing test first:** `feature-toggle-backend/tests/grpc_ingest_idempotency_integration.rs:198` pushes random `Uuid::new_v4()` feature and environment IDs and expects success. Switch it to seeded IDs: team `51ecc366-f1cd-4d3d-ab73-fa60bad98f27`, client `a1b2c3d4-0000-4000-8000-000000000001`, and a feature and environment from that team in `init.sql`.
- New integration test: push, with the team-`51ecc366` client, a feature owned by team `3eef17bc-9e06-411d-b5f4-7a786e68bb96`. Assert `PermissionDenied` and zero rows in `user_flag_assignments`.
- Unit test in `logic/user_flag.rs` with a mock repo: `upsert` is never called when the feature belongs to another team.

## Acceptance criteria

- No assignment row can be written for a feature or environment outside the authenticated client's team.
- `cargo test -p feature-toggle-backend` passes.
