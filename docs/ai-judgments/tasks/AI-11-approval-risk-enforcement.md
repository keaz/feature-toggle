# AI-11: Approval risk enforcement (gate auto-approve, extra approver)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Not started. **Needs maintainer sign-off before merge.** |
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

_No entries yet._
