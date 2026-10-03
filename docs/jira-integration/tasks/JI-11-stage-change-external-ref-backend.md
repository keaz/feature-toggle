# JI-11: `externalRef` and `reason` on stage changes (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
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

(empty)
