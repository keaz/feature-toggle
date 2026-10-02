# AI-41: "Ask FluxGate" natural-language search in the command palette

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Done in UI 18c9bdf |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | AI-40, AI-02 |
| Behavior change | New palette item and results group. Existing palette behavior is unchanged. |
| Design | [design.md §5.4](../design.md#54-natural-language-search-ai-40-ai-41) |

## Goal

Let users type a question such as "stale payment flags in prod" in the command palette, select "Ask FluxGate", and get matching flags.

## Current code (verify first, paths relative to `feature-toggle-ui/src`)

| What | Where |
|---|---|
| Palette (cmdk `^1.1.1` in a shadcn `Dialog`) | `components/CommandPalette.tsx`. Groups: "Jump to", "Actions", "Features". Features come from `useTeamFeatures(teamId, {enabled: open, loadAll: true})`, each item's `value` is `feature ${key} ${description}`, and selecting navigates to `/features/{id}`. `Command.Input` is uncontrolled. |
| Hotkey | `hooks/useCommandPaletteHotkey.ts` |
| Existing test | `components/__tests__/CommandPalette.test.tsx` |

## Changes

1. **API**: in `api/ai.ts`, add `nlSearchFeatures(teamId, {query, limit})` returning `{available, filtersApplied?, results?: {feature: Feature; relevance: number | null}[]}`.
2. **Palette**:
   - Make the input controlled (`value`/`onValueChange`) and keep cmdk filtering for the existing groups.
   - When `useAiFeatures(teamId).nlSearch` is true and the trimmed query has at least 3 words, show a first item "Ask FluxGate: “<query>”" (lucide `Sparkles` icon).
     - The item must stay visible whatever cmdk's fuzzy filter does. Use cmdk's `forceMount` on the item and group if your cmdk version supports it, or a custom `filter` prop that always scores this item 1. Check the installed cmdk API.
   - Selecting it calls `nlSearchFeatures` once. It never runs per keystroke.
     - While loading, show a "Searching…" row.
     - On a result, show an "Ask results" group (also always visible) with one chip row for `filtersApplied` (for example `stale`, `tag: payments`), then the results, each with key, description snippet, and relevance as a percentage when it is not null. Selecting a result navigates to `/features/{id}`.
     - `available: false` or an error shows "AI search unavailable", and the normal groups still work.
   - Editing the query after a search clears the Ask results.
   - Add an "Open in list" item that navigates to `/features` with `filtersApplied` encoded as the list's URL params. This reuses the existing URL filter sync in `FeatureTable`.

## Tests

- The Ask item is hidden when AI is off and for queries under 3 words; it is shown otherwise.
- Selecting the Ask item calls the API once with the query; results render; selecting a result navigates.
- Typing after results clears them.
- `available: false` shows the unavailable text.
- The existing `CommandPalette.test.tsx` cases still pass.

## Acceptance criteria

- [ ] Lint, build, and tests pass, including the design-token guard.
- [ ] With AI off, the palette is identical to before.
- [ ] With AI on, one search makes exactly one HTTP request from the browser.

## Out of scope

Backend. Search history or saved queries.

## Handoff log

### 2026-10-02 (UI `18c9bdf`)

- `api/ai.ts`: `nlSearchFeatures(teamId, {query, limit})` plus `NlSearchResult`, `NlSearchHit`, `NlSearchFiltersApplied` types. Pure helpers in `lib/nlSearch.ts` (`countWords`, `filterChips`, `filtersToListParams`, `formatRelevance`).
- Palette: input is controlled (`value` / `onValueChange`, reset when the palette closes). The Ask item shows when `useAiFeatures(teamId).nlSearch` is on and the trimmed query has 3 or more words. cmdk filter stays on for the existing groups; a custom `filter` returns 1 for any value starting with `ask-fluxgate`, otherwise cmdk's exported `defaultFilter`. `forceMount` was not used because cmdk skips registering force-mounted items, which breaks the empty-state count.
- One search is one request: selecting the item (click or Enter) calls the API once with `limit: 10`; the item is replaced by a "Searching…" row while loading, so it cannot fire twice. A sequence ref drops late responses after the query is edited. Editing the query clears the Ask state. `available: false` or an error shows "AI search unavailable". The "No results." row is suppressed while an Ask state is showing.
- Results group "Ask results": chip row, then "Open in list" (only when at least one filter was applied), then hits (key, description snippet, relevance as a percent when not null). Selecting a hit goes to `/features/{id}`.
- "Open in list" goes to `/features?...` using only the params `FeatureTable` reads: `lifecycleStage`, `featureType`, `dependencyStatus`, `approvalStatus`, `flagKind`, `tag`, `owner`, and `stale` / `expired` as `true` / `false`. Backend values (`ARCHIVED`, `CONTEXTUAL`, lowercase `flagKind`) already match what the table expects. The table derives `includeArchived` from `lifecycleStage=ARCHIVED` itself.
- Tests: 12 new cases in `components/__tests__/CommandPalette.test.tsx` (hidden when off / under 3 words, shown first, no call while typing, one call on click and on Enter, results and chips, navigation, Searching, clear on edit, unavailable on `available: false` and on error, Open in list params, Open in list hidden with no filters). Full suite 653 passed, lint and build clean.
