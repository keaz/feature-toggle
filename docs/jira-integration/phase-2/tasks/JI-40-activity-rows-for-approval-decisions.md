# JI-40: Activity rows for approval decisions, cancel and gated requests (backend)

| Field | Value |
|---|---|
| Type | Feature (audit gap) |
| Status | Done in c79fc64 |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — |
| Behavior change | Additive. New `activity_log` rows; the UI activity feed shows them. No status logic changes. |
| Design | [design.md §3.3, J21](../design.md#33-capture-ji-43) |

## Goal

JI-43 learns about stage changes from `activity_log`. Several approval paths change a stage status and write **no** activity row. Add one row, in the same transaction as the status change, for each of them. This task also closes an audit gap: today a human approval does not appear in the activity feed.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where | Activity row today |
|---|---|---|
| Final vote, approved | `logic/approval.rs` `apply_vote_tx` (~1396), `execute_change_tx` call ~1446 | none |
| Final vote, rejected | same function, `execute_change_tx` call ~1483 | none |
| Vote without pool | `apply_vote` (~1301) | none |
| Auto-approval | `auto_approve_request` (~1897), `execute_change_tx` call ~1921, actor `SENTINEL_UUID` | none |
| Capped reconciliation | `approve_capped_request` (~1974), call ~2002 | `approval_requirement_reconciled` only, no stage fields |
| Cancel | `cancel_request` (~1782): stage reset to `previous_status` at ~1826 (tx) and ~1870 (pool) | none |
| Gated request | `logic/feature.rs` `request_stage_change` approval branch (~1773-1889); stage set to `*_REQUESTED` at ~1800 (pool) | none (only a notification) |
| External approval (Jira) | `approve_stage_change_externally` (~2132), call ~2207 | `approval_request_approved_externally`, tx. **Do not change.** |
| Pattern for an in-tx row | `approve_capped_request` ~2009-2035 (`create_activity_tx`) | |
| Direct-branch metadata to copy | `logic/feature.rs` ~2002-2022 | |
| Type constants | `utils/activity_logger.rs`: `STAGE_APPROVED` (never written today), `STAGE_REJECTED` | |

`execute_change_tx` (~1553) computes `final_status` from `change_payload` (`stage_id`, `next_status`, `approval_target_status`, `rejection_target_status`) and writes it with `approve_or_reject_stage_change_tx`.

## Interfaces

- Consumes: nothing new.
- Produces (JI-43 relies on these exact names):
  - Activity types `stage_approved`, `stage_rejected`, `approval_request_cancelled` (new constant `APPROVAL_REQUEST_CANCELLED`), and `stage_change_requested` (new constant `STAGE_CHANGE_REQUESTED`; replace the literal at `logic/feature.rs` ~1998 with it).
  - Every row: `entity_type = "stage"`, `entity_id = <stage_id>`, metadata keys `feature_id`, `feature_key`, `team_id`, `stage_id`, `environment_id`, `environment_name`, `status` (the status written), `approval_request_id`, plus `external_ref` and `reason` when the request has them. Same key names as the direct branch.
  - `actor_id`/`actor_name`:

    | Path | `actor_id` | `actor_name` |
    |---|---|---|
    | vote | approver id | approver username |
    | auto-approval and capped reconciliation | `None` | `"Auto-approval"` / `"Approval reconciliation"` |
    | cancel | the canceller | the canceller's username |
    | gated request | requester id | requester username |

## Changes

1. `execute_change_tx` returns `Result<Option<String>, Error>`: the final status written, `None` when it returned early. The external path ignores the value.
2. New private fn in `logic/approval.rs`:
   ```rust
   /// Activity row for an approval decision that changed a stage (JI-40).
   fn stage_decision_activity(
       request: &ApprovalRequest,
       activity_type: &str,
       final_status: &str,
       stage: &StageContext,           // feature_key, team_id, stage_id, environment_id, environment_name
       actor_id: Option<Uuid>,
       actor_name: Option<String>,
   ) -> CreateActivityLog
   ```
   `StageContext` is a small struct. It is loaded in the transaction from the stage id in `change_payload`. Reuse the feature and environment lookups the approval code already does for notifications (`dispatch_stage_change_approved_notification`); do not add new SQL if one exists.
3. Call sites write the row with `create_activity_tx` **in the same transaction, before commit**:
   - the vote approved branch (`stage_approved`) and the vote rejected branch (`stage_rejected`), in both `apply_vote_tx` and `apply_vote`.
     - In `apply_vote` (pool) use `create_activity`, matching that function's existing non-tx style.
   - `auto_approve_request` and `approve_capped_request` (`stage_approved`). Keep the existing `approval_requirement_reconciled` row.
   - `cancel_request`: `approval_request_cancelled` with `status = previous_status`. Write it in the tx branch, and with `create_activity` in the pool branch.
4. Gated request (`logic/feature.rs`): after the approval request is created and the stage set to `*_REQUESTED`, write `stage_change_requested`.
   - Use the same best-effort `log_activity` the direct branch uses; that path is pool-based.
   - Metadata matches the direct branch plus `approval_request_id`.

## Steps

- [ ] **Step 1: failing DB tests.** Add `tests/database/approval_activity_test.rs` (wire it in `tests/database/mod.rs`). Copy the setup style of `tests/database/external_change_test.rs`: own team, two environments, a `specific_environments` policy on prod with one required approver, a feature with stages, cleanup at the end. Tests:
  ```rust
  #[tokio::test]
  async fn final_approval_vote_writes_stage_approved_in_the_same_transaction()
  // request DEPLOYMENT_REQUESTED on prod -> approver votes approve ->
  // exactly one activity_log row: activity_type = 'stage_approved', entity_type = 'stage',
  // entity_id = prod stage id, actor_id = approver, metadata.status = 'DEPLOYMENT_APPROVED',
  // metadata.feature_id, metadata.environment_name = prod name, metadata.approval_request_id.

  #[tokio::test]
  async fn rejection_vote_writes_stage_rejected()

  #[tokio::test]
  async fn auto_approval_writes_stage_approved_without_actor_id()

  #[tokio::test]
  async fn cancel_writes_approval_request_cancelled_with_previous_status()

  #[tokio::test]
  async fn gated_request_writes_stage_change_requested_with_approval_request_id()

  #[tokio::test]
  async fn external_approval_still_writes_only_its_own_row()
  // approve_stage_change_externally -> no stage_approved row, one approval_request_approved_externally row.

  #[tokio::test]
  async fn failed_execution_rolls_back_the_activity_row()
  // make execute_change_tx fail (stage deleted between request and vote; the vote path
  // resets the request to pending) -> no stage_approved row.
  ```
- [ ] **Step 2:** run `cargo test -p feature-toggle-backend --test integration_test approval_activity`. Expected: all fail on "0 rows".
- [ ] **Step 3:** changes 1-4 above.
- [ ] **Step 4:** re-run. Expected: pass. Then run the full `cargo test -p feature-toggle-backend` (the activity count asserts in older tests may change; update them only where the new row is the reason, and note each in the handoff log).
- [ ] **Step 5:** `cargo fmt`, `cargo clippy --all-targets`, `graphify update .`. No REST DTO change, so no contract update.
- [ ] **Step 6:** commit `feat(approval): activity rows for approval decisions, cancel and gated requests (JI-40)`.

## Done when

- Each path in the table writes exactly one row with the interface above, in the same transaction as the status change where a transaction exists.
- The Jira external approval path is unchanged.
- fmt, clippy and the full backend test suite pass.
- Handoff log, `../HANDOFF.md` and the phase 2 README table updated (`docs(jira): ...` commit).

## Handoff log

- Rows written in the same transaction as the status change in `apply_vote_tx`, `auto_approve_request`, `approve_capped_request` and the `cancel_request` tx branch, through `record_stage_decision_tx` and `stage_decision_activity` in `logic/approval.rs`. `execute_change_tx` now returns `Result<Option<String>, Error>`; the external path ignores the value.
- Gated request row: `log_gated_stage_change_requested` in `logic/feature.rs` (best effort, pool). Constants `STAGE_CHANGE_REQUESTED` and `APPROVAL_REQUEST_CANCELLED` added; the direct branch uses the constant.
- Non-transaction `apply_vote`, the non-tx `cancel_request` branch and the non-tx `auto_approve_request` branch write no row: `ApprovalLogicImpl` has no activity repository without a pool, and production always wires a pool (`approval_logic_with_pool`). Only the pool-less constructor used by mock unit tests reaches them.
- `actor_name` for vote and cancel is the username (not the full name the notifications use).
- A cancel for a stage deleted meanwhile still succeeds and writes no row.
- No existing test count changed. New tests: `tests/database/approval_activity_test.rs` (7).
