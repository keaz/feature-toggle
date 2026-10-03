# Jira integration: handoff

Read this after [`README.md`](README.md) and [`design.md`](design.md). It records the current state, the next task, and facts a later task needs. Update it at the end of every task.

## 1. Status

| Task | State | Where |
|---|---|---|
| Planning (this folder) | Done 2026-10-03 | backend `46a50a7` |
| JI-01 system clients cannot vote | Done | backend `e1bebfe` |
| JI-10 external links backend | Done | backend `f3941e4` |
| JI-11 `externalRef`/`reason` backend | Done | backend `e251f9e` |
| JI-13 integration config | Done | backend `4b66d36` |
| JI-14 external approval path | Done | backend `0751c25` |
| JI-15 inbound events + rules | Done | backend `120b73d` |
| JI-12 by-key endpoints (optional) | Done (built before JI-13, at the user's request) | backend `513f393` |
| JI-20 UI Jira links | Done | UI `3366750`, backend `a95d30c` |
| JI-21 UI ref/reason, Jira approval | Done | UI `b603188` |
| JI-22 UI Jira settings | Done | UI `131060d` |
| JI-30 setup guide + e2e test | Done | backend `1ba9d09` |

**Phase 1 is complete (2026-10-03).**

**Phase 2 is planned (2026-10-03):** write-back to Jira (comments + live remote link), inbound rate limit, signed native webhooks, reason hint. Design [`phase-2/design.md`](phase-2/design.md), tasks [`phase-2/README.md`](phase-2/README.md). **Next task: [JI-41](phase-2/tasks/JI-41-writeback-config.md).**

| Phase 2 task | Status | Commit |
|---|---|---|
| JI-40 activity rows for approval decisions | Done | backend `c79fc64` |
| JI-41 write-back config | Open | |
| JI-42 outbound jobs + sender | Open | |
| JI-43 capture | Open | |
| JI-44 inbound rate limit | Open | |
| JI-45 native webhook HMAC | Open | |
| JI-46 UI write-back settings | Open | |
| JI-47 reason hint on stage change | Open | |
| JI-50 guide + e2e | Open | |

Facts found while planning phase 2 (backend `b276078`):
- Approval decisions (vote, auto-approval, capped reconciliation), cancel and approval-gated requests write no activity row today; the `stage_approved` constant is never written (JI-40 fixes this).
- Stage rows from `request_stage_change` are best effort on the pool (`let _ = log_activity`), entity `stage`, with `metadata.feature_id`.
- Jira-made `stage_deployed`/`stage_rollbacked` rows carry no `approval_source`; the reliable marker is `actor_id` = the integration's `actor_user_id`.
- `receive_jira_event` uses no transaction; `JiraEventRepository::insert` is pool only.
- `public_base_url` is the backend URL; the UI origin is `allowed_origin`.

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
- **Migrations:** latest is `20261004040000_jira_integration_events.sql` (JI-15). Later tasks use later `20261004...` timestamps, in task order.
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

### From JI-13 (`4b66d36`)

- Tables `jira_integrations`, `jira_status_rules` (migration `20261004020000`). Rules are stored trimmed, run in `position` order (0-based list index); `environment_ids NULL` = every environment the issue names.
- `environment_aliases` maps a Jira value to an **environment id** (`BTreeMap<String, Uuid>` in `JiraIntegrationRow`), not a name. Keys are unique ignoring case; JI-15 should compare the issue value to the keys ignoring case and surrounding spaces, then fall back to `get_active_environments_by_name`.
- `database/jira_integration.rs`: `JiraIntegrationRepository` (`#[automock]`) `get`, `list_for_team`, `list_rules`, `create`, `update`, `delete`, `replace_rules`, `set_secret_hash`, `team_environment_ids`; Tx variants in `JiraIntegrationRepositoryTx`. Registered as `web::Data<Box<dyn JiraIntegrationRepository>>`. Row types `JiraIntegrationRow` (holds `secret_hash`; never serialize it) and `JiraStatusRuleRow` in `database/entity.rs`.
- `logic/jira_integration.rs`: `hash_secret` (SHA-256 hex, same as `jwt_guard::hash_token`) for JI-15's secret check; `ACTION_REQUEST/APPROVE/DEPLOY/ROLLBACK`; `validate_field_name`.
- Shadow user: `users.username = 'jira-integration-<integration id>'`, `auth_source = 'system'`, password hash `JIRA_INTEGRATION_NO_LOGIN`, not admin, `Requester` role only, **no `user_teams` row**, id in `jira_integrations.actor_user_id`. Its id is not in `system_clients`, so `is_system_client` is false for it. Delete sets `enabled = false` and keeps the row. JI-14: check whether the stage change path requires the requester to be a team member; if it does, decide whether to add a `user_teams` row at create time (and note it here).
- Policy: `PolicyAction::ManageJiraIntegrations` covers every method under `/api/v1/jira-integrations/**` and `/api/v1/teams/{id}/jira-integrations/**` (so JI-15's `GET /jira-integrations/{id}/events` is covered). JI-15's inbound endpoint must **not** live under `/jira-integrations/`: design §3.9 puts it at `/api/v1/integrations/jira/{integrationId}/events`, which is outside this policy; make it public in `JwtGuard`.
- `logic::policy::is_system_client_management_route` was replaced by `is_team_admin_management_route`.
- Activity types `jira_integration_created/updated/secret_rotated/rules_replaced/deleted`, `entity_type = 'jira_integration'`, `entity_id` = integration id, metadata `{integration_id, team_id, name, actor_user_id, ...}`; never the secret or hash.
- REST: create and rotate return `JiraIntegrationWithSecretResponse {integration, secret}`; the secret is 43 characters (32 bytes base64url).

### From JI-14 (`0751c25`)

- Entry point for JI-15: `web::Data<Box<dyn ExternalChangeLogic>>` (`logic/external_change.rs`), `apply_external_action(feature_id, environment_id, ExternalAction, &ExternalChangeContext) -> Result<ExternalOutcome, Error>`. Map rule actions with `ACTION_REQUEST/APPROVE/DEPLOY/ROLLBACK` → `ExternalAction::Request/Approve/Deploy/Rollback`. `MockExternalChangeLogic` exists for the handler tests.
- `ExternalChangeContext { actor_user_id: integration.actor_user_id, external_ref: issue key, external_status: Jira status, reason: "Jira status '<status>'", external_actor: ExternalActor { system: "jira", account_id, display_name }, trusted_approval: environment ∈ jira_approved_environment_ids }`. `external_actor.system` becomes `approval_source`, so it must be `jira` (DB `CHECK`).
- `ExternalOutcome` serializes as `{"outcome": "applied", "from", "to", "approval_request_id"}`, `{"outcome": "no_op", "status"}`, `{"outcome": "refused", "reason"}`; JI-15 can store it in `results` as is, or map it to the design's `{outcome, from, to, reason}`. `Err` means infrastructure only: log it for that target and continue with the others.
- The call does its own freeze check (logs `freeze_blocked`), dependency check, approval request creation, broadcast and activity rows. JI-15 does not broadcast or log per target.
- No-policy finding: no path but JI-14's reaches `DEPLOYMENT_APPROVED` without a policy (details in the JI-14 handoff log). Untrusted `request` in a no-policy environment leaves the stage at `DEPLOYMENT_REQUESTED`, where only a trusted Jira `approve` (or a reject) moves it on. Mention this in JI-30's guide.
- `approval_requests.approval_source` (`fluxgate`, `jira`, `auto`) and `external_approver` JSON (`system`, `account_id`, `display_name`, `issue_key`, `status`); API `approvalSource`, `externalApprover` (JI-21 shows "Approved by Jira (<display name>, <issue key>)" from these).
- `ApprovalLogic::approve_stage_change_externally` needs the pool; `approval_logic(...)` without a pool returns `InvalidInput("Transaction pool not configured")`.
- The shadow user needs no `user_teams` row: the logic request path does not check team membership.

### From JI-15 (`120b73d`)

- Inbound endpoint: `POST /api/v1/integrations/jira/{integrationId}/events`, no JWT (`middleware::is_public_jira_event_path`), secret in `Authorization: Bearer <secret>` or `X-FluxGate-Jira-Secret`. Body ≤ 1 MiB: Jira webhook (Cloud/DC) or Automation "Issue data (Jira format)", bare or `{issue, user}`. Response 200 `{eventId, results, unknownEnvironments, unknownFeatures, ignored?, duplicate}`; 400 bad body; 401 any auth failure (one message); 413 too large; 500 database error (event stored with `error`).
- Event log for the UI (JI-22): `GET /api/v1/jira-integrations/{id}/events?offset&limit` → `{items: [JiraEventLogItem], meta}`; item: `id`, `integrationId`, `receivedAt`, `issueKey`, `jiraStatus`, `jiraActor {accountId, displayName}`, `results` (`RuleResult` list), `unknownEnvironments`, `unknownFeatures`, `ignored`, `error`. Events are kept 30 days.
- `RuleResult` fields: `featureId`, `featureKey`, `environmentId`, `environment`, `ruleId`, `ruleStatus`, `action`, `outcome` (`applied`/`no_op`/`refused`/`error`), `from`, `to`, `reason`, `approvalRequestId`.
- For JI-30's guide: a Jira webhook acts only on events whose changelog has a `status` item; Automation bodies always count as a status change (the rule decides when to send). The environment comes from `environmentField` (`labels` or a custom field; option objects and multi-selects work). Repeats within 10 minutes (same issue, status, environments and changelog id / `fields.updated`) return the first result.
- Open point: no rate limit on the public route (the repo has none). Add one at the proxy before exposing it to the internet.
- Manual curl script used for verification: see the JI-15 handoff log (setup by SQL + API, then three curls).

### From JI-20 (UI `3366750`, backend `a95d30c`)

- UI API module `src/api/externalLinks.ts` (`listExternalLinks`, `createExternalLink`, `deleteExternalLink`, type `ExternalLink`). JI-21 can use `listExternalLinks` to decide whether an `externalRef` is a linked Jira key (design §3.3: link it only then, using the link's `url`).
- `JiraLinksCard` shares the `useSharedQuery` key `feature-external-links:<featureId>`; another component that changes links should `invalidateSharedQuery` that key.
- UI permission for link writes: `canAccessTeamsManagement()` (admin or Team Admin), matching the backend rule.
- Duplicate link 409 message is now `<KEY> is already linked to this feature`.
- The UI repo had an unrelated uncommitted change in `src/pages/FeatureDetail.tsx` (stage label); it is still in the working tree, not committed. Do not commit it with a task unless the user asks.
- Manual UI checks: Chrome extension was not connected; the Chrome DevTools MCP (`new_page` with `isolatedContext`) works. Backend on `:8080` + `pnpm dev --port 8090` needs no config change (`public/config.js` points at `:8080`).

### From JI-21 (UI `b603188`)

- `requestStageChange(stageId, request, { freezeOverrideReason?, externalRef?, reason? })` (`StageChangeOptions` in `api/features.ts`) sends only non-empty, trimmed values.
- `ApprovalRequest` (`api/approvals.ts`) has `externalRef`, `requestReason`, `approvalSource: 'fluxgate' | 'jira' | 'auto'`, `externalApprover: ExternalApprover | null`. `ExternalApprover` keys are snake_case (`account_id`, `display_name`, `issue_key`, `status`) because the backend stores and returns that JSON as is.
- Reusable for JI-22: `ApprovalRequestContext` (`components/approvals/`), `getJiraApproval` and `findExternalRefUrl` (`utils/approvalRequestContext.ts`). The approvals detail reads links through the shared key `feature-external-links:<featureId>`.
- Stage requests by a user need the Requester role; `api-test-admin` has none, so manual UI checks of stage requests need a separate requester user (the test DB now has `ji21-requester` / `password123` in team `JI-21 check 1791039213`).
- Open follow-up: `ReasonQualityHint` on the new stage change reason field.

### From JI-22 (UI `131060d`)

- Settings → Jira at `/settings/jira` (admins and Team Admins). JI-30's guide can point users there for the events URL, the header and the secret (shown once on create and rotate).
- UI API module `src/api/jiraIntegrations.ts`; helpers in `src/utils/jiraIntegrations.ts` (`jiraInboundEventsUrl`, `ruleWarnings`). The events URL is built from the UI's `REST_HTTP_URL`; if the backend is public under another host, the guide should say to use the public URL.
- Integration delete removes the integration row (rules and events cascade); the JI-13 note "Delete sets `enabled = false`" is about the shadow user. To pause an integration, use the Enabled switch.
- Test DB now holds integration `JI-22 Jira` (team `JI-21 check 1791039213`, rules "Ready for Release" → approve, "Done" → deploy, no Jira-approved environment) and four events for `PROJ-12`.

### From JI-30 (`1ba9d09`)

- End-to-end test: `api-tests/src/tests/advanced/jira-flow.test.ts`. Run it against a local backend: `API_BASE_URL=http://127.0.0.1:18180/api/v1 pnpm --dir api-tests exec jest --runInBand jira-flow`. Its helpers `webhookBody` / `automationBody` / `sendJiraEvent` are the reference Jira bodies for later tests.
- A feature with two stages needs a relationship (`Pipeline must have at least 1 relationships` otherwise).
- Admin guide: `docs/jira-integration/setup-guide.md`. It recommends Jira Automation "Send web request" only, because Jira's native webhooks cannot send the secret header. If a later phase adds HMAC or URL-token authentication for native webhooks, update section 2 of the guide.
- The guide's Jira smart values were not run against a real Jira site. The first team that sets it up should confirm them and fix the guide if needed.
- Open for phase 2: rate limit on the public inbound route, write-back to Jira, `ReasonQualityHint` on the stage change reason field.
