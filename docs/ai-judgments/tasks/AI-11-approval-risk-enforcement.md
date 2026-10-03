# AI-11: Approval risk enforcement (gate auto-approve, extra approver)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 8cde72c. Maintainer sign-off given 2026-10-02. |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-10 |
| Behavior change | **Yes, user-visible.** For policies set to `gate_auto_approve` or `require_extra_approver`, high-risk requests are no longer auto-approved, or need one more approval. |
| Design | [design.md §5.1, "Policy enforcement"](../design.md#51-approval-risk-triage-ai-10-ai-11-ai-12) |

## Goal

Make the two enforcing policy modes work, while keeping fail-open behavior (decision D3).

## Current code (verify first, paths under `feature-toggle-backend/src`)

| What | Where |
|---|---|
| Auto-approval query | `database/approval.rs`: `list_requests_due_for_auto_approval` (`WHERE r.status = 'pending' AND p.auto_approve_after_hours IS NOT NULL AND r.created_at + make_interval(...) <= NOW()`) |
| Auto-approval scheduler | `scheduler/auto_approval.rs`: `run_pending`, then `ApprovalLogic::auto_approve_request` |
| Vote counting (status becomes approved when `approved_count + 1 >= $3`) | `database/approval.rs`: `record_vote(..., required_approvers: i32)`, the `UPDATE approval_requests ... RETURNING` query |
| Callers that pass `required_approvers` | `logic/approval.rs`: `apply_vote`, `apply_vote_tx` |
| Request entity and row mapping | `database/entity.rs` (`ApprovalRequest`), `database/approval.rs` (`map_request_row`) |
| AI-10 apply step to extend | `judgment/approval_risk.rs`: `ApprovalRiskHandler::apply` |

## Changes

1. **Migration**: `approval_requests.required_approvers_override INT NULL` (design §4.3, AI-11 line). Add the field to the entity and `map_request_row`.
2. **Gate auto-approve**: in `list_requests_due_for_auto_approval`, add:
   ```sql
   AND NOT (
     p.ai_risk_mode = 'gate_auto_approve'
     AND EXISTS (
       SELECT 1 FROM ai_judgments j
       JOIN team_ai_settings s ON s.team_id = j.team_id AND s.approval_risk
       WHERE j.subject_type = 'approval_request' AND j.subject_id = r.id
         AND j.kind = 'approval_risk' AND j.status = 'done'
         AND j.derived->>'level' = 'high'))
   ```
   Pending, failed, or missing judgments do not exclude the request.
3. **Extra approver**:
   - In `ApprovalRiskHandler::apply`, load the request and its policy.
   - If `request.status = 'pending'`, `policy.ai_risk_mode = 'require_extra_approver'`, and `derived.level = 'high'`, run `UPDATE approval_requests SET required_approvers_override = $1 WHERE id = $2 AND status = 'pending' AND required_approvers_override IS NULL` with `$1 = policy.required_approvers + 1`.
   - Add a repository method for it.
4. **Votes**: in `apply_vote` and `apply_vote_tx`, pass `request.required_approvers_override.unwrap_or(policy.required_approvers)` to `record_vote`. Do not change the SQL in `record_vote`.
5. **Response**: `required_approvals_effective` (added in AI-10) now uses the override when present.
6. **Activity**: when the override is set, add `"required_approvers_override": n` to the `approval_risk_assessed` metadata.
7. Admin override paths stay unchanged. Verify by reading `admin_override_enabled` handling and add a test.

## Tests

- Auto-approval query (DB test): a request is excluded only when mode is `gate_auto_approve`, the judgment is done with level high, and the team setting is on. It is included when the judgment is pending or failed, when it is missing, when the level is medium, when the team setting is off, or when the mode is `advisory`.
- Override:
  - Set only for `require_extra_approver` with high level on a pending request.
  - Not set when the request is already approved.
  - Not changed by a second apply.
- Votes: with `required = 1` and override 2, the first approve leaves the request pending and the second approves it. Without an override, the behavior is unchanged.
- An existing `tests/auto_approval_scheduler_test.rs` case still passes.

## Acceptance criteria

- [ ] Maintainer sign-off is recorded in the PR.
- [ ] With TypeSafe down (no key, or forced client errors), auto-approval and vote counts behave exactly as before.
- [ ] Every case in the test list passes.
- [ ] Contract baseline is updated if response schemas changed.

## Out of scope

UI display of the effective approval count (AI-12).

## Handoff log

### 2026-10-02: done in `8cde72c`

Maintainer sign-off: the maintainer approved implementing and committing this task on `main` on 2026-10-02.

**What changed:**

- Migration `20261002060000_approval_request_required_approvers_override.sql` adds `approval_requests.required_approvers_override INT NULL`. `ApprovalRequest` and `map_request_row` carry it; every request `SELECT`/`RETURNING` list in `database/approval.rs` includes it.
- `list_requests_due_for_auto_approval` excludes a request when its policy is `gate_auto_approve` and a `done` approval-risk judgment has `level = high` and the team's `approval_risk` setting is on. Pending, failed, missing, medium or low judgments, setting off, and other modes do not exclude.
- `ApprovalRepository::set_required_approvers_override(request_id, n) -> bool` runs `UPDATE ... WHERE id = $1 AND status = 'pending' AND required_approvers_override IS NULL`. It returns whether a row changed.
- `ApprovalRiskHandler::apply`: for `level = high` on a `pending` request it loads the policy; under `require_extra_approver` it sets the override to `policy.required_approvers + 1`. A request that already has an override is left alone. The `approval_risk_assessed` metadata gains `required_approvers_override` when the request carries one (set by this apply, or already set).
- `apply_vote` and `apply_vote_tx` pass `request.required_approvers_override.unwrap_or(policy.required_approvers)` to the vote repository. The vote SQL is unchanged.
- `required_approvals_effective` in `rest/approval.rs` prefers the override, then the policy, then the payload snapshot. It is used by `map_request_with_policy`, so the list, approve/reject/cancel responses, and the WebSocket stream all show it.
- The `apply` activity is still written for a closed request (history). Only the override write is guarded by `status = 'pending'`; the guard is in SQL, so a request closed between the read and the write is not changed.
- Admin override is unchanged: `ensure_user_can_vote` only widens who may vote. An admin vote still counts as one approval against the overridden requirement (unit test `admin_override_does_not_lower_the_overridden_requirement`).

**Verified:**

- Unit tests (mocks): handler sets the override only for `require_extra_approver` + `high` + pending; not for other modes or levels, not for approved/rejected/cancelled, not changed by a second apply, no metadata when the update loses a race. Vote tests: `add_vote` receives the override, or the policy value without one, also with admin override. REST mapping prefers the override.
- DB tests in `tests/database/approval_risk_enforcement_test.rs`: the 9-case auto-approval matrix, gate lifted when the policy leaves gate mode, override set once and only on a pending request, and an override of 3 keeps a request pending after two approvals on both the pool (tx) and non-pool vote paths.
- `tests/auto_approval_scheduler_test.rs` passes. Full `cargo test -p feature-toggle-backend` on `feture_toggle_test`: all pass (716 lib, 265 integration). Clippy has no new warnings.
- Contract: no DTO or schema change (`requiredApprovalsEffective` already existed), so the baseline is unchanged and `contract_compatibility_test` passes.
- Fail open: with no judgment (TypeSafe down, no key) the due query and vote counts behave as before.

**Open questions and notes:**

- The override is `required_approvers + 1` without checking eligible approvers. If a policy has exactly `required_approvers` eligible approvers, a high-risk request cannot reach the override by votes alone. An admin override voter or cancel still works; a policy edit does **not**, because the override is stored on the request row and is never cleared (corrected 2026-10-03). Superseded by the 2026-10-03 entry below: the override is now capped.
- The override is read at apply time from the policy's current mode. Changing a policy to `require_extra_approver` later does not affect requests already assessed.
- `gate_auto_approve` re-evaluates on every scheduler run, so switching the policy mode makes the held request eligible again.
- The test DB must have the new migration applied (`sqlx migrate run`); `init_pg_pool` does not migrate.

**For later tasks:** AI-12 UI needs no change; it already shows the hint once `requiredApprovalsEffective` exceeds the policy's `requiredApprovers`.

### 2026-10-03: final-review fixes (user decisions)

Two user decisions, both dated 2026-10-03, change this task's behavior:

1. **Override capped at the eligible approver count.** `ApprovalRiskHandler::enforce_extra_approver` now sets `required_approvers_override = min(policy.required_approvers + 1, len(request.eligible_approver_ids))` when the request's eligible approver list is non-empty. When the list is empty, it keeps `required_approvers + 1` (corrected 2026-10-03: the list is empty only for a request created without a database pool or a legacy row from before routing; routing resolves role members into the list, so role-based policies are capped too). When the cap leaves the value at or below `required_approvers`, no override is written (it never lowers the requirement) and the `approval_risk_assessed` metadata gets `"extra_approver_skipped": "no_additional_eligible_approver"`. Pure helper `extra_approver_requirement`; unit tests `the_extra_approver_is_capped_at_the_eligible_approver_count`, `no_override_when_no_additional_approver_is_eligible`, `a_request_without_an_eligible_list_gets_required_plus_one` (renamed 2026-10-03 from `a_role_routed_request_gets_required_plus_one`).
2. **`require_extra_approver` also blocks auto-approval.** `list_requests_due_for_auto_approval` now excludes a request when its policy mode is `gate_auto_approve` **or** `require_extra_approver` and a `done` approval-risk judgment has `level = high` with the team setting on. The DB matrix test (`enforcing_modes_exclude_only_done_high_risk_with_the_team_setting_on`, 13 cases) covers both modes, pending/medium/setting-off for `require_extra_approver`, and `off`.

The earlier note that "a policy edit still works" as an escape was wrong: the override lives on the request and a policy edit does not clear it. With the cap, votes alone can always approve a request whose eligible approvers are listed, as long as they stay eligible (see the next entry for approvers who leave). A request with an empty list (no pool at creation, or a legacy row) can still need one more approver than exists; admin override and cancel remain the escapes there. (Corrected 2026-10-03: this paragraph first said "role-routed request"; role-routed requests list their approvers and are capped.)

### 2026-10-03: known-issue fix `bbca40a` (approvers who can still vote)

`eligible_approver_ids` is a snapshot taken at creation. If an eligible approver was later disabled, removed from the team, or lost the Approver role (or the policy no longer names them), a capped override could still need more votes than anyone could cast, and the request stayed pending until an admin override or cancel.

- **Effective requirement.** `logic::approval::effective_required_approvals(policy_required, override, approved_count, remaining)` = the policy without an override; with one, `max(policy_required, min(override, approved_count + remaining))`. `remaining` comes from `ApprovalRepository::count_remaining_eligible_approvers`: snapshot approvers who still qualify under the request's current policy and have not voted. The qualification SQL is `database::approval::approver_qualifies_sql`, which routing (`resolve_approval_routing`) now uses too, so both apply the same checks. Unknown `remaining` (empty list) keeps the override.
- **Votes.** `apply_vote` and `apply_vote_tx` compute the requirement with `required_approvals` before `add_vote`/`add_vote_tx`; the vote SQL is unchanged. Modes `off` and `advisory` never have an override, so they are unchanged.
- **Response.** `requiredApprovalsEffective` uses the same function. List, approve/reject/cancel responses and the approvals stream read `remaining` in one batched query, only for pending requests with an override and a non-empty list (`rest::approval::load_remaining_approvers`, fails open to the uncapped override).
- **Reconciliation.** `AutoApprovalScheduler::reconcile_capped_requests` (same 60 s tick as auto-approval) lists `list_capped_requests_ready_for_approval` (pending, override set, non-empty list, `policy required <= approved < override`, nobody left to vote) and calls `ApprovalLogic::approve_capped_request`: one `UPDATE ... WHERE status = 'pending' AND <same condition>` inside a transaction, then the change is executed and an `approval_requirement_reconciled` activity is written (metadata `approval_request_id`, `reason: "no_remaining_eligible_approver"`, `approved_count`, `required_approvers_override`, `policy_required_approvers`, `required_approvals_effective`). Closed requests never match; without a database pool it does nothing.
- **Tests.** Unit: `effective_requirement_*`, `vote_counts_against_the_approvers_who_can_still_vote`, `the_capped_requirement_never_drops_below_the_policy`, REST `required_approvals_effective_caps_the_override_at_who_can_still_vote`, `remaining_approvers_are_loaded_only_for_overridden_pending_requests`, scheduler `reconciliation_approves_each_ready_request`. DB (`approval_risk_enforcement_test.rs`): approver loses the role before voting (both vote paths), approver disabled after one approval then reconciled, never below the policy, advisory unchanged, closed request untouched.
- No DTO or endpoint change; the contract baseline is unchanged.
