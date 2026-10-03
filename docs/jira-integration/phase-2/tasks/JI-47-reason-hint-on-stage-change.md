# JI-47: `ReasonQualityHint` on the stage change reason field (backend + UI)

| Field | Value |
|---|---|
| Type | Feature (follow-up from JI-21) |
| Status | Open |
| Repo | backend (`feature-toggle/`) **then** UI (`../feature-toggle-ui/`): two commits |
| Depends on | — |
| Behavior change | Additive. New `reasonKind` value `stage_change` for the justification check; advisory hint under the reason field. Never blocks submit. |
| Design | [design.md §3.6](../design.md#36-ui-ji-46-ji-47) |

## Goal

When a user types a reason for a stage change, show the same AI "this reason is vague" hint that the emergency, freeze and cleanup reason fields already show.

## Current code (verify first)

| Piece | Where |
|---|---|
| Backend kinds | `feature-toggle-backend/src/judgment/justification.rs`: `enum ReasonKind` (~44), `as_str` (~54), description (~69), list of all kinds (~474) |
| Backend endpoint | `rest/ai.rs` (`reason_kind` ~187; `unknown_reason_kind_is_rejected` test ~1149) |
| UI kinds | `feature-toggle-ui/src/api/ai.ts` `ReasonKind` (~46) |
| UI hint | `src/components/ai/ReasonQualityHint.tsx` (props `teamId`, `reasonKind`, `reason`, `featureKey?`, `id?`) |
| Usage example | `src/components/modals/FeatureEmergencyActionModal.tsx` ~260; `src/pages/FeatureCreate.tsx` cleanup reason ~1800-1818 (`aria-describedby` + hint `id`) |
| Field to change | `src/pages/FeatureCreate.tsx` `stage-change-reason` textarea (~2086-2097), state `stageChangeReason` |
| Hint tests | `src/components/ai/__tests__/ReasonQualityHint.test.tsx` |

## Changes

**Backend commit**
1. Add `ReasonKind::StageChange`, serialized as `stage_change`. `as_str` returns `"stage_change"`. Description: `"Request a stage change (deploy, approve or roll back a feature flag in an environment)"`. Add it to the list of all kinds.
2. Fix the exhaustive matches the compiler reports.
3. Tests, first:
   - `stage_change_kind_round_trips` in `justification.rs`.
   - A REST case in `rest/ai.rs` showing `"reasonKind": "stage_change"` is accepted.
4. Run `cargo fmt`, `cargo clippy --all-targets` and the backend tests. Export the contracts, copy them to the baseline, and run the check (the enum is in the OpenAPI schema).
5. Commit `feat(ai): stage_change reason kind for the justification check (JI-47)`, noting the contract baseline update.

**UI commit**
1. Add `'stage_change'` to `ReasonKind` in `api/ai.ts`.
2. In `FeatureCreate.tsx`, under the `stage-change-reason` textarea:
   ```tsx
   <ReasonQualityHint
     teamId={<the team id this page already uses for the cleanup hint>}
     reasonKind="stage_change"
     reason={stageChangeReason}
     featureKey={<feature key in scope, as the cleanup hint passes it>}
     id="stage-change-reason-hint"
   />
   ```
   Add `aria-describedby="stage-change-reason-hint"` to the textarea. Use the same props source as the cleanup reason hint at ~1816. Do not invent a new one.
3. Test first, in `pages/__tests__/FeatureCreate.test.tsx`: `stage change reason shows the reason hint`. Mock the AI features as on, mock `checkJustification` to return a weak verdict, type a short reason, and expect the hint text. Copy the existing cleanup-hint test setup if there is one (`grep -n ReasonQualityHint src/pages/__tests__`).
4. Run `pnpm lint`, `pnpm build` and `pnpm test:run`, then `graphify update .`.
5. Commit `feat(jira): reason quality hint on stage change reason (JI-47)`.

## Done when

- The hint shows under the stage change reason when the AI check calls the reason weak.
- Submitting is never blocked, and nothing renders when the AI features are off.
- Both repos pass their checks. Handoff log, `../HANDOFF.md` and the README table updated.

## Handoff log

(empty)
