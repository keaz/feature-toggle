# JI-21: UI, `externalRef` and `reason` on stage changes, approvals and activity

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | JI-11, JI-14 (JI-20 for linking known keys) |
| Behavior change | Two optional inputs on the stage request UI; new read-only fields on approvals and activity. |
| Design | [design.md §3.5](../design.md#35-ui-ji-20-ji-21-ji-22) |

## Goal

Let users give a ticket reference and a reason when they request a stage change, and show both (from users or from Jira) to approvers and in the activity feed.

## Current code (verify first, paths under `feature-toggle-ui/src`)

| Piece | Where |
|---|---|
| Stage request call | `api/features.ts`: `requestStageChange(stageId, request, { freezeOverrideReason? })` (~499-508), type `StageChangeRequest` (~270-276) |
| Stage request UI | `pages/FeatureCreate.tsx`: call near ~1021; freeze override textarea near ~2086-2093 |
| Approval type | `api/approvals.ts`: `interface ApprovalRequest` (~99-124: `changeDescription`, `requestedBy`, `aiRisk`, ...) |
| Approvals page | `pages/ApprovalsPage.tsx`: list item renders `changeDescription` (~754); find the detail view |
| Activity feed | `components/Dashboard/ActivityFeed.tsx`: `ActivityFeedMetadata`, `getMetadataValue()` |
| Reason hint | `components/ai/ReasonQualityHint.tsx` (AI-21). Do not attach it to the new reason field in this task; note it as a follow-up. |

## Changes

1. `api/features.ts`: options become `{ freezeOverrideReason?, externalRef?, reason? }`; send only non-empty values.
2. `FeatureCreate.tsx` stage request UI: optional "External reference" (single line, max 100) and "Reason" (textarea, max 1000) inputs, cleared after a successful request.
3. `api/approvals.ts`: `externalRef?: string | null`, `requestReason?: string | null`.
4. `ApprovalsPage.tsx`: show `externalRef` as a badge and `requestReason` as text on the list item and the detail view. If `externalRef` matches a Jira link of that feature (JI-20 API) with a URL, render it as a link; otherwise plain text. Keep it to one extra request per opened detail, not per list row.
5. `ActivityFeed.tsx`: show `external_ref` and `reason` from metadata when present.
6. Approval source (JI-14): `api/approvals.ts` gains `approvalSource?: 'fluxgate' | 'jira' | 'auto'` and `externalApprover?: { system, accountId?, displayName?, issueKey?, status? } | null`. When `approvalSource === 'jira'`, show "Approved by Jira" with the display name, issue key and status on the list item and detail, instead of the approver list.

## Tests (write first)

- `requestStageChange` sends `externalRef`/`reason` only when set (mock `apiJson`).
- Approvals list item renders both fields; absent fields render nothing.
- Activity feed renders `external_ref` / `reason` metadata.
- A Jira-approved request renders "Approved by Jira" with display name and issue key.
- Design token guard passes.

## Done when

- `pnpm lint`, `pnpm build`, `pnpm test:run` pass.
- Checked by hand: request with ref and reason, see it on the approval and in the activity feed.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

(empty)
