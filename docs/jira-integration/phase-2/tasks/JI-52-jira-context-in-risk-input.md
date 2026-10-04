# JI-52: Jira reference and reason in the AI risk input (backend)

| Field | Value |
|---|---|
| Type | Feature (follow-up from JI-11, found 2026-10-04) |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Depends on | JI-11 |
| Behavior change | The approval-risk input sent to Jev gets `change.external_ref` and `change.reason`. The questions, the derivation and the API are unchanged. Only new requests are affected. |
| Design | [design.md §3.8](../design.md#38-ai-judgments-and-jira-ji-51-ji-52-ji-53), decision J26 |

## Goal

A stage change from Jira, or from a person who gives a ticket and a reason, carries `external_ref` and `request_reason` on the approval request (JI-11). The approval-risk assessment did not pass them on, so Jev rated the change without knowing which issue asked for it or why.

## Current code (verify first)

| Piece | Where |
|---|---|
| Input builder | `judgment/approval_risk.rs`: `build_input(feature, stage, environment, change_payload)`, `MAX_TEXT_CHARS`, `truncate_chars` |
| Caller | `logic/approval.rs`: `submit_risk_assessment` (has the `ApprovalRequest`) |
| Shape tests | `build_input_has_the_designed_shape`, `input_never_contains_owner_or_user_data`, `build_input_tolerates_a_bare_payload` |

## Changes

1. Tests first, in `judgment/approval_risk.rs`:
   - Update `build_input_has_the_designed_shape`: `change` has `"external_ref": "PROJ-123"` and `"reason": "Ready for release"`.
   - `build_input_sends_null_reference_and_reason_when_absent`.
   - `build_input_cuts_a_long_reason_to_500_characters` (count characters, not bytes).
2. `build_input` gets `external_ref: Option<&str>` and `reason: Option<&str>`. Add both keys to `change`, always present, with `reason` cut to `MAX_TEXT_CHARS`.
3. `submit_risk_assessment` passes `request.external_ref.as_deref()` and `request.request_reason.as_deref()`.
4. Update the input snapshot in `docs/ai-judgments/design.md` §5.1.
5. Run `cargo fmt`, `cargo clippy --all-targets` and the backend tests on the test DB. No contract change.
6. Commit `feat(ai): Jira reference and reason in the approval risk input (JI-52)`.

## Done when

- The input of a new request with a reference and a reason contains both. A request without them sends `null`.
- Checks pass. Handoff log, `../../HANDOFF.md` and the README table updated.

## Handoff log

(empty)
