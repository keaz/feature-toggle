# JI-53: Record stage change reasons for the justification check (backend)

| Field | Value |
|---|---|
| Type | Feature (follow-up from JI-47, found 2026-10-04) |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Depends on | JI-47 |
| Behavior change | A stage change reason typed by a person is checked after the change, like emergency, cleanup and freeze override reasons. The verdict lands in the activity row's `metadata.ai_justification`. Reasons that FluxGate generates for Jira status rules are not checked. No API change. |
| Design | [design.md §3.8](../design.md#38-ai-judgments-and-jira-ji-51-ji-52-ji-53), decision J27 |

## Goal

JI-47 shows the reason hint before submit, but the reason was not recorded after submit, so the audit trail had no verdict for stage changes. Record it, but only for reasons a person wrote.

## Current code (verify first)

| Piece | Where |
|---|---|
| Meta | `model.rs`: `StageChangeMeta { external_ref, reason }` |
| REST | `rest/feature.rs`: `perform_stage_change` (stage route and by-key route) |
| Jira | `logic/external_change.rs`: builds `StageChangeMeta` with `reason: Some(ctx.reason)` (`Jira status '<status>'`) |
| Logic | `logic/feature.rs`: `request_stage_change` (direct branch `log_activity`, approval branch `log_gated_stage_change_requested`), constructors `feature_logic_with_approval_and_notifications` (only `lib.rs` calls it) |
| Recording | `judgment/justification.rs`: `record_justification`, `ReasonKind::StageChange`, handler merges `ai_justification` into an `activity` subject |

## Changes

1. Tests first, in `logic/feature.rs` (unit, with a `JudgmentService` built from mocks, like the `justification_recording` tests in `rest/feature.rs`):
   - `direct_stage_change_records_a_person_reason`: `check_reason = true`, a reason, the team toggle on, so a justification row is upserted for the new activity row's id with kind `stage_change`.
   - `gated_stage_change_records_the_reason_on_the_requested_row`: same, for the `stage_change_requested` row of a request with an approval.
   - `generated_reasons_are_not_recorded`: `check_reason = false`, so no upsert.
   - `rest/feature.rs`: `perform_stage_change` passes `check_reason = true` (extend `request_change_passes_external_ref_and_reason_to_the_logic`).
2. `StageChangeMeta.check_reason: bool`, with a doc comment. `types::validate_stage_change_meta` sets it to `true`. `external_change.rs` sets it to `false`.
3. `FeatureLogicImpl.judgments: Option<Arc<JudgmentService>>`. `feature_logic_with_approval_and_notifications` takes it (pass `judgment_service.clone()` in `lib.rs`); the other constructors pass `None`.
4. `request_stage_change`: write the activity rows through `create_activity` to get the row id (still best effort). Then, when `check_reason` is on and a reason is present, call `record_justification` with `SubjectType::Activity`, the row id, `ReasonKind::StageChange`, the reason and the feature key. Do this in the direct branch and in the approval branch.
5. Update `docs/ai-judgments/design.md` §5.2 (reason kinds table).
6. Run `cargo fmt`, `cargo clippy --all-targets` and the backend tests on the test DB. No contract change.
7. Commit `feat(ai): record stage change reasons for the justification check (JI-53)`.

## Done when

- A person's stage change reason gets a justification verdict on its activity row. Jira-generated reasons never do.
- With AI off (no key, or the team toggle off), nothing changes.
- Checks pass. Handoff log, `../../HANDOFF.md` and the README table updated.

## Handoff log

(empty)
