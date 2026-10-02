# AI-20: Justification check (sync endpoint and post-submit recording)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in ddd3a6d |
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

- [x] All six paths record a judgment when the setting is on, and none do when it is off.
- [x] Emergency endpoint latency is unchanged; there is no API call in the request path.
- [x] The sync endpoint answers with `available: false`, never 5xx, on any TypeSafe failure.
- [x] Contract baseline is updated; all backend tests pass.

## Out of scope

UI hint component (AI-21). Showing verdicts in the activity log UI (later).

## Handoff log

### 2026-10-02, Claude (AI-20 implementation)

**What changed** (commit ddd3a6d):

- `judgment/justification.rs`: `ReasonKind` (6 values, snake_case), `action_description`, `rule_check` (ticket key, URL, `#123`, `INC123`), `build_input`, `build`, `derive` (named thresholds `PLACEHOLDER_WEAK_AT` 0.6 and `CONCRETE_WEAK_BELOW` 0.4; a missing `concrete_cause` counts as 0.5 so incomplete answers never warn), `JustificationHandler` (`apply` merges `ai_justification` into the activity metadata for `activity` subjects, nothing for other subjects), and `record_justification` (the one helper every call site uses: skips on no service, blank reason, or team toggle off; rule pass goes to `record_rule_result`, otherwise `submit`; errors are logged and dropped). `test_support::recording_runtime` is a test-only `AiRuntime` for handler tests.
- `JudgmentService::record_rule_result`: upserts the row, then in the background marks it `done` (`model = "rule"`, `derived = {verdict: "ok", source: "rule"}`) and runs the handler's `apply`, so the activity metadata gets `{verdict: "ok", probability: 1.0, model: "rule"}`. Like `submit`, it does not check the team toggle; callers do.
- `ActivityLogRepository::merge_activity_metadata(id, key, value)` (`jsonb_set`, keeps other keys, no error for a missing row).
- `POST /api/v1/teams/{team_id}/ai/justification-check` in `rest/ai.rs` (`JustificationCheckRequest`/`Response`, `ReasonKind` in `ApiDoc`; contract baseline updated). Reason is trimmed and must be 1 to 1000 characters, else 400. Unavailable (no key, toggle off, settings read error, client error) gives `{"available": false}`.
- Logic: `emergency_disable_feature_in_tx` and `emergency_enable_feature_in_tx` now return `(Feature, activity_id)`. `update_feature_in_tx` returns `FeatureUpdateOutcome { feature, activity_id, cleanup_reason }`. It adds `cleanup_reason` to the `feature_lifecycle_updated` metadata only when the update moves the feature to `archived` and the stored reason is non-blank; `outcome.cleanup_reason` is set only then. `enforce_freeze_for_feature_environment` and `enforce_freeze_for_stage` return `Option<FreezeOverride>` (activity id, team, key, reason) when an override was used.
- Recording call sites (all after the write, `Option<web::Data<AiRuntime>>` extractor so apps without an AI runtime still work): emergency disable and enable, `update_feature` (archive cleanup, and each freeze override), `request_stage_change` (freeze override), `create_scheduled_change`, `reschedule_scheduled_change`, `create_freeze_window`, `update_freeze_window`.
- `lib.rs::run` registers `JustificationHandler` in the same `with_handler` chain. `regex = "1.10"` added to the backend crate.

**Decisions and behavior to know:**

- Subjects: activity row where one exists (emergency, freeze override, scheduled change create, archive). Reschedule uses (`scheduled_change`, id). Freeze windows use (`freeze_window`, id). Verdicts for the last two are only in `ai_judgments`, because they have no activity row to merge into.
- Reschedule and freeze window update record only a reason sent in that request, not a stored one. A freeze window create records `reason` when present. Reschedule has no feature key, so the input has `feature_key: null`.
- Freeze overrides are recorded right after `enforce_freeze_*` returns (the override activity row is already committed then), not after the later feature transaction. If that transaction fails, the judgment still exists for the override row.
- Archive only records when the lifecycle stage actually changes to archived (that is the only case with a `feature_lifecycle_updated` row). Re-saving an archived feature records nothing.
- `submit`/`record_rule_result` are awaited in the request; each is one upsert. No API call is in the request path. The emergency response does not change when it fails (tested).
- The input stored in `ai_judgments` is `{reason_kind, reason, feature_key}`, reason trimmed and cut to 1000 characters. Rule check runs on the untruncated reason.
- Retry sweep: failed justification rows rerun through `JustificationHandler::build` from the stored input.

**Verified:**

- `cargo test -p feature-toggle-backend` on `feture_toggle_test`: 628 unit tests, 251 integration tests, 25 grpc tests and contract compatibility all pass (0 failures). New tests: `judgment::justification` (rule, derive, wire snapshot, recording through the service with mocks), `rest::ai::tests::justification_check` (off, rule pass makes 0 client calls, model path, client error, 1001 chars gives 400), `rest::feature` handler tests (emergency disable/enable/off/submit fails, archive with and without reason), `rest::operational_safety` tests (schedule, reschedule, freeze window create/update, freeze override), integration `justification_recording_test` (activity ids, `cleanup_reason` in metadata, `merge_activity_metadata`).
- Live tuning: `cargo test -p feature-toggle-backend --test justification_live_test -- --ignored --nocapture`. Fixture `tests/fixtures/ai/justification.json`: 34 labelled reasons (16 weak, 18 ok, 4 of them rule passes). Accuracy 34/34 = 1.00 with the design thresholds unchanged (min asserted 0.75). Weak placeholders score `placeholder_text` 0.4 to 0.99 and `concrete_cause` below 0.3; real reasons score `concrete_cause` 0.95 or more. Short non-placeholder weak reasons ("cleanup", "freeze", "not needed") are caught by `concrete_cause`, not `placeholder_text`.

**For later tasks:**

- AI-21 calls the new endpoint with `{reasonKind, reason, featureKey?}` and reads `{available, verdict, probability, hints, source}`; `available: false` means show nothing. Hints are fixed English strings.
- Activity rows now carry `metadata.ai_justification = {verdict, probability, model}` once the judgment is done (later, not at write time). A UI that shows it must not assume it exists.
- Not done (out of scope): UI hint (AI-21), showing verdicts in the activity log UI.
