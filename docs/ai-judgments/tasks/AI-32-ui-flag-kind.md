# AI-32: UI flag kind field, suggestion chips, filter, detail

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Done in UI `9aa9005` |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | AI-30, AI-02 |
| Behavior change | New UI only |
| Design | [design.md §5.3](../design.md#53-flag-kind-ai-30-ai-31-ai-32) |

## Goal

Let users see and set a flag's kind, accept AI suggestions for kind and tags in the create/edit form, and filter the list by kind.

## Current code (verify first, paths relative to `feature-toggle-ui/src`)

| What | Where |
|---|---|
| Feature types | `api/features.ts`: `Feature`, `CreateFeatureInput`, `UpdateFeatureInput`, `FeatureFilters`, `fetchFeatures` (builds the query string) |
| Create and edit form (one component, 2300+ lines) | `pages/FeatureCreate.tsx`: `FeatureCreate({isEdit})`; field state block (key, description, type, lifecycleStage, owner, purpose, referenceUrl, expiresAt, cleanupReason, tagsText); "Basic Information" card; type `Select`; tag preview chips; submit `handleCreate` |
| Feature detail | `pages/FeatureDetail.tsx`: header badges row; Overview tab "Lifecycle" `SectionCard` (purpose, tags, cleanupReason, staleReasons) |
| List filters | `components/features/FeatureTable.tsx`: filter state seeded from the URL, `writeFiltersToUrl`, filter config rendered by `DataTable` (`components/tables/DataTable.tsx`, filter types text/select/boolean) |

## Changes

1. **Types and API**:
   - `Feature` gains `flagKind?: FlagKind | null`, `flagKindSource?: 'ai' | 'user' | null`, `flagKindConfidence?: number | null`.
   - The create and update inputs gain `flagKind?: FlagKind | null`.
   - `FeatureFilters.flagKind?: FlagKind | 'unclassified'`, passed by `fetchFeatures` as `flagKind`.
   - In `api/ai.ts`, add `getFeatureSuggestions(teamId, {key, description, purpose, tags})` and `backfillFlagKinds(teamId)`.
2. **Form** (`FeatureCreate.tsx`):
   - Add a "Kind" `Select` next to the type select, with options Release, Experiment, Ops / kill switch, Permission, Config, and "Not set".
   - The field shows the stored value. When the source is `ai`, show a small muted label "AI suggested".
   - Send `flagKind` on save **only when the user changed the field** (track a `flagKindTouched` flag). This keeps AI-sourced values from becoming user-sourced on every save.
   - Suggestions, only when `useAiFeatures(teamId).flagKind`:
     - After description or purpose blur, or 1 s idle, with a key present, call `getFeatureSuggestions`.
     - When the kind field is empty, untouched, and the suggestion is non-null, show a chip under the select: "Suggested: Ops (82%) · Use". Clicking sets the kind and marks it touched.
     - After the tag preview chips, show up to 5 "＋tag" chips; clicking appends the tag to `tagsText`.
     - Ignore stale responses, and render nothing on `available: false`.
3. **Detail** (`FeatureDetail.tsx`): in the Lifecycle card, show "Kind: Ops", with "(AI, 82%)" or "(set by user)". When AI-31 is merged and the kind is ops, permission, or config, show the muted note "Inactivity stale checks skipped for this kind".
4. **List** (`FeatureTable.tsx`):
   - Add a select filter "Kind" (the kinds plus Unclassified), synced to the URL like the other filters.
   - Optionally show a kind badge next to the lifecycle status.
5. **Backfill button**: on the AI settings page (from AI-02), add an admin-only "Classify existing flags" button that calls `backfillFlagKinds` and toasts `Queued N flags`. Show it only when `flagKind` is on.

## Tests

- Form:
  - The kind select appears.
  - An untouched AI value is not sent on save; a changed value is sent.
  - The suggestion chip appears only for an empty, untouched kind, and clicking applies it.
  - Tag chips append to the tags.
  - Nothing renders when AI is off.
- Detail: renders the kind with its source text.
- Table: the kind filter writes to the URL and calls `fetchFeatures` with `flagKind`.

## Acceptance criteria

- [ ] Lint, build, and tests pass, including the design-token guard. Note `FeatureDetail.tsx` is in the guard's `PENDING` exemption set; do not add raw colors anyway.
- [ ] With AI off, the form, detail page, and list behave as before, apart from the new optional Kind select and filter (these work without AI).
- [ ] Editing and saving a feature with an AI-set kind keeps `flagKindSource = ai`.

## Out of scope

Backend. Bulk "set kind" actions.

## Handoff log

### 2026-10-02, Claude (AI-32 implementation)

**What changed** (UI commit 9aa9005):

- `api/features.ts`: `FlagKind`, `FlagKindSource`; `Feature` gains `flagKind`, `flagKindSource`, `flagKindConfidence`; create and update inputs gain `flagKind`; `FeatureFilters.flagKind` (`FlagKind | 'unclassified'`) is passed by `fetchFeatures`. `api/ai.ts`: `getFeatureSuggestions`, `backfillFlagKinds`.
- `lib/flagKind.ts`: labels (short for badges, "Ops / kill switch" for select options) and `isStaleExemptKind` (ops, permission, config).
- `hooks/useFeatureSuggestions.ts`: debounced (1 s idle on description or purpose change, immediate on blur) suggestions call. Needs a non-empty key and the team's `flagKind` toggle. Skips an identical repeat request, ignores stale responses, renders nothing on `available: false` or an error.
- `components/features/FlagKindField.tsx`: Kind select (5 kinds plus "Not set"), muted "AI suggested" label for an untouched AI value, "Suggested: Ops (82%) · Use" chip for an empty untouched field, and `SuggestedTagChips` (up to 5 "＋tag" chips, hides tags already entered).
- `FeatureCreate.tsx`: Kind field next to the type select, tag chips under the tag preview. `flagKind` is sent on create and update only when `flagKindTouched`; "Not set" after a touch sends `null`. Touch state resets when the saved feature is reloaded.
- `FeatureTable.tsx`: "Kind" select filter (kinds plus Unclassified) seeded from and written to the `flagKind` URL param, included in saved views (old saved views without it load as unset); kind badge next to the lifecycle badge.
- `FeatureDetail.tsx`: Lifecycle card shows "Kind: Ops (AI, 82%)" or "(set by user)" when a kind exists, and the muted note "Inactivity stale checks skipped for this kind" for ops, permission and config (ahead of AI-31, per ruling).
- `AiSettingsPage.tsx`: "Classify existing flags" button under the flag kind toggle, shown when the server has AI and the saved `flagKind` toggle is on (not for an unsaved toggle); toasts `Queued N flags` (`Queued 1 flag` for one).

**Decisions and behavior to know:**

- Suggestions trigger from user edits only, not when an existing feature loads in edit mode.
- The kind chip shows only for an empty, untouched kind (a stored NULL with source `user` still counts as empty).
- With no kind set the detail page shows no Kind row, so a page without kinds looks as before.
- The note text appears before AI-31 lands; until then it is slightly ahead of the backend behavior.
- The backend sends `flagKindConfidence` only for the `ai` source; the detail page omits the percentage when it is missing.

**Verified:** `pnpm lint`, `pnpm build`, `pnpm test:run` (80 files, 616 tests, includes the design-token guard). New tests: `api/ai.test.ts`, `hooks/useFeatureSuggestions.test.tsx`, `FlagKindField.test.tsx`, `FeatureDetail.test.tsx`, plus new cases in `FeatureCreate.test.tsx` (the Select mock now forwards `onValueChange`), `FeatureTable.test.tsx` (memory router, because the test setup stubs `window.history`), `AiSettingsPage.test.tsx`. The unrelated 2-line local change in `FeatureDetail.tsx` was not committed.


- 2026-10-02 (final-review fix): the feature key input now calls `suggestions.refreshNow` on blur (commit 2396a9b); the suggested-kind Use button has `aria-label` "Use suggested kind <label>" (commit f094d1c).
