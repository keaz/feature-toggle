# AI-12: UI approval risk panel, badge, and policy mode

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Not started |
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

_No entries yet._
