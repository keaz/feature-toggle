# JI-20: UI, Jira links panel on the feature page

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in UI `3366750` (backend `a95d30c`) |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | JI-10 |
| Behavior change | New card on the feature detail page. |
| Design | [design.md §3.5](../design.md#35-ui-ji-20-ji-21-ji-22) |

## Goal

Show the Jira issues linked to a feature, and let users who can edit the feature add and remove links.

## Current code (verify first, paths under `feature-toggle-ui/src`)

| Piece | Where |
|---|---|
| Feature detail page | `pages/FeatureDetail.tsx`: tabs `overview`, `stages`, `variants`, `dependencies`, `activity` (~476-482); `referenceUrl` link (~528-536); `tags` badges (~538-544) |
| API pattern | `api/client.ts` (`apiJson`, auth header, 401 refresh); one file per resource in `api/` (for example `api/approvals.ts`, `api/features.ts`); shared types `api/types.ts` |
| Data hooks | `useSharedQuery` (see `hooks/`), as used by `hooks/useAiFeatures.ts` |
| Edit permission | How `FeatureDetail.tsx` decides whether the user may edit (reuse it; do not invent a new rule) |
| Tests | Vitest + Testing Library; example `components/ai/__tests__/ReasonQualityHint.test.tsx`; `__tests__/designTokenGuard.test.ts` forbids raw palette classes and hex colors |

## Changes

1. `api/externalLinks.ts`: `ExternalLink` type (`id`, `featureId`, `system: 'jira'`, `externalKey`, `url: string | null`, `createdBy`, `createdAt`); `listExternalLinks(featureId)`, `createExternalLink(featureId, {system: 'jira', externalKey, url?})`, `deleteExternalLink(featureId, linkId)`.
2. `components/features/JiraLinksCard.tsx`: shown on the overview tab next to reference URL and tags.
   - List: key as a link (new tab, `rel="noopener noreferrer"`) when `url` is set, plain text otherwise.
   - Editors: "Add" opens an inline form (key, optional URL). Upper-case the key on input. Show the server's 400/409 message inline. Remove button with a confirm step in the UI (not `window.confirm`).
   - Empty state: "No Jira issues linked."
   - Loading and error states follow the page's existing pattern.
3. Wire the card into `FeatureDetail.tsx`.

## Tests (write first)

- `components/features/__tests__/JiraLinksCard.test.tsx`: renders links (with and without URL); add calls the API with an upper-cased key and refreshes the list; 409 shows the message; remove calls delete; non-editors see no add/remove controls.
- Design token guard passes.

## Done when

- `pnpm lint`, `pnpm build`, `pnpm test:run` pass in `feature-toggle-ui/`.
- Checked by hand against a running backend with JI-10 (add, duplicate, remove).
- Handoff log entry written, `HANDOFF.md` and README table updated (docs live in the backend repo).

## Handoff log

### 2026-10-03: done in UI `3366750`, backend `a95d30c`

**What changed (UI, `feature-toggle-ui/`)**

- `src/api/externalLinks.ts`: `ExternalLink`, `listExternalLinks(featureId) -> ExternalLink[]` (unwraps `items`), `createExternalLink(featureId, {system: 'jira', externalKey, url?})`, `deleteExternalLink(featureId, linkId)`.
- `src/components/features/JiraLinksCard.tsx`: `JiraLinksCard({featureId, canEdit})`. Data through `useSharedQuery` key `feature-external-links:<featureId>` (staleTime 15 s); refetched after add and remove. List: key as a link (new tab, `rel="noopener noreferrer"`, external-link icon) when `url` is set, plain text otherwise. Editors: "Add" (accessible name "Add Jira issue") opens an inline form (key upper-cased as typed, optional URL, URL left out of the request when blank); the server message is shown in a `role="alert"` under the form, which stays open. Remove: "Remove <KEY>" button → inline confirm group ("Confirm removing <KEY>") with Cancel / Remove. Empty: "No Jira issues linked."; error: "Jira links unavailable: <message>"; loading: skeleton.
- `src/pages/FeatureDetail.tsx`: the card sits on the overview grid right after the Lifecycle card (which holds reference URL and tags).

**Edit permission**

- `FeatureDetail.tsx` has no edit rule of its own (the Edit link is shown to everyone; the server enforces). The backend allows link writes for admins and Team Admins (`authorize_feature_update`), so the card uses the existing `canAccessTeamsManagement()` from `src/utils/auth.ts` (`is_admin` or the `Team Admin` role). It does not check the team of the role; the server still does.

**Backend change found during the manual check (`a95d30c`)**

- A duplicate link returned 409 with `external link jira PROJ-1` (the repository's `RecordAlreadyExists` text). `rest/external_link.rs::create_external_link` now maps it to `PROJ-1 is already linked to this feature`. `create_duplicate_returns_409` asserts the message. No contract change.

**Not committed**

- The UI working tree had an unrelated uncommitted change in `src/pages/FeatureDetail.tsx` (stage label "Rollout stage N" instead of "Position … · order …"). It was left in the working tree and is not part of `3366750`.

**Verified**

- UI: `pnpm lint` (no issues), `pnpm build` (ok), `pnpm test:run` (83 files, 674 tests, design token guard included). New tests: `components/features/__tests__/JiraLinksCard.test.tsx` (9: list with/without URL, empty, load error, add with upper-cased key and refresh, blank URL left out, 409 inline, remove after confirm, cancel remove, no controls for non-editors).
- Backend: `cargo test -p feature-toggle-backend` all pass; clippy no warnings in the changed file; contract hashes unchanged.
- Manual (backend on `127.0.0.1:8080` with the test DB, Vite on `localhost:8090`, Chrome DevTools, admin `api-test-admin`): empty state; add `proj-123` + URL → shown as `PROJ-123` link; add again → "PROJ-123 is already linked to this feature"; `not a key` → "externalKey must be a Jira issue key such as PROJ-123"; remove → confirm → empty state. The temporary feature was deleted afterwards.
