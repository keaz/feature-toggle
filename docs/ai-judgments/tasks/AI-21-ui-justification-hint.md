# AI-21: UI `ReasonQualityHint` on reason inputs

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Not started |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | AI-20, AI-02 |
| Behavior change | Warning text only. Submit is never blocked. |
| Design | [design.md §5.2](../design.md#52-justification-check-ai-20-ai-21) |

## Goal

Show an inline warning under free-text reason fields when the backend says the reason is vague.

## Current code (verify first, paths relative to `feature-toggle-ui/src`)

| Form | Where |
|---|---|
| Emergency disable and enable reason | `components/modals/FeatureEmergencyActionModal.tsx`: `reason` state, textareas `#emergency-reason` (disable and enable variants), `handleConfirm` (5-character minimum). It already shows `ApprovalPolicyPreviewBanner`. |
| Freeze override reason | `pages/FeatureCreate.tsx`: textarea `#freeze-override-reason`, shown only when `activeFreeze` is set |
| Scheduled change reason | `pages/FeatureCreate.tsx`: input `#scheduled-reason`, submit `handleScheduleCreate` |
| Cleanup reason | `pages/FeatureCreate.tsx`: input `#feature-cleanup-reason` (`cleanupReason` state) |
| Freeze window reason | `pages/FreezeWindowsPage.tsx`: reason input, submit `handleCreate` |

## Changes

1. **API**: in `api/ai.ts`, add `checkJustification(teamId, {reasonKind, reason, featureKey?})` returning `{available, verdict?, probability?, hints?, source?}`.
2. **`components/ai/ReasonQualityHint.tsx`**
   - Props: `{ teamId: string; reasonKind: ReasonKind; reason: string; featureKey?: string }`.
   - Renders nothing unless `useAiFeatures(teamId).justificationCheck` is true.
   - Checks when the user stops typing for 800 ms, or on `onBlur` (the parent forwards the input's blur, or the component accepts a `checkNow` key). Requires at least 5 trimmed characters.
   - Cancels or ignores stale responses: compare the request's reason to the current reason.
   - Caches results per `(reasonKind, reason)` for the component's life.
   - `weak`: a warning row (token classes such as `text-warning`, icon `AlertTriangle` from lucide) with the hints joined. A `source: "rule"` or `ok` result renders nothing. `available: false` renders nothing.
   - Never disables a submit button and never throws.
3. **Wire it in** directly under each of the 5 inputs, with the matching `reasonKind`:
   - `emergency_disable`, `emergency_enable`
   - `freeze_override`
   - `scheduled_change`
   - `archive_cleanup` (render only when the lifecycle is ARCHIVED or the cleanup reason is non-empty)
   - `freeze_window`

   Pass `featureKey` where the form knows it.

## Tests

- `ReasonQualityHint`:
  - Hidden when the feature is off.
  - Shows the warning for `weak`.
  - Hidden for `ok` and for `available: false`.
  - Debounces: several quick changes produce one call (fake timers).
  - Ignores a stale response.
- `FeatureEmergencyActionModal`: with a weak verdict mocked, the confirm button still submits.

## Acceptance criteria

- [ ] Lint, build, and tests pass, including the design-token guard.
- [ ] With the AI feature off, the five forms look and behave as before, with no extra network calls.
- [ ] With it on, typing "test" shows a warning, and "Checkout errors, see JIRA-123" shows nothing.

## Out of scope

Backend. Displaying stored verdicts in the activity UI.

## Handoff log

_No entries yet._
