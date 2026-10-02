# AI-12: UI approval risk panel, badge, and policy mode

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Done in UI `fb3a756` |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | AI-10 and AI-02. AI-11 is needed only for a non-default `requiredApprovalsEffective`; the UI just displays the field. |
| Behavior change | New UI only |
| Design | [design.md §5.1](../design.md#51-approval-risk-triage-ai-10-ai-11-ai-12) |

## Goal

Show the AI risk assessment on approval requests. Let admins choose `aiRiskMode` per approval policy.

## Current code (verify first, paths relative to `feature-toggle-ui/src`)

| What | Where |
|---|---|
| Approvals page and card list | `pages/ApprovalsPage.tsx`: `ApprovalsPage`, request `Card` render, status `Badge` in the card header |
| Expanded detail: badges row with `riskMarkers` (labels in `RISK_MARKER_LABELS`), then the Blast radius block | `pages/ApprovalsPage.tsx`, the `isSelected &&` section. Insert between the badges row and Blast radius. |
| Polling: paged `useSharedQuery` every 30 s; `usePendingApprovalRequests` | `pages/ApprovalsPage.tsx`, `hooks/usePendingApprovalRequests.ts` |
| `ApprovalRequest` type and API functions | `api/approvals.ts` |
| Policy types | `types/approval.ts` (`ApprovalPolicy`, inputs, form state) |
| Policy form: field state, submit payload builder, create/update calls | `components/modals/ApprovalPolicyFormModal.tsx` |
| Policy table | `components/tables/ApprovalPolicyTable.tsx` |
| Feature detail approval list | `pages/FeatureDetail.tsx` (`ApprovalList`) |

## Changes

1. **Types**:
   - `api/approvals.ts`: add `ApprovalRequest.aiRisk?: AiRiskSummary | null` and `requiredApprovalsEffective?: number`.
   - Define `AiRiskSummary { status: 'pending' | 'done' | 'failed'; level?: 'low' | 'medium' | 'high'; reasons: string[]; signals?: Record<string, number>; model?: string; assessedAt?: string }`.
   - `types/approval.ts`: add `aiRiskMode: 'off' | 'advisory' | 'gate_auto_approve' | 'require_extra_approver'` to the policy, its inputs, and its form state.
2. **`components/approvals/AiRiskPanel.tsx`**, shown only when `useAiFeatures(teamId).approvalRisk` is true and `aiRisk` is present:
   - `pending`: a skeleton line, "Assessing risk…".
   - `done`: a level badge (low, medium, or high, with token colors such as `bg-success/10`, `bg-warning/10`, `bg-destructive/10`) and a reasons list. A collapsible "Signals" area shows each signal as a percentage, plus the model name.
   - `failed`: muted text "Risk assessment unavailable. Approval works as usual."
   - Footer: "AI assessment. Advisory unless the policy says otherwise." Show the policy mode when known.
3. **Placement**:
   - Render `AiRiskPanel` in the expanded detail between the badges row and Blast radius.
   - Add a compact level badge next to the status badge in the card header when the level is known.
   - Optionally add the same compact badge in `FeatureDetail`'s `ApprovalList`.
4. **Pending refresh**: while any visible request has `aiRisk.status === 'pending'`, use a 5 s `pollInterval` on the approvals query; otherwise keep the current 30 s.
5. **Effective approvals**: where the UI shows required or received approvals, use `requiredApprovalsEffective` when present. When it exceeds the policy value, show the hint "+1 approver required: high AI risk".
6. **Policy form**:
   - Add a select "AI risk mode" with options:
     - Off
     - Advisory: show only
     - Block auto-approve when high
     - Require one extra approver when high
   - Show it only when `useAiFeatures().available`. Include the field in the create and update payloads.
   - Show a short help text that the default is Advisory and enforcement fails open.
   - Show the mode in `ApprovalPolicyTable` as a small column or badge.

## Tests

- `AiRiskPanel`: renders the pending, done (each level), and failed states; renders nothing when the AI feature is off or `aiRisk` is absent.
- Approvals page: the poll interval switches to 5 s when a pending assessment is present (mock timers or assert the hook argument).
- Policy modal: the select appears only when AI is available, and the payload includes `aiRiskMode`.

## Acceptance criteria

- [ ] Lint, build, and tests pass, including the design-token guard.
- [ ] With the backend AI off, the approvals UI looks and behaves exactly as before.
- [ ] With AI on, a new request shows "Assessing risk…" and then the level and reasons within about 10 s, without a manual reload.

## Out of scope

Backend logic. Any change to vote buttons or flows.

## Handoff log

### 2026-10-02, Claude (AI-12 implementation)

**What changed** (UI commit `fb3a756`):

- Types: `AiRiskSummary`, `ApprovalRequest.aiRisk`, `requiredApprovalsEffective` in `api/approvals.ts`; `AiRiskMode` and `aiRiskMode` on policy, create and update inputs, and form state in `types/approval.ts`. `ApprovalPolicy.aiRiskMode` is optional in the type so older fixtures still compile; the server always sends it.
- `components/approvals/AiRiskPanel.tsx`: `AiRiskPanel` (pending skeleton "Assessing risk…", done level badge plus reasons plus collapsible Signals and model, failed muted text, advisory footer with the policy mode) and `AiRiskLevelBadge` (compact badge in the card header, only for a done assessment with a level). Both render nothing unless `useAiFeatures(teamId).approvalRisk` is on. Panel sits between the badges row and Blast radius.
- Poll: `ApprovalsPage` uses 5 s while AI is on and any listed request is `pending`, 30 s otherwise (state set from the data, so it returns to 30 s once nothing is pending). Applies to both the pending query and the paged query.
- Effective approvals: the policy block shows `requiredApprovalsEffective` when present. When it exceeds the policy `requiredApprovers`, the card shows "+1 approver required: high AI risk" (or "+N approvers ..."). It is not gated on the AI flag, because the raised requirement stays true after AI is turned off.
- Policy form: "AI risk mode" select (4 options, help text: default is Advisory, enforcement fails open), shown only when `useAiFeatures().available`. `aiRiskMode` is in the create/update payload only when the select is shown; otherwise it is omitted (server default on create, unchanged on update). Policy table has an "AI Risk" column when AI is available (`utils/aiRiskModes.ts` holds labels).

**Decisions and behavior to know:**

- `ApprovalRequest.policy` (the summary) has no `aiRiskMode`, so the panel footer gets the mode from `fetchApprovalPolicies`, fetched through `useSharedQuery` only when AI is on and a request is expanded. The footer omits the mode until it loads. If the backend later adds `aiRiskMode` to `ApprovalPolicySummaryResponse`, use it and drop that fetch.
- Signals: `overall_risk` is a 0 to 3 score, not a probability, so it shows as "2.4 / 3". The other signals show as percentages.
- Optional FeatureDetail `ApprovalList` badge skipped (FeatureDetail.tsx has an unrelated uncommitted user change).
- Pre-existing: `ApprovalsPage` strips `?requestId=` on first render because the list is empty at that point, so deep links to a request do not open details.

**Verified:** `pnpm lint`, `pnpm build` (tsc -b plus vite), `pnpm test:run` (82 files, 642 tests) pass, including the design-token guard. New tests: `AiRiskPanel.test.tsx` (12), `ApprovalsAiRisk.test.tsx` (7, fake-timer poll 5 s then back to 30 s), 5 policy modal tests, 2 policy table tests.


- 2026-10-02 (final-review fix): an assessment pending for 10 minutes or more no longer keeps the 5 s poll (constant `AI_ASSESSMENT_PENDING_MAX_AGE_MS` in `ApprovalsPage`) and the panel shows the unavailable text; `AiRiskPanel` keeps an always-mounted `role=status` region; `AiRiskSummary` nullable fields typed `T | null`. UI commits c22376d, 627620e, dc16a2f.
