# AI-10: Approval risk assessment (advisory)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 69ef888 |
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

### 2026-10-02, Claude (AI-10 implementation)

**What changed** (commit `69ef888`):

- Migration `20261002040000_approval_policy_ai_risk_mode.sql` adds `approval_policies.ai_risk_mode` (default `advisory`, CHECK on the 4 values). Entity, inputs, all policy SQL, and the three DTOs carry it. Create defaults to `advisory`; update keeps the stored value when the field is absent; unknown values give 400.
- `judgment/approval_risk.rs`: `build_input` (bucketed numbers, 30 diff entries, 200-char values, no owner or ids), `ApprovalRiskHandler` (4 questions with the design text, `derive` with named threshold constants, `apply` writes `approval_risk_assessed`).
- `ApprovalLogicImpl` takes `Option<Arc<JudgmentService>>` (last argument of `approval_logic_with_notifications` and `approval_logic_with_pool_and_notifications`; `approval_logic` and `approval_logic_with_pool` pass `None`). `submit_risk_assessment` runs right after `create_request` succeeds. Every error is logged and swallowed.
- `lib.rs::run` builds the judgment service (with the handler) before the logic services.
- Responses: `ApprovalRequestResponse.aiRisk` and `requiredApprovalsEffective`. `list_approval_requests` loads judgments with one `get_for_subjects` call. `approve`/`reject`/`cancel` and the WebSocket stream fill the fields too. `AiJudgmentRepository` is now a `web::Data` registered in `lib.rs`.
- `ApiDoc` has `AiRiskStatus` and `AiRiskSummary`; contract baseline updated.

**Decisions and behavior to know:**

- Every mode except `off` behaves like `advisory`. `requiredApprovalsEffective` is the policy's `required_approvers` (falls back to the policy snapshot in `change_payload` when the policy row is gone, then to 1).
- `apply` fetches the request through `ApprovalRepository::get_request_by_id` for the feature id, and skips when the request no longer exists. It logs the activity even when the request is already closed (history, not a decision). The activity uses `entity_type = feature`, `entity_id = feature id`, no actor, so it shows in the feature history.
- AI-11 extends `ApprovalRiskHandler::apply` (it already holds the approval repository). It has no pool or transaction yet; the `required_approvers_override` update needs a new repository method.
- The stream (`rest/stream.rs`) used to map requests with no policy. It now loads the policy per request (like the list endpoint) so `policy` and `requiredApprovalsEffective` are right there too. `policy` in stream messages is therefore populated now (additive).
- Reason strings: a reason is listed when its signal reaches the threshold of the rule it triggers (widens >= 0.7, safety >= 0.7, sensitive >= 0.7 or (>= 0.6 with widens >= 0.7), impact >= 1.3, confidence < 0.3).
- Missing answers count as 0 with zero confidence, so an incomplete answer set reads as medium ("Assessment uncertain").
- `feature.description` and `feature.purpose` are cut to 500 characters (not in the design; keeps the state small).
- In a test module that imports `actix_web::test`, `#[test]` resolves to the actix module: use `#[actix_web::test]` (as `rest/approval.rs` tests do).

**Verified:**

- `cargo test -p feature-toggle-backend` on `feture_toggle_test`: all pass (589 lib tests, 247 integration tests). Clippy has no new warnings. Contract check passes.
- Live tuning: `cargo test -p feature-toggle-backend --test approval_risk_live_test -- --ignored --nocapture`. Fixture `tests/fixtures/ai/approval_risk.json` has 26 labelled cases (8 high, 8 medium, 10 low). Accuracy 24/26 = 0.92 with the design thresholds unchanged (min asserted 0.7). Confusion (rows expected): low 9/1/0, medium 0/7/1, high 0/0/8. The two misses: "new invoice PDF layout for early customers" (medium expected, high: billing read as sensitive 0.92) and "seasonal banner rolled back in production" (low expected, medium: overall_risk 1.77). Both defensible; no threshold change.
- Live end-to-end: `live_stage_change_request_gets_a_done_assessment` (ignored, needs key and the seeded test DB) creates a stage-change request through `ApprovalLogic`, waits for the judgment to reach `done`, and checks the `approval_risk_assessed` activity. It passed (level `high` for a payments flag).

**For later tasks:**

- AI-12 reads `aiRisk` (`status`, `level`, `reasons`, `signals`, `model`, `assessedAt`) and `requiredApprovalsEffective`; the policy DTOs have `aiRiskMode`.
- The seeded policy needs 2 eligible approvers when the logic has a DB pool. Tests that create requests through a pool-based logic need approvers; the pool-less constructor skips that check.
- Not done (out of scope): enforcement (AI-11), UI (AI-12). The api-tests were not changed.
