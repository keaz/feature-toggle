# JI-14: Approval and rollout by an external system (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in `0751c25` |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-11 |
| Behavior change | Additive. Two approval-request columns and response fields. The new path is only called by the JI-15 rule engine. Existing approval flows are unchanged. |
| Design | [design.md §3.8](../design.md#38-approval-by-an-external-system-ji-14) |

## Goal

Give the rule engine one logic call that requests, approves, deploys or rolls back a feature in one environment on behalf of Jira, using the same rules as the human paths (state machine, freeze windows, dependencies, edge broadcast, activity), and recording Jira as approver when the environment trusts it (decision J12).

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Stage request orchestration | `logic/feature.rs`: `DeploymentLogic::request_stage_change` (after JI-11 it takes `StageChangeMeta`) |
| Approval creation | `logic/approval.rs`: `maybe_create_stage_change_request` |
| Vote and execution | `logic/approval.rs`: `apply_vote_tx`, `execute_change_tx` (applies the stage change after approval), `publish_event`, `notify_edge_servers`, `dispatch_stage_change_approved_notification` |
| Auto-approval close pattern | `logic/approval.rs`: auto-approval path closes the request guarded by `status = 'pending'` and runs the change in the same transaction (see `HANDOFF` of the AI work, fix `0af0a84`). Reuse the same guard. |
| State machine | `validation.rs`: `validate_stage_transition` |
| Freeze windows | `rest/operational_safety.rs`: `enforce_freeze_for_stage` (REST-level; you need a logic-level check that does not accept an override) |
| Dependencies | `logic/dependency_graph.rs`: `ensure_rollout_dependencies_safe` |
| Broadcast | `rest/feature.rs`: `broadcast_feature_update`; the approval logic has its own `notify_edge_servers` |
| Approval DTO | `rest/approval.rs`: `ApprovalRequestResponse` |

**Check first and record in the handoff log:** how a stage reaches `DEPLOYMENT_APPROVED` when **no** approval policy applies to its environment. `validate_stage_transition` only allows `DEPLOYMENT_REQUESTED → DEPLOYMENT_APPROVED`, and `status_requires_interception` only intercepts `*_REQUESTED`. Find the real path (UI and api-tests `stage-deployment.test.ts`) before designing the no-policy branch.

## Changes

1. **Migration** `<timestamp>_approval_request_source.sql`: `approval_source VARCHAR(20) NOT NULL DEFAULT 'fluxgate' CHECK (approval_source IN ('fluxgate','jira','auto'))`, `external_approver JSONB NULL`. Backfill `auto` for `status = 'auto_approved'` if that is cheap; otherwise leave the default and note it.
2. **Entity and DTO**: `ApprovalRequest.approval_source`, `external_approver`; `ApprovalRequestResponse.approvalSource`, `externalApprover`. Update every select and struct literal.
3. **Logic** `logic/external_change.rs` with the types and the state table of design §3.8:
   - `apply_external_action(feature_id, environment_id, action, ctx) -> Result<ExternalOutcome, Error>`.
   - `request` reuses `DeploymentLogic::request_stage_change` with `StageChangeMeta { external_ref: Some(issue_key), reason: Some("Jira status '<status>'") }` and `ctx.actor_user_id`.
   - `approve` with `trusted_approval`: in one transaction, close the pending request (`status = 'approved'` guarded by `status = 'pending'`, `approval_source = 'jira'`, `external_approver`, `executed_at`), run `execute_change_tx`, write activity `approval_request_approved_externally` (metadata: issue key, Jira status, Jira actor, `ai_risk_mode_skipped` when the policy's AI mode is not `off`). After commit: publish the approval event, notify edge servers, send the "approved" notification.
   - `deploy`: only from `DEPLOYMENT_APPROVED`, through `request_stage_change(.., Deployed, ..)`; otherwise `Refused("not approved")` or `NoOp`.
   - `rollback`: `ROLLBACK_REQUESTED` through the normal path, then (trusted) approve as Jira and execute `ROLLBACKED`.
   - Freeze: a stage in a freeze window gives `Refused("freeze window <name>")`. No override.
   - Dependency failures and invalid transitions give `Refused(<message>)`, not `Err`. `Err` is only for infrastructure errors.
   - Never call the vote endpoint logic (`approve_request`): JI-01 forbids the integration's shadow user from voting, and an external approval is not a vote.
4. Wire `ExternalChangeLogic` in `lib.rs::run` and register it as `web::Data` for JI-15.

## Tests (write first)

- Table test over the state table: every (action, status, trusted) row gives the expected `Applied`/`NoOp`/`Refused`.
- DB integration tests (seeded DB): trusted `approve` from `NOT_DEPLOYED` under a policy closes the request with `approval_source = 'jira'` and the stage is `DEPLOYMENT_APPROVED`; untrusted `approve` refuses and changes nothing; `deploy` before approval refuses; `deploy` after approval gives `DEPLOYED` and a `FeatureUpdate` is broadcast; trusted `rollback` from `DEPLOYED` ends `ROLLBACKED`; a freeze window refuses.
- Concurrency: a human vote and a Jira approval on the same pending request: exactly one closes it (guarded update).
- Existing approval tests pass unchanged.

## Done when

- All rows of the state table are covered by tests and pass.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend`, `./scripts/check-contract-compat.sh` pass.
- Handoff log entry written (including the no-policy finding), `HANDOFF.md` and README table updated.

## Handoff log

### 2026-10-03: done in `0751c25`

**No-policy finding (checked first, as asked)**

- Without an approval policy, no code path moves a stage to `DEPLOYMENT_APPROVED` (or `ROLLBACK_APPROVED`). `StageChangeRequestType` has no `DeploymentApproved`; `request_stage_change(DeploymentRequested)` with no policy sets the stage to `DEPLOYMENT_REQUESTED` directly, and from there `DEPLOYED` is an invalid transition (`validate_stage_transition`). The UI (`FeatureCreate.tsx` `getAvailableActions`) offers no action on `DEPLOYMENT_REQUESTED`, and `api-tests/.../advanced/stage-deployment.test.ts` only requests (it accepts 200/400/403). Only the approval path (`execute_change[_tx]` with `approval_target_status`) writes `*_APPROVED`.
- So a human-driven stage in a no-policy environment stays at `DEPLOYMENT_REQUESTED` (pre-existing gap, not fixed here; a person can only reject and re-request). The JI-14 path closes it for Jira only: when a trusted approval finds no pending request, it locks the stage (`SELECT ... FOR UPDATE`) and moves `DEPLOYMENT_REQUESTED → DEPLOYMENT_APPROVED` (or `ROLLBACK_REQUESTED → ROLLBACK_APPROVED`) with `approve_or_reject_stage_change_tx`.

**What changed**

- Migration `20261004030000_approval_request_source.sql`: `approval_requests.approval_source VARCHAR(20) NOT NULL DEFAULT 'fluxgate' CHECK (IN ('fluxgate','jira','auto'))`, `external_approver JSONB NULL`. Backfill `auto` for `status = 'auto_approved'` (cheap, done).
- `ApprovalRequest.approval_source: String`, `.external_approver: Option<JsonValue>`; constants `APPROVAL_SOURCE_FLUXGATE/AUTO/JIRA` in `database/entity.rs`. Every request select and `REQUEST_RETURNING` reads them. Auto-approval (`mark_auto_approved*`) now sets `approval_source = 'auto'`; `revert_auto_approval` puts back `'fluxgate'`.
- `ApprovalRepository::find_pending_stage_change_request(stage_id)` (newest pending `stage_change` whose payload `stage_id` matches). `ApprovalRepositoryTx::approve_externally_tx(conn, request_id, source, external_approver)`: guarded by `status = 'pending'`, sets `approved`, source, approver, `executed_at`; no vote row.
- `ApprovalLogic::approve_stage_change_externally(stage_id, ExternalApproval) -> ExternalApprovalResult { Approved { approval_request_id: Option<Uuid> }, AlreadyResolved }`. With a pending request: guarded close + `execute_change_tx` + activity row in one transaction, then `publish_event`, `notify_edge_servers`, "approved" notification. Without one: the direct move above, activity row, edge notify. Needs the pool (`approval_logic_with_pool*`). Mocks: `MockApprovalLogic` gained the method.
- `logic/external_change.rs`: `ExternalAction`, `ExternalActor`, `ExternalChangeContext` (design §3.8 plus `external_status`, the Jira status for `external_approver`), `ExternalOutcome` (`Serialize`, tag `outcome`, snake case), trait `ExternalChangeLogic` (`#[automock]`) with `apply_external_action`, factory `external_change_logic(pool, feature_logic, approval_logic, feature_repository, activity_log_repository, updates_tx)`. The state table is the pure `plan(action, status, trusted) -> Plan` (crate-private). Refusal constants: `REFUSED_NOT_TRUSTED`, `REFUSED_NOT_APPROVED`, `REFUSED_NO_STAGE`, `REFUSED_ALREADY_RESOLVED`.
- Wired in `lib.rs::run` as `web::Data<Box<dyn ExternalChangeLogic>>`.
- Activity type `approval_request_approved_externally` (`activity_types::APPROVAL_REQUEST_APPROVED_EXTERNALLY`), `entity_type = 'feature'`, actor = shadow user, actor name "Jira (<display name>)". Metadata: `feature_id`, `feature_key`, `team_id`, `stage_id`, `environment_id`, `environment_name`, `approval_source`, `approval_request_id` (null without a request), `status` (the stage status reached), `issue_key`, `external_status`, `external_actor {system, account_id, display_name}`, and `ai_risk_mode_skipped: "<mode>"` when the policy's AI mode is not `off`.
- A freeze refusal writes a `freeze_blocked` activity row (same type as REST) with `external_ref` and `external_status`.
- REST: `ApprovalRequestResponse.approvalSource` (enum `ApprovalSource`: `fluxgate`, `jira`, `auto`, registered in `ApiDoc`) and `.externalApprover` (JSON or null). Contract baseline updated.

**Decisions taken in this task**

- The state table follows design §3.8 with two readings where it was silent: `ROLLBACK_REJECTED` counts as deployed (`deploy` → `NoOp`, `rollback` → request again, like `DEPLOYED`); `approve` on any rollback status (trusted) is `NoOp`. Untrusted `approve` is `Refused` whatever the status.
- Freeze is checked once, before the first step of any non-`NoOp` plan, for every action (approve included).
- Errors: `Error::DatabaseError` is `Err`; every other error from a step (invalid transition, dependency, no eligible approver, not found) is `Refused(<message>)`. When a later step is refused after an earlier one applied (for example request ok, approve refused), the outcome is `Refused("<reason> (stage now <STATUS>)")` and the feature is still broadcast.
- A Jira approval that loses the race to a vote (or finds the stage changed) is `Refused("approval request already resolved")`, never a second close.
- The executor broadcasts a `FeatureUpdate` after any applied step (the logic request path does not broadcast; REST handlers do). The approval close also notifies edge servers, so a deploy-adjacent approve may broadcast twice; harmless upsert.
- Team membership: the request path (`DeploymentLogic::request_stage_change`, `maybe_create_stage_change_request`) does **not** check that the requester is a team member (only REST does, through `RoleAuthorizer`). The shadow user still has no `user_teams` row; none was added.
- `logic` reuses `rest::operational_safety::active_freeze_for_environment` (as the scheduler already does) instead of copying the query.

**Verified**

- `cargo fmt`; `cargo clippy --all-targets`: no warnings in changed files (remaining warnings are pre-existing).
- `cargo test -p feature-toggle-backend` on `feture_toggle_test`: all pass (lib 816, integration_test 317, others).
- `SQLX_OFFLINE=true cargo build --all-targets`: ok (no new `query!` macros, no `.sqlx` change).
- `./scripts/check-contract-compat.sh`: ok after the baseline update.
- New tests: `logic::external_change::tests` (3, every action × status × trust), `rest::approval::tests::map_request_returns_the_approval_source_and_external_approver`, `tests/database/external_change_test.rs` (12: request, trusted approve under a policy, approve of a pending request, untrusted approve, no-policy approve, deploy before/after approval with broadcast, trusted and untrusted rollback, freeze, missing stage, vote vs Jira race ×5). `auto_approval_applies_the_stage_change_*` now also asserts `approval_source = 'auto'`.
- Mutation checks: removing the freeze check, the final broadcast, the `status = 'pending'` guard or the `'auto'` source each makes the matching test fail.
