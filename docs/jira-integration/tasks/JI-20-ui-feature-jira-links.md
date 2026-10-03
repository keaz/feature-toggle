# JI-20: UI, Jira links panel on the feature page

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
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

(empty)
