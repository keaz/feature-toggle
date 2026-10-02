# AI-21: UI `ReasonQualityHint` on reason inputs

| Field | Value |
|---|---|
| Type | Feature (UI) |
| Status | Done in UI 1672526 |
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

### 2026-10-02, Claude (AI-21 implementation)

**What changed** (UI repo, commit 1672526):

- `api/ai.ts`: `ReasonKind`, `JustificationCheckInput`, `JustificationCheckResult`, `checkJustification(teamId, input)` (POST `/teams/{id}/ai/justification-check`, camelCase body and response).
- `components/ai/ReasonQualityHint.tsx` (props `teamId`, `reasonKind`, `reason`, `featureKey?`): renders nothing unless `useAiFeatures(teamId).justificationCheck`. Checks after 800 ms without typing, only for 5 to 1000 trimmed characters. Results are cached in a ref per `(reasonKind, trimmed reason)`. A response is shown only while its key equals the current reason's key, so stale responses and old warnings never show. Only `available && verdict === "weak" && source !== "rule"` renders: `AlertTriangle` plus hints joined with a space, `text-warning`, `role="status"`, `data-testid="reason-quality-hint"`. Errors, `available: false`, `ok` and rule passes render nothing. Never touches the submit button.
- Wired under: `FeatureEmergencyActionModal` (both textareas, kinds `emergency_disable` and `emergency_enable`, team and key from the feature), `FeatureCreate` (`#freeze-override-reason`, `#scheduled-reason`, `#feature-cleanup-reason` with `archive_cleanup` shown only when the stage is ARCHIVED or the reason is non-empty; key from the form, omitted when empty), `FreezeWindowsPage` (`freeze_window`, no feature key).

**Decisions and behavior to know:**

- The brief's acceptance example "typing `test`" has 4 characters, below the 5-character minimum the brief and the modal's own validation use, so no call is made for it. Any 5+ character placeholder (for example "asdfg", "testing") shows the warning.
- Blur does not trigger an immediate check; the 800 ms debounce is the only trigger (the brief allowed either). Errors are not cached, so retyping retries.
- `useAiFeatures` only reads the team setting on mount (known gap in HANDOFF), so toggling the setting applies on the next mount.
- No dedicated test for the `FeatureCreate` and `FreezeWindowsPage` wiring (the `FeatureCreate` test mounts a heavy page); the component and the modal are tested.

**Verified:** `pnpm lint`, `pnpm build`, `pnpm test:run`: 76 files, 572 tests pass (includes the design-token guard). New: 11 tests in `components/ai/__tests__/ReasonQualityHint.test.tsx`, 3 in the modal test (weak hint still submits for disable and enable, no call when off).
