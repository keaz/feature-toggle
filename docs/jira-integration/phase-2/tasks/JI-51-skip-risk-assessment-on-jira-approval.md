# JI-51: Skip the AI risk assessment of a request Jira approved (backend)

| Field | Value |
|---|---|
| Type | Fix (follow-up from JI-14, found 2026-10-04) |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Depends on | JI-14 |
| Behavior change | A request that Jira approves before its approval-risk assessment finishes gets no assessment. No Jev call is made for it, and no late `approval_risk_assessed` row is written. `aiRisk` is `null` for it. Finished assessments and every request closed by a person are unchanged. |
| Design | [design.md §3.8](../design.md#38-ai-judgments-and-jira-ji-51-ji-52-ji-53), decision J25 |

## Goal

Jira's `approve` action closes the approval request in the same event that created it. The approval-risk assessment queued at creation still called Jev and later wrote "AI risk assessment: high" for a request that was already approved. Stop that: skip the unfinished assessment when Jira approves.

## Current code (verify first)

| Piece | Where |
|---|---|
| Statuses | `migrations/20261002030000_ai_judgments.sql`: `CHECK (status IN ('pending','done','failed'))` |
| Repository | `database/ai.rs`: `AiJudgmentRepository` (`start_attempt`, `mark_done`, `mark_failed` guarded by `status <> 'done'`; `claim_retryable` reads `pending`/`failed`) |
| Service | `judgment/service.rs`: `JudgmentService::run_with_queue_limit` (`start_attempt` false gives `Skipped`; `mark_done` false gives `Stale`, no `apply`) |
| Jira approval | `logic/approval.rs`: `approve_stage_change_externally`, the branch with a pending request (writes `ai_risk_mode_skipped`) |
| REST mapping | `rest/approval.rs`: `map_ai_risk_at` (an unknown status already gives `None`) |

## Changes

1. Tests first:
   - `tests/database/ai_test.rs`: `skip_unfinished_closes_a_pending_row_for_good`. Skip a pending row; it is `skipped` with the error and `completed_at`; `start_attempt`, `mark_done` and `mark_failed` return false; `claim_retryable` does not return it. Also `skip_unfinished_keeps_a_done_row` (returns false, row unchanged).
   - `judgment/service.rs`: `a_run_whose_row_was_skipped_makes_no_call` (`start_attempt` false, client never called).
   - `rest/approval.rs`: `a_skipped_judgment_maps_to_no_assessment`.
   - `tests/database/system_client_approval_test.rs` (or the JI-14 test file): `jira_approval_skips_the_pending_risk_assessment`. Create a request under an `advisory` policy with a pending `approval_risk` row, approve it as Jira, then expect the row `skipped` and the activity metadata `ai_risk_assessment = "skipped"`.
2. Migration `20261004110000_ai_judgments_skipped_status.sql`: drop `ai_judgments_status_check` and add it back with `'skipped'`.
3. `database/ai.rs`: `skip_unfinished(subject_type, subject_id, kind, reason) -> Result<bool, Error>`. Change the guards of `start_attempt`, `mark_done` and `mark_failed` to `status IN ('pending','failed')`.
4. `judgment/service.rs`: `JudgmentService::skip(subject_type, subject_id, kind, reason) -> Result<bool, crate::Error>`.
5. `logic/approval.rs` `approve_stage_change_externally`: when `policy.ai_risk_mode != "off"`, also set `metadata["ai_risk_assessment"] = "skipped"`. After the commit, call `skip` for `(ApprovalRequest, approved.id, ApprovalRisk)` with `skipped: approval request approved by <source> before the assessment ran`. Log a failure with `warn!` and continue.
6. `rest/approval.rs` `map_ai_risk_at`: an explicit `"skipped" => return None` arm, with a comment.
7. Update `docs/ai-judgments/design.md` §4.2 (status list) and §5.1 (apply step).
8. Run `cargo fmt`, `cargo clippy --all-targets` and the backend tests on the test DB (apply the migration first). No contract change.
9. Commit `fix(jira): skip the AI risk assessment of a request Jira approved (JI-51)`.

## Done when

- A Jira-approved request has no assessment: its row is `skipped`, `aiRisk` is `null`, and no `approval_risk_assessed` row is written later.
- A request closed by a vote, a cancel or auto-approval keeps its assessment as before.
- Checks pass. Handoff log, `../../HANDOFF.md` and the README table updated.

## Handoff log

(empty)
