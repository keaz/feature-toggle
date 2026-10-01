# AI-10: Approval risk assessment (advisory)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Not started |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-01 |
| Behavior change | Additive. New response fields and a new policy field. Approval decisions do not change; enforcement is AI-11. |
| Design | [design.md §5.1](../design.md#51-approval-risk-triage-ai-10-ai-11-ai-12) |

## Goal

When an approval request is created, assess its risk with Jev in the background. Store the result and return it on approval request responses. Add the per-policy `ai_risk_mode` field. In this task every mode except `off` behaves like `advisory`.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| What | Where |
|---|---|
| The only creation site (`change_type = "stage_change"`) | `logic/approval.rs`: `ApprovalLogicImpl::maybe_create_stage_change_request`, the `create_request(CreateApprovalRequestInput { ... })` call |
| `change_payload` content: before/after snapshots, diff, risk markers, blast radius, policy, routing | built in the same function before `create_request` |
| Blast radius from stage changes (camelCase JSON) | `logic/approval.rs`: `build_stage_change_blast_radius` |
| Blast radius response fields | `rest/operational_safety.rs`: `BlastRadiusPreviewResponse` (`risk_level`, `affected_environments`, `affected_clients`, `dependency_count`, `evaluation_volume_7d`, `risk_markers`) |
| Diff entries `{path, change_type, before, after}` | `database/feature.rs`: `FeatureVersionDiffEntry` |
| Approval logic constructor | `logic/approval.rs`: `approval_logic_with_pool_and_notifications`, called from `lib.rs::run` |
| Policy entity and inputs | `database/entity.rs` (`ApprovalPolicy`), `database/approval.rs` (`CreateApprovalPolicyInput`, `UpdateApprovalPolicyInput`, policy SELECT/INSERT/UPDATE SQL) |
| Policy REST DTOs and handlers | `rest/approval.rs`: `ApprovalPolicyResponse`, `CreateApprovalPolicyRequest`, `UpdateApprovalPolicyRequest`, `create_approval_policy`, `update_approval_policy`; tx logic in `logic/approval_tx.rs` |
| Request response mapping (also used by the stream) | `rest/approval.rs`: `ApprovalRequestResponse`, `map_request_with_policy`; `rest/stream.rs` |
| List endpoint | `rest/approval.rs`: `list_approval_requests` |
| Activity constants | `utils/activity_logger.rs` |

## Changes

1. **Migration**: `approval_policies.ai_risk_mode` as in design §4.3 (AI-10 line only).
2. **Policy plumbing**:
   - Add `ai_risk_mode: String` to the entity, the inputs, the SQL, and the three DTOs.
   - Validate it against the 4 values and return 400 otherwise.
   - Default is `advisory` on create. On update, leave it unchanged when absent.
3. **Handler** `src/judgment/approval_risk.rs`:
   - `pub fn build_input(feature, stage, environment, change_payload) -> Value`.
     - Produces the input snapshot in design §5.1.
     - Bucket the numbers in code: client count 0/1–3/4–10/>10; 7-day evaluation volume 0/<1k/<100k/≥100k; dependents 0/1–3/>3.
     - Keep at most 30 diff entries and truncate values to 200 characters.
     - Never include owner, user ids, or emails.
   - `ApprovalRiskHandler` implements `JudgmentHandler`:
     - `build` uses the 4 questions with the exact text in design §5.1.
     - `derive` applies the thresholds as named constants and returns `{level, reasons, signals}`.
     - `apply` writes activity `approval_risk_assessed` (add the constant) with metadata `{approval_request_id, feature_id, level, reasons, model}`. It does nothing else in this task.
   - Register the handler with `JudgmentService` in `lib.rs::run`.
4. **Trigger**:
   - Inject `Option<Arc<JudgmentService>>` into `ApprovalLogicImpl` through `approval_logic_with_pool_and_notifications`. Pass `None` in existing tests.
   - After `create_request` returns `Ok(request)` in `maybe_create_stage_change_request`, submit when all of these hold: the service is `Some`, `team_enabled(team, approval_risk)`, and `policy.ai_risk_mode != "off"`.
   - Submission errors are logged and swallowed. The request creation result must not change.
5. **Response fields**:
   - `ApprovalRequestResponse` gains `ai_risk: Option<AiRiskSummary>` (design §5.1) and `required_approvals_effective: i32`. In this task, `required_approvals_effective` equals the policy's `required_approvers`.
   - In `list_approval_requests`, batch-load judgments with `get_for_subjects`; do not query per row.
   - Also fill the fields wherever `map_request_with_policy` is used (single request paths and `rest/stream.rs`).
   - `status` mapping: no row gives `None`; `pending`, `done`, and `failed` are passed through. `level`, `reasons`, and `signals` are only set when `done`.
6. Register the new schemas in `ApiDoc` and update the contract baseline.

## Tests

- `build_input`:
  - The bucket boundaries (0, 1, 3, 4, 10, 11; 999, 1 000, 99 999, 100 000).
  - Diff truncation.
  - The owner is never present.
- `derive` table tests covering each rule:
  - Each high trigger alone.
  - Each medium trigger alone.
  - Low confidence forces at least medium.
  - The low baseline.
- Logic test with mocks: request creation with the team setting on and mode `advisory` calls `submit` once. With mode `off`, with the setting off, or with a `None` service, it does not. Creation still succeeds when `submit` fails.
- Response mapping: none, pending, done, and failed judgments map as specified.
- Live tuning (`#[ignore]`, needs key): `tests/fixtures/ai/approval_risk.json` with at least 20 labelled cases (expected level). Examples:
  - Enabling a payments flag in production for 100% of users: high.
  - Turning off a kill switch: high.
  - Moving a dev-only cosmetic flag: low.
  - A small beta rollout in production: medium.

  Record accuracy and any threshold changes in the handoff log.

## Acceptance criteria

- [ ] Creating a stage-change approval with the feature on produces an `ai_judgments` row that reaches `done` with a real key. `GET /teams/{id}/approval-requests` then shows `aiRisk.level` and `aiRisk.reasons`.
- [ ] With no key or the team setting off, responses have `aiRisk: null`, and creation latency and behavior are unchanged.
- [ ] Approval outcomes are identical to before for every policy mode. Enforcement is not in this task.
- [ ] Contract baseline is updated; all backend tests pass.

## Out of scope

Gating auto-approve and extra approvers (AI-11). UI (AI-12).

## Handoff log

_No entries yet._
