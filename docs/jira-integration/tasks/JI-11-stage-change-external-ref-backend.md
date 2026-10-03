# JI-11: `externalRef` and `reason` on stage changes (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in `e251f9e` |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — (run after JI-10 by project order) |
| Behavior change | Additive. Two optional request fields, two approval-request columns and response fields, extra activity and notification metadata. Requests without the fields behave as today. |
| Design | [design.md §3.3](../design.md#33-externalref-and-reason-on-stage-changes-ji-11) |

## Goal

When Jira (or a user) requests a stage change, record which ticket asked for it and why. Approvers see it on the approval request; the audit trail keeps it.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Request DTO | `rest/feature/types.rs`: `StageChangeRequestBody { request, freeze_override_reason }` |
| Handler | `rest/feature.rs`: `request_stage_change` |
| Logic trait | `logic/feature.rs`: `DeploymentLogic::request_stage_change(stage_id: ID, request: StageChangeRequestType, user_id: Uuid)` (trait near line 223, mock near 373, impl `FeatureLogicImpl` near 1695). Many unit tests call it (`grep -n "request_stage_change(" src/logic/feature.rs`). |
| Other callers | `scheduler/scheduled_changes.rs` (`.request_stage_change(ID::from(stage_id), request, user_id)`), `tests/approval_risk_live_test.rs` |
| Approval creation | `logic/approval.rs`: `ApprovalLogic::maybe_create_stage_change_request(feature, stage, next_status, requested_by)` (trait near 139, impl near 1556), builds `CreateApprovalRequestInput` (`database/approval.rs`) |
| Approval entity / response | `database/entity.rs`: `ApprovalRequest`; `rest/approval.rs`: `ApprovalRequestResponse` and its mapping |
| Approval execution | `logic/approval.rs`: `execute_change`, `execute_change_tx` (applies the stage change after approval). Check whether this path writes an activity row. |
| Stage change activity | `logic/feature.rs`, end of `request_stage_change` impl: `metadata = json!({feature_id, feature_key, stage_id, status, team_id, teamId, ...})` |
| Notification | same impl: `NotificationEvent` with `NOTIFICATION_TYPE_STAGE_CHANGE_REQUESTED` and metadata |
| Approval request SQL | `database/approval.rs`: every `SELECT` that maps to `ApprovalRequest` must read the new columns |

## Changes

1. **Model** `model.rs`: `#[derive(Debug, Clone, Default, PartialEq)] pub struct StageChangeMeta { pub external_ref: Option<String>, pub reason: Option<String> }`.
2. **Validation** (REST layer, small pure fn): trim; blank → `None`; `externalRef` 1–100 chars, `reason` 1–1000 chars; reject control characters except `\n` in `reason`. 400 with a clear message.
3. **DTO** `StageChangeRequestBody`: `pub external_ref: Option<String>`, `pub reason: Option<String>` (camelCase `externalRef`, `reason`).
4. **Logic signature**: `request_stage_change(stage_id, request, user_id, meta: StageChangeMeta)` and `maybe_create_stage_change_request(feature, stage, next_status, requested_by, meta: &StageChangeMeta)`. Update the trait, mock expectations, impl, and every caller. The scheduler passes `StageChangeMeta { external_ref: None, reason: <the scheduled change's reason, if it has one> }`.
5. **Migration** `migrations/<timestamp>_approval_request_external_ref.sql`: `ALTER TABLE approval_requests ADD COLUMN external_ref TEXT NULL, ADD COLUMN request_reason TEXT NULL;`
6. **Persistence**: `CreateApprovalRequestInput` and `ApprovalRequest` gain `external_ref`, `request_reason`; insert and all selects updated. Fix every struct literal in tests.
7. **Response**: `ApprovalRequestResponse` gains `external_ref: Option<String>` and `request_reason: Option<String>` (camelCase). Check the approvals stream payload (`ApprovalRequestEvent`) uses the same response type; if not, add the fields there too.
8. **Activity**: add `external_ref` and `reason` to the stage change activity metadata when present. In the approval execution path, if it writes an activity row, add the request's `external_ref` and `request_reason`. If it writes none, do not add one; record that in the handoff log.
9. **Notification**: add `external_ref` to the `STAGE_CHANGE_REQUESTED` metadata, and append ` Ref: <externalRef>.` to the message when present.
10. Contracts: export and update the baseline.

Out of scope: AI justification recording of `reason` (could reuse AI-20 later; note it in the handoff), auto-creating external links from `externalRef`.

## Tests (write first)

- Validation table test (blank, too long, control chars, trimmed).
- `rest/feature.rs` handler test: body with `externalRef`/`reason` reaches the logic mock as `StageChangeMeta` (`withf`).
- `logic/feature.rs`: with an approval policy, the created request carries `external_ref`/`request_reason`; the activity metadata contains both; without them, metadata has neither key.
- DB integration test: create an approval request with both fields and read it back through the list and get queries.
- `rest/approval.rs`: response JSON contains `externalRef` and `requestReason`.

## Done when

- A request-change with `externalRef` and `reason` shows both on `GET /approval-requests` and in the activity log; one without them behaves as today.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend`, `./scripts/check-contract-compat.sh` pass.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

### 2026-10-03: done in `e251f9e`

What changed:

- `model.rs`: `StageChangeMeta { external_ref, reason }`.
- `rest/feature/types.rs`: `StageChangeRequestBody` gained `external_ref` / `reason` (camelCase `externalRef`, `reason`). `validate_stage_change_meta(external_ref, reason) -> Result<StageChangeMeta, String>` trims, drops blanks, checks length in characters (100 / 1000) and control characters (`\n` allowed only in `reason`). The handler validates right after the role check, before the freeze check, and returns 400 `invalid_input` with the message.
- `DeploymentLogic::request_stage_change(stage_id, request, user_id, meta)` and `ApprovalLogic::maybe_create_stage_change_request(feature, stage, next_status, requested_by, &meta)`. Every caller and mock updated. The scheduler passes `StageChangeMeta { external_ref: None, reason: <trimmed scheduled change reason, None if blank> }`.
- Migration `20261004010000_approval_request_external_ref.sql`: `approval_requests.external_ref`, `approval_requests.request_reason` (both `TEXT NULL`).
- `CreateApprovalRequestInput` and `ApprovalRequest` gained `external_ref` / `request_reason`; both inserts and every `RETURNING`/`SELECT` list read them (`map_request_row`). The `SELECT r.*` queries pick them up automatically.
- `ApprovalRequestResponse` gained `externalRef` / `requestReason`. The approvals stream uses the same `map_request_with_policy`, so it carries them too.
- Activity: the direct (not gated) stage change activity row gets `external_ref` and `reason` in metadata when present; without them neither key exists.
- Notifications: both `STAGE_CHANGE_REQUESTED` dispatches (gated and direct) add `external_ref` to metadata when present and append ` Ref: <externalRef>.` to the message.
- Contract baseline updated.

Findings:

- **The gated path writes no activity row** at request time (`request_stage_change` returns after creating the approval request), and **approval execution writes none either** (`execute_change`, `execute_change_tx` only call `approve_or_reject_stage_change[_tx]`). Per the task, no activity row was added. For a gated request the ref and reason are visible on the approval request (`GET /approval-requests`) and in the notification, not in the activity log. JI-14 may want an activity row for Jira approvals; it can read `request.external_ref` / `request.request_reason` there.
- The logic test "with an approval policy ... the activity metadata contains both" was split accordingly: the gated test checks that the meta reaches `maybe_create_stage_change_request` and the notification; the direct tests check the activity metadata.
- Out of scope, not done: recording `reason` as an AI justification (AI-20 could reuse it), auto-creating external links from `externalRef`.

Verified (test DB `feture_toggle_test`, migration applied):

- `cargo fmt --check`: clean.
- `cargo clippy --all-targets`: no warnings in changed lines (existing warnings elsewhere unchanged).
- `cargo test -p feature-toggle-backend`: all pass (783 lib tests, integration suite, contract test).
- `./scripts/check-contract-compat.sh`: pass after the baseline update.

New tests: `rest::feature::types::tests::stage_change_meta_validation`, `stage_change_body_reads_external_ref_and_reason`; `rest::feature::tests::stage_change_meta::*` (handler passes meta to the logic mock; 400 on a 101-character ref); `logic::feature::test::{direct_stage_change_records_external_ref_and_reason, direct_stage_change_without_meta_adds_no_keys, gated_stage_change_passes_meta_to_the_approval_request}`; `logic::approval::tests::test_maybe_create_stage_change_request_emits_event` (asserts the create input); `rest::approval::tests::map_request_returns_external_ref_and_request_reason`; DB tests in `tests/database/approval_workflow_test.rs` (gated request read back through get and list, request without the fields, direct activity metadata).
