# JI-14: Approval and rollout by an external system (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
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

(empty)
