# JI-21: UI, `externalRef` and `reason` on stage changes, approvals and activity

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done (UI `b603188`) |
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

### 2026-10-03, UI `b603188`

- `requestStageChange(stageId, request, StageChangeOptions)`: options `{ freezeOverrideReason?, externalRef?, reason? }`; each is trimmed and sent only when non-empty.
- `FeatureCreate.tsx`: "Stage Change Details" panel (`#stage-change-external-ref`, max 100; `#stage-change-reason`, max 1000) shown for a selected stage when the user can request or approve. Values go with any action from the Actions menu and are cleared after a successful request. They are not cleared when the user selects another stage.
- `ApprovalRequest` gained `externalRef`, `requestReason`, `approvalSource` (`ApprovalSource`) and `externalApprover` (`ExternalApprover`). **The backend sends `externalApprover` with snake_case keys** (`account_id`, `display_name`, `issue_key`, `status`), not the camelCase the task text listed; the type matches the backend.
- `components/approvals/ApprovalRequestContext.tsx` (`variant: 'list' | 'detail'`) renders ref badge, reason and the Jira approval badge; helpers `getJiraApproval` and `findExternalRefUrl` in `utils/approvalRequestContext.ts`. The detail fetches links with `listExternalLinks` under the shared key `feature-external-links:<featureId>` (same key as `JiraLinksCard`), only when the opened request has an `externalRef`. A ref is linked only when it equals (upper case) a `jira` link key with a URL.
- Jira-approved requests: "Approved by Jira (<display name or account id>, <issue key or externalRef>)" on the list item and detail; the detail adds "Jira status: <status>" and shows the Jira line in place of the votes.
- `ActivityFeed.tsx` shows `external_ref` (badge) and `reason` (text) from any activity metadata. Other activity types that already carry `reason` (emergency actions, freeze) now show it too.
- Gated stage requests write no activity row (JI-11), so ref/reason reach the activity feed only for direct (ungated) changes.
- Tests: `api/features.test.ts`, `components/__tests__/ActivityFeed.test.tsx`, `pages/__tests__/ApprovalsExternalRef.test.tsx`, one new case in `FeatureCreate.test.tsx`. `pnpm lint`, `pnpm build`, `pnpm test:run` (86 files, 685 tests) pass.
- Manual check (backend on `:8080` against `feture_toggle_test`, `pnpm dev --port 8090`, Chrome DevTools MCP): a requester (Requester role; `api-test-admin` has no Requester role) requested a production deployment with `PROJ-12` and a reason; the approval list and detail showed both, and the detail linked `PROJ-12` to its Jira link URL. An ungated dev request with `OPS-7` showed the ref and reason in Recent Activity. The Jira-approved display was checked by tests only, not by hand. Data left in the test DB: team `JI-21 check 1791039213`, users `ji21-requester` and `ji21-approver-1791039213`.
- Follow-up (not done): attach `ReasonQualityHint` to the new reason field.
