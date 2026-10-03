# Jira integration: handoff

Read this after [`README.md`](README.md) and [`design.md`](design.md). It records the current state, the next task, and facts a later task needs. Update it at the end of every task.

## 1. Status

| Task | State | Where |
|---|---|---|
| Planning (this folder) | Done 2026-10-03 | backend `46a50a7` |
| JI-01 system clients cannot vote | Done | backend `e1bebfe` |
| JI-10 external links backend | Done | backend `f3941e4` |
| JI-11 `externalRef`/`reason` backend | Done | backend `e251f9e` |
| JI-13 integration config | Open, **next** | — |
| JI-14 external approval path | Open (needs JI-11) | — |
| JI-15 inbound events + rules | Open (needs JI-13, JI-14) | — |
| JI-12 by-key endpoints (optional) | Done (built before JI-13, at the user's request) | backend `513f393` |
| JI-20 UI Jira links | Open (needs JI-10) | — |
| JI-21 UI ref/reason, Jira approval | Open (needs JI-11, JI-14) | — |
| JI-22 UI Jira settings | Open (needs JI-13, JI-15) | — |
| JI-30 setup guide + e2e test | Open (needs JI-15) | — |

**Next task: [JI-13](tasks/JI-13-jira-integration-config-backend.md).**

## 2. Planning log (2026-10-03)

- User decisions: Jira Cloud **and** Data Center; approvals stay in FluxGate; phase 1 scope = linking + `externalRef` (no webhooks, bridge or Forge). See README decisions J1-J8.
- The investigation listed security prerequisites P1-P11. P1, P2, P4, P5, P6, P7 and P8 were verified fixed in backend `6c9b332`:
  - P4: `PolicyAction::ManageSystemClients` → `evaluate_team_admin_or_admin`, which denies system clients (`logic/policy.rs`).
  - P5: system-client JWT `is_admin: false` (`create_system_client_jwt_token`); the guard ignores a legacy `is_admin: true` claim; `admin_exists` excludes system-client shadow users.
  - P6: scheduled `ENABLE/DISABLE/ARCHIVE_FEATURE` go through `authorize_scheduled_action` → `authorize_feature_update`; the creator is reloaded from the DB at execution (`load_scheduled_change_creator`).
  - P7: `PUT /criteria/{id}/variant-allocations` writes a version, an activity row and broadcasts.
  - P8: `ensure_user_can_vote` returns `SelfApprovalNotAllowed` when the requester approves.
  - P3 (JWT `iss`/`aud`, key rotation) and P9-P11 (edge/SDK) are not needed for Jira phase 1.
- Gap found during planning, closed by JI-01: system-client JWTs carry roles `Requester` and `Approver`, and `flag:write` allows POST on `approval-requests` paths.
- `docs/investigations/2026-10-sso-jira-sdks.md` is not committed in this repo (untracked at planning time). The README links to it; commit it or keep a local copy.

## 2a. Replan log (2026-10-03, after JI-01)

The user wants Jira status to drive the rollout per environment, configured dynamically: for example "Ready for Release" with Environment = QA approves the feature for QA, and "Done" deploys it. Decisions (README J10-J15):

- Jira is a **trusted approver** in environments an admin opts in. The approval request is closed as `approval_source = 'jira'` with the Jira user in the audit. Jira users are not mapped to FluxGate users.
- The environment comes from a **Jira field** (custom field or labels) plus an alias map.
- Rule actions: request, approve, deploy, rollback. Rules are rows in FluxGate, edited in the UI.
- `deploy` on a stage that is not approved is refused and logged; it never approves implicitly.

This revises J2 ("approvals stay in FluxGate"). **JI-01 stays valid**: a system-client token still cannot vote through the vote endpoint, and system clients are still never eligible approvers. Jira approves only through the integration path (JI-14/JI-15), authenticated by the integration secret, with its own non-approver shadow user.

New tasks JI-13, JI-14, JI-15, JI-22; JI-12 became optional; JI-21 and JI-30 changed. Design §3.7-3.9 hold the new parts.

## 3. Environment and verification

- **Database:** local Postgres, user `postgres`; the password is in your shell's `DATABASE_URL`. Do not commit it.
  - Dev DB `feature_toggle` has no seed data; about 10 seed-dependent tests fail there.
  - Use the test DB `feture_toggle_test` (migrated and seeded) for the full suite:
    ```bash
    export DATABASE_URL="${DATABASE_URL%/*}/feture_toggle_test"
    cargo test -p feature-toggle-backend
    ```
  - After adding a migration, apply it to the test DB: `sqlx migrate run --database-url "$DATABASE_URL" --source feature-toggle-backend/migrations`. To reset: `psql "$DATABASE_URL" -c 'DROP SCHEMA public CASCADE; CREATE SCHEMA public;'`, then migrate and `psql "$DATABASE_URL" -f init.sql`.
- **Docker is not installed** on the development machine at JI-01 time, so `pnpm --dir api-tests run test:docker` fails (`docker: command not found`). Run API tests against a local backend instead (next bullet), with `API_BASE_URL=http://127.0.0.1:18180/api/v1 pnpm --dir api-tests exec jest --runInBand <pattern>`. The test DB already has the `api-test-admin` / `password123` user.
- **Run the backend end to end** without clashing with a dev server: a TOML outside the repo with `http_addr = "127.0.0.1:18180"` and `grpc_addr = "127.0.0.1:15151"`, then `FEATURE_TOGGLE_CONFIG=<file> ./target/debug/feature-toggle-backend` from `feature-toggle/` (needs `log4rs.yaml` in the working directory). On an empty DB create the admin with `POST /api/v1/admins` (`api-test-admin` / `password123`), then `POST /api/v1/auth/login`.
- **System client for manual checks:** `POST /api/v1/teams/{teamId}/system-clients` as a team admin returns `{systemClient, token}`. Use the token as `Authorization: Bearer <token>`. Never paste it into a file or commit.
- **Contracts:** after a DTO or endpoint change run `./scripts/export-contracts.sh`, copy `feature-toggle-backend/contracts/generated/contract-hashes.json` to `contracts/baseline/`, then `./scripts/check-contract-compat.sh`. Only the baseline file is tracked.
- **Migrations:** latest is `20261004010000_approval_request_external_ref.sql` (JI-11). Later tasks use later `20261004...` timestamps, in task order.
- **UI:** `../feature-toggle-ui/` on `main` (last commit at planning: `dc16a2f`). pnpm only: `pnpm lint`, `pnpm build`, `pnpm test:run`.
- After code changes run `graphify update .` in the repo you changed.

## 4. Facts for later tasks

(Add entries here as tasks finish: new symbols, signature changes, migrations, decisions taken during a task.)

### From JI-01 (`e1bebfe`)

- System clients cannot vote and are never eligible approvers (decision J9). A request a system client creates needs a human approver in the team; tests that drive approvals with a system client must add one (as `api-tests/src/tests/system-client.test.ts` does).
- Vote denial: 403, `error: "forbidden"`, `code: "system_client_vote_not_permitted"`. Constants in `rest/error.rs` (`SYSTEM_CLIENT_VOTE_CODE`, `SYSTEM_CLIENT_VOTE_MESSAGE`); `JwtGuard` and `RestError::system_client_vote_not_permitted()` both use them.
- `ApprovalRepository` gained `is_system_client(user_id)`. It is `#[automock]`ed: any mock test that reaches `ensure_user_can_vote` needs `expect_is_system_client().returning(|_| Ok(false))`.
- `approver_qualifies_sql` now requires a `user_teams` row and excludes `system_clients`. Change it only together with routing, the reachable count and `is_eligible_voter`, which all share it.
- No migration, no contract change.

### From JI-10 (`f3941e4`)

- Table `feature_external_links` (migration `20261004000000`), unique constraint `feature_external_links_unique (feature_id, system, external_key)`, index on `(system, external_key)`. Keys are stored upper case; only `system = 'jira'` is allowed.
- `database/external_link.rs`: `ExternalLinkRepository` (`#[automock]`) with `feature_scope(feature_id) -> Option<FeatureScope { team_id, key }>`, `list_for_feature`, `create`, `delete -> bool`, `feature_ids_for_key(team_id, system, key)`. Use `feature_ids_for_key` in JI-15 to resolve an issue key to the team's features. Tx variants in `ExternalLinkRepositoryTx`. Registered as `web::Data<Box<dyn ExternalLinkRepository>>`.
- `logic/external_link.rs`: `normalize_jira_key` (trim, upper-case, `^[A-Z][A-Z0-9_]+-[1-9][0-9]*$`), `validate_url`, `validate_new_link`, `SYSTEM_JIRA`. Reuse `normalize_jira_key` for any issue key that comes from Jira.
- Activity types `external_link_added` / `external_link_removed` (`activity_types::EXTERNAL_LINK_ADDED/REMOVED`), `entity_type = 'feature'`, metadata `{feature_id, feature_key, team_id, link_id, system, external_key, url}`.
- `get_features_with_offset_filtered` (repository and `FeatureCrudLogic`) has a new `external_key: Option<String>` argument after `flag_kind`. Mocks and callers with 14 arguments now take 15.
- Link writes: users need `authorize_feature_update` (admin or Team Admin of the team); system clients need `flag:write` and a token of the feature's team. Reading links as a system client needs `admin:read` or `evaluate` (existing `JwtGuard` rule for GET on `features`): a Jira token needs `flag:write` + `admin:read`.
- `rest/operational_safety.rs`: `policy_actor_for_request(pool, jwt)` (user vs system client, checked in the DB) and `rest_error_from_policy` are `pub(crate)` for reuse.

### From JI-11 (`e251f9e`)

- `crate::model::StageChangeMeta { external_ref: Option<String>, reason: Option<String> }` (`Default`, `PartialEq`).
- Signatures: `DeploymentLogic::request_stage_change(stage_id, request, user_id, meta: StageChangeMeta)`; `ApprovalLogic::maybe_create_stage_change_request(feature, stage, next_status, requested_by, meta: &StageChangeMeta)`. Mock expectations take one more argument (`withf(|_, _, _, meta| ...)`).
- REST validation: `rest/feature/types.rs` `validate_stage_change_meta(external_ref, reason)` (trim, blank → `None`, `externalRef` ≤ 100 chars and no control characters, `reason` ≤ 1000 chars and no control characters except `\n`). Reuse it for any other entry point that takes a ref and reason (JI-12, JI-15).
- Migration `20261004010000`: `approval_requests.external_ref`, `approval_requests.request_reason`. `ApprovalRequest` and `CreateApprovalRequestInput` have both fields; every struct literal needs them (`None` in tests).
- Response: `ApprovalRequestResponse.externalRef`, `.requestReason` (also on the approvals stream).
- Activity: only the direct (not gated) stage change activity row carries `external_ref` / `reason`. The gated path and approval execution (`execute_change[_tx]`) write **no** activity row; none was added. JI-14 should decide whether a Jira approval writes one, and can take `external_ref` from the request.
- Notifications: `STAGE_CHANGE_REQUESTED` metadata has `external_ref` when present; the message ends with ` Ref: <externalRef>.`.
- Scheduled stage changes pass their scheduled change reason as `reason`.

### From JI-12 (`513f393`)

- `rest/feature.rs::perform_stage_change(...)` is the one stage change path for REST callers (role check, `externalRef`/`reason` validation, freeze, logic, broadcast). Any new entry point that requests a stage change for a user or system client should call it rather than copy it. JI-15's rule engine authenticates differently (integration secret), so it may call the logic directly; if it does, it must broadcast the feature update itself.
- `resolve_stage_by_key(feature_repo, env_logic, team_id, feature_key, env_name) -> Result<Uuid, RestError>` and `resolve_feature_id_by_key` (both private in `rest/feature.rs`). JI-15 needs the same resolution from an issue's environment value; move them to `logic/` if it needs them outside REST.
- `EnvironmentRepository` / `EnvironmentLogic::get_active_environments_by_name(team_id, name) -> Vec<Environment>`: active, equal ignoring case, no substring. Environment names are not unique in a team, so it can return several; JI-13's alias map should store environment ids, not names.
- 404 codes: `feature_not_found`, `environment_not_found`, `stage_not_found` (in `code`, and as the message prefix). 409 when same-named environments both hold a stage.
- Both mocks (`MockEnvironmentRepository`, `MockEnvironmentLogic`) gained the method; no existing test needed a change.
- No migration, no `.sqlx` change. Contract baseline updated.
