# Jira integration: handoff

Read this after [`README.md`](README.md) and [`design.md`](design.md). It records the current state, the next task, and facts a later task needs. Update it at the end of every task.

## 1. Status

| Task | State | Where |
|---|---|---|
| Planning (this folder) | Done 2026-10-03 | backend docs commit `docs(jira): plan phase 1 ...` |
| JI-01 system clients cannot vote | Open, **next** | — |
| JI-10 external links backend | Open | — |
| JI-11 `externalRef`/`reason` backend | Open | — |
| JI-12 by-key endpoints | Open (needs JI-11) | — |
| JI-20 UI Jira links | Open (needs JI-10) | — |
| JI-21 UI ref/reason | Open (needs JI-11) | — |
| JI-30 recipe + e2e test | Open (needs JI-01, JI-10, JI-11, JI-12) | — |

**Next task: [JI-01](tasks/JI-01-system-clients-cannot-vote.md).**

## 2. Planning log (2026-10-03)

- User decisions: Jira Cloud **and** Data Center; approvals stay in FluxGate; phase 1 scope = linking + `externalRef` (no webhooks, bridge or Forge). See README decisions J1-J8.
- The investigation listed security prerequisites P1-P11. P1, P2, P4, P5, P6, P7 and P8 were verified fixed in backend `6c9b332`:
  - P4: `PolicyAction::ManageSystemClients` → `evaluate_team_admin_or_admin`, which denies system clients (`logic/policy.rs`).
  - P5: system-client JWT `is_admin: false` (`create_system_client_jwt_token`); the guard ignores a legacy `is_admin: true` claim; `admin_exists` excludes system-client shadow users.
  - P6: scheduled `ENABLE/DISABLE/ARCHIVE_FEATURE` go through `authorize_scheduled_action` → `authorize_feature_update`; the creator is reloaded from the DB at execution (`load_scheduled_change_creator`).
  - P7: `PUT /criteria/{id}/variant-allocations` writes a version, an activity row and broadcasts.
  - P8: `ensure_user_can_vote` returns `SelfApprovalNotAllowed` when the requester approves.
  - P3 (JWT `iss`/`aud`, key rotation) and P9-P11 (edge/SDK) are not needed for Jira phase 1.
- Gap found during planning, now JI-01: system-client JWTs carry roles `Requester` and `Approver`, and `flag:write` allows POST on `approval-requests` paths. Votes probably fail today only because `ensure_user_can_vote` reads the `Approver` role from `user_roles`, which the shadow user should not have. There is no explicit rule.
- `docs/investigations/2026-10-sso-jira-sdks.md` is not committed in this repo (untracked at planning time). The README links to it; commit it or keep a local copy.

## 3. Environment and verification

- **Database:** local Postgres, user `postgres`; the password is in your shell's `DATABASE_URL`. Do not commit it.
  - Dev DB `feature_toggle` has no seed data; about 10 seed-dependent tests fail there.
  - Use the test DB `feture_toggle_test` (migrated and seeded) for the full suite:
    ```bash
    export DATABASE_URL="${DATABASE_URL%/*}/feture_toggle_test"
    cargo test -p feature-toggle-backend
    ```
  - After adding a migration, apply it to the test DB: `sqlx migrate run --database-url "$DATABASE_URL" --source feature-toggle-backend/migrations`. To reset: `psql "$DATABASE_URL" -c 'DROP SCHEMA public CASCADE; CREATE SCHEMA public;'`, then migrate and `psql "$DATABASE_URL" -f init.sql`.
- **Run the backend end to end** without clashing with a dev server: a TOML outside the repo with `http_addr = "127.0.0.1:18180"` and `grpc_addr = "127.0.0.1:15151"`, then `FEATURE_TOGGLE_CONFIG=<file> ./target/debug/feature-toggle-backend` from `feature-toggle/` (needs `log4rs.yaml` in the working directory). On an empty DB create the admin with `POST /api/v1/admins` (`api-test-admin` / `password123`), then `POST /api/v1/auth/login`.
- **System client for manual checks:** `POST /api/v1/teams/{teamId}/system-clients` as a team admin returns `{systemClient, token}`. Use the token as `Authorization: Bearer <token>`. Never paste it into a file or commit.
- **Contracts:** after a DTO or endpoint change run `./scripts/export-contracts.sh`, copy `feature-toggle-backend/contracts/generated/contract-hashes.json` to `contracts/baseline/`, then `./scripts/check-contract-compat.sh`. Only the baseline file is tracked.
- **Migrations:** latest is `20261003030000_approval_request_auto_approve_failures.sql`. JI-10 and JI-11 each add one; use `20261004...` timestamps and keep them in task order.
- **UI:** `../feature-toggle-ui/` on `main` (last commit at planning: `dc16a2f`). pnpm only: `pnpm lint`, `pnpm build`, `pnpm test:run`.
- After code changes run `graphify update .` in the repo you changed.

## 4. Facts for later tasks

(Add entries here as tasks finish: new symbols, signature changes, migrations, decisions taken during a task.)
