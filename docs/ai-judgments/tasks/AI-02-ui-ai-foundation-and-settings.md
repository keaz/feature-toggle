# AI-02: UI foundation: `api/ai.ts`, `useAiFeatures`, AI settings page

| Field | Value |
|---|---|
| Type | Feature (UI foundation) |
| Status | Done in 80bb277 (feature-toggle-ui, commits 02e7fd3, 80bb277) |
| Repo | UI (`../feature-toggle-ui/`, separate git repo) |
| Depends on | AI-01 merged and deployed to the dev backend |
| Behavior change | New settings page. Nothing else changes. |
| Design | [design.md §6, §8](../design.md#6-rest-surface-summary) |

## Goal

Give every AI UI task one hook that answers "is this AI feature on for this team?". Add a page where admins turn each feature on or off and see what data is sent to TypeSafe.

## Current code (verify first, paths relative to `feature-toggle-ui/src`)

| What | Where |
|---|---|
| Fetch wrapper: `apiFetch`, `apiJson<T>`, `ApiError` | `api/client.ts` |
| Domain API module pattern | `api/approvals.ts`, `api/teams.ts` |
| Shared query cache: `useSharedQuery({key, queryFn, enabled, pollInterval, staleTime})`, `invalidateSharedQuery(key)` | `hooks/useSharedQuery.ts` |
| Selected team | `contexts/TeamContext.tsx` (`selectedTeam`) |
| Admin-gated settings page example | `pages/NotificationSettingsPage.tsx` |
| Nav groups (Settings / Govern) | `layout/navConfig.ts` |
| Routes | `routes/AppShellRoutes.tsx`, `routes/routeModules.ts` |
| shadcn primitives (card, switch or checkbox, button, badge) | `components/ui/*` |
| Note: CLAUDE.md mentions Apollo, but the app is REST only | `package.json` |

## Changes

1. **`api/ai.ts`**, with types hand-written like the other modules:
   - `getAiStatus(): Promise<{available: boolean; model: string | null}>` for `GET /ai/status`.
   - `getTeamAiSettings(teamId)` and `updateTeamAiSettings(teamId, input)` for `GET`/`PUT /teams/{teamId}/ai-settings`. Fields: `available`, `approvalRisk`, `justificationCheck`, `flagKind`, `nlSearch`, `updatedAt`.
   - Leave stubs or type-only declarations for later endpoints to the tasks that own them. Do not add them here.
2. **`hooks/useAiFeatures.ts`**
   - `useAiFeatures(teamId?: string)` returns `{ loading, available, approvalRisk, justificationCheck, flagKind, nlSearch }`.
   - It uses `useSharedQuery` with key `ai-settings:<teamId>` and `staleTime` 60 s.
   - On any error it returns all false, so AI UI hides.
   - Export `invalidateAiFeatures(teamId)`.
3. **`pages/AiSettingsPage.tsx`**
   - A card with four switches and one-line descriptions:
     - Approval risk triage
     - Reason quality check
     - Flag kind classification
     - Natural-language search
   - Admin-gated the same way as `NotificationSettingsPage`.
   - When `available` is false, show an info state: "AI judgments are not configured on this server (TYPESAFE_API_KEY not set)". Disable the switches.
   - Data notice, always visible: "When on, FluxGate sends flag keys, descriptions, purposes, tags, change diffs, and free-text reasons to TypeSafe (api.typesafe.ai) for evaluation. Natural-language search also sends the team's tag and owner names. TypeSafe does not train on this data."
   - Save calls `updateTeamAiSettings`, shows a `sonner` toast, and calls `invalidateAiFeatures`.
4. **Route and nav:** add `/settings/ai` (label "AI assistance") to the Settings or Govern group in `navConfig.ts`, the route in `AppShellRoutes.tsx`, and the lazy module in `routeModules.ts`. Follow how `NotificationSettingsPage` is registered.

## Tests (`__tests__/`, Vitest and Testing Library; mock `api/ai.ts`)

- `useAiFeatures`: success maps fields; an error returns all false.
- `AiSettingsPage`:
  - `available: false` disables the switches and shows the message.
  - Toggling and saving calls `updateTeamAiSettings` with the right body.
  - A non-admin sees the same gate as on the notification settings page.

## Acceptance criteria

- [x] `pnpm lint`, `pnpm build`, and `pnpm test:run` pass (this repo uses pnpm), including the design-token guard.
- [ ] With the backend running without a key, the page shows the not-configured state.
- [ ] With a key, the toggles persist across reload.

## Out of scope

Any feature-specific AI UI (AI-12, AI-21, AI-32, AI-41).

## Handoff log

_No entries yet._

### 2026-10-02, Claude (plan 2026-10-02-ai-judgments-foundation)

- Changed (UI repo): `api/ai.ts`, `hooks/useAiFeatures.ts` (a feature is on only when `available` and its toggle are both true; any error means all off), `pages/AiSettingsPage.tsx` (system-admin only, team from `TeamContext`, data notice, not-configured and load-error states), route `/settings/ai`, Settings nav item "AI assistance" (admin gate).
- Verified: `pnpm lint`, `pnpm build`, `pnpm test:run` (75 files, 554 tests). Backend end to end on a seeded test DB: without a key the server logs "disabled", `/ai/status` returns `{"available":false,"model":null}` and settings return `available: false`; with a key it logs "enabled (model jev-1.13.0)", a PUT persists and the next GET returns it, and an `ai_settings_updated` activity row is written. The key appears in no log line and no commit.
- Not verified: the page in a real browser (the two unticked boxes). The page states are covered by component tests and the API by the end-to-end calls above.
- Note: `useSharedQuery` does not handle the rejection of its background fetch (every consumer gets an unhandled rejection on a failed request). `useAiFeatures` maps a failed fetch to `null` to avoid it; the shared hook is unchanged.
- Next: AI-12, AI-21, AI-32, AI-41 call `useAiFeatures(selectedTeam?.id)` and hide their UI when the flag is false.
