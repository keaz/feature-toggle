# AI-20: Justification check (sync endpoint and post-submit recording)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Not started |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-01 |
| Behavior change | Additive. One new endpoint. Activity metadata gains `ai_justification` and, for archive, `cleanup_reason`. No request is ever blocked or slowed. |
| Design | [design.md §5.2](../design.md#52-justification-check-ai-20-ai-21) |

## Goal

Tell users when a free-text reason is vague, before they submit (sync endpoint) and in the audit trail after they submit (async judgment). Warn only.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Reason kind | Where it is accepted and logged today |
|---|---|
| `emergency_disable` | `rest/feature.rs`: `emergency_disable_feature` (DTO `EmergencyDisableRequest` in `rest/feature/types.rs`, validation `validate_emergency_reason`), then `logic/feature_tx.rs`: `emergency_disable_feature_in_tx`, activity `KILL_SWITCH_ACTIVATED` with `reason` |
| `emergency_enable` | `rest/feature.rs`: `emergency_enable_feature`, then `emergency_enable_feature_in_tx`, activity `KILL_SWITCH_DEACTIVATED` |
| `freeze_override` | `rest/operational_safety.rs`: `enforce_freeze_for_feature_environment`, `enforce_freeze_for_stage`, then `log_freeze_attempt` (activity `freeze_override`, metadata `override_reason`) |
| `scheduled_change` | `rest/operational_safety.rs`: `create_scheduled_change` (activity `scheduled_change_created`, metadata `reason`) and the reschedule handler (DTO with `reason`). Check whether reschedule writes an activity row. |
| `archive_cleanup` | `logic/feature_tx.rs`: `update_feature_in_tx`, activity `FEATURE_LIFECYCLE_UPDATED`. Its metadata has no `cleanup_reason` today. |
| `freeze_window` | `rest/operational_safety.rs`: `create_freeze_window`, `update_freeze_window`. No activity row. |
| Activity repository | `database/activity_log.rs`: `create_activity`, `create_activity_tx` (both return `ActivityLogRow`); `utils/activity_logger.rs` helpers |

## Changes

1. **Handler** `src/judgment/justification.rs`:
   - `enum ReasonKind` with the 6 values and serde `snake_case`. `fn action_description(kind) -> &'static str` returns fixed texts such as `emergency_disable`: "Turn off a feature flag in an emergency (kill switch)". Write one for each kind.
   - `fn rule_check(reason) -> bool`: true when the reason matches a ticket key `\b[A-Z][A-Z0-9]+-\d+\b`, a URL `https?://\S+`, `#\d+`, or `\bINC\d+\b`. `regex` is already used by the evaluation engine; add it to the backend crate if it is missing.
   - `fn build(input)` returns the 2 Noul questions with the exact text in design §5.2.
   - `fn derive(answers) -> {verdict, probability, hints, source: "model"}` with named-constant thresholds.
   - `JustificationHandler: JudgmentHandler`:
     - `build` and `derive` as above.
     - `apply`: for subject type `activity`, call the new `merge_activity_metadata(activity_id, "ai_justification", {verdict, probability, model})`. Other subject types do nothing.
   - Add `ActivityLogRepository::merge_activity_metadata(id, key, value)` using `UPDATE activity_log SET metadata = jsonb_set(COALESCE(metadata, '{}'::jsonb), ARRAY[$2], $3, true) WHERE id = $1`. Check the real table name.
2. **Sync endpoint** `POST /api/v1/teams/{team_id}/ai/justification-check`, in `rest/ai.rs`:
   - Body `{reasonKind, reason, featureKey?}`. The reason must be 1 to 1000 characters; otherwise return 400.
   - When the subsystem is off or the team setting `justification_check` is off, return `{available: false}`.
   - When `rule_check` passes, return `{available: true, verdict: "ok", probability: 1.0, hints: [], source: "rule"}` without calling the API.
   - Otherwise call `JudgmentClient` directly and return the derived result. Any client error returns `{available: false}`.
3. **Post-submit recording**: for each path in the table, after the main transaction commits:
   - If the service exists and the team setting is on, call `submit(team, Justification, subject_type, subject_id, {reason_kind, reason, feature_key})`.
   - Never `await` the API call in the request path; `submit` only upserts and spawns.
   - Subject: the activity id from `create_activity(_tx)` where an activity is written. Thread the returned row id out of the logger helpers if they discard it.
   - `freeze_window`: (`freeze_window`, id). A reschedule without an activity row: (`scheduled_change`, id).
   - `archive_cleanup`: in `update_feature_in_tx`, when the new lifecycle stage is archived and `cleanup_reason` is non-empty, add `"cleanup_reason"` to the `FEATURE_LIFECYCLE_UPDATED` metadata. Then use that activity row as the subject.
   - Rule-check passes are also recorded. Upsert the row and mark it `done` with `derived = {verdict: "ok", source: "rule"}` without an API call. Add a `JudgmentService::record_rule_result(...)` helper for this, which also merges the metadata.
4. Register the endpoint and schemas in `ApiDoc` and update the contract baseline.

## Tests

- `rule_check`: positives (`JIRA-123`, `https://x.y`, `#42`, `INC12345`) and negatives (`urgent`, `fix prod`).
- `derive`: placeholder high gives weak with the placeholder hint; concrete low gives weak with the cause hint; both fine gives ok.
- Endpoint: off gives `available: false`; rule pass makes no client call (`MockJudgmentClient` expects 0 calls); model path maps answers; client error gives `available: false`; a 1001-character reason gives 400.
- Recording: for emergency disable with the setting on, `submit` is called once with subject `activity` and the activity id. With the setting off, it is not called. The emergency handler's response is unchanged when `submit` fails.
- Archive: the activity metadata contains `cleanup_reason`.
- Live tuning (`#[ignore]`): `tests/fixtures/ai/justification.json` with at least 20 labelled reasons, for example "test", "asdf", "urgent", "per Bob", "Disabling: checkout 500s since 14:00, see incident channel", "Experiment finished, variant B shipped in release 4.2".

## Acceptance criteria

- [ ] All six paths record a judgment when the setting is on, and none do when it is off.
- [ ] Emergency endpoint latency is unchanged; there is no API call in the request path.
- [ ] The sync endpoint answers with `available: false`, never 5xx, on any TypeSafe failure.
- [ ] Contract baseline is updated; all backend tests pass.

## Out of scope

UI hint component (AI-21). Showing verdicts in the activity log UI (later).

## Handoff log

_No entries yet._
