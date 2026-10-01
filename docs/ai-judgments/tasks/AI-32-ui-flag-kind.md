# AI-32: UI flag kind field, suggestion chips, filter, detail

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Not started |
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

_No entries yet._
