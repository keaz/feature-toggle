# AI judgments: handoff

Date: 2026-10-03 (final state after the final-review fix waves and the known-issue fixes). Read this after [`README.md`](README.md) and [`design.md`](design.md). All tasks AI-00 to AI-41 are done. This file records what is built, what changed from the design, the known gaps, and what anyone extending the feature must know.

## 1. Status

| Task | State | Where |
|---|---|---|
| AI-00 client, config, `/ai/status` | Done | backend `e9bf1e6..f7ddccb` |
| AI-01 judgment store, team settings, service, retry sweep | Done, plus pre-AI-10 fixes | backend `88728a1..db0dc7e`, fixes `5ec630e` |
| AI-02 UI settings page and `useAiFeatures` | Done | UI `02e7fd3`, `80bb277`, race fix `cb32e5b`, `useSharedQuery` fix `813342a` |
| AI-10 approval risk assessment | Done | backend `69ef888` |
| AI-11 risk enforcement | Done (signed off 2026-10-02; changed by user decisions 2026-10-03, see §5) | backend `8cde72c`, final fixes `40ebcdf` |
| AI-12 UI approval risk | Done | UI `fb3a756` |
| AI-20 justification check backend | Done | backend `ddd3a6d` |
| AI-21 UI justification hint | Done | UI `1672526` |
| AI-30 flag kind backend | Done | backend `f928d65`, review fixes `cf3e23f` (flaky test fix `5499270`) |
| AI-31 stale rules use flag kind | Done (signed off 2026-10-02) | backend `335b2f6` |
| AI-32 UI flag kind | Done | UI `9aa9005` |
| AI-40 NL search backend | Done | backend `86dedbb`, review fixes `e3c3611` |
| AI-41 UI NL search palette | Done | UI `18c9bdf`, review fixes `9da4987` |
| UI final-review fix wave | Done | UI `9da4987..dc16a2f` (6 commits), docs `e8c0b7a` |
| Backend final-review fix wave | Done | backend `8ab8761`, `b08ec4f`, `40ebcdf`, `166a573`, `5c54068`, `3a8faac`, docs commit after them |
| Backend known-issue fixes (2026-10-03) | Done | backend `ce36bfe` (retry attempts), `bbca40a` (approvers who can still vote), docs commit after them |

Backend final-review fix wave (2026-10-03):

- `8ab8761` Emergency disable/enable and archive record their reason **after** the edge broadcast; freeze-override reasons are recorded once the update or stage change has committed and been broadcast.
- `b08ec4f` A risk assessment still `pending` 10 minutes after it was queued is reported as `failed` (`AI_RISK_PENDING_MAX_AGE_MINUTES`, same cap as the UI poll).
- `40ebcdf` Extra approver capped at the eligible approver count; `require_extra_approver` also blocks auto-approval (user decisions 2026-10-03).
- `166a573` Sync AI calls no longer wait behind the background pipeline (bounded permit wait, separate background semaphore; see §2).
- `5c54068` The approval request event is published before the AI assessment is queued, then once more after it is queued.
- `3a8faac` Approval list and approvals stream read each policy once; a failed policy read no longer aborts the stream send.

Backend known-issue fixes (2026-10-03, after the final review):

- `ce36bfe` `attempts` counts only runs that reached the API; a run that waited for a permit (`Deferred`, client `Busy`) spends nothing. The sweep is bounded by a separate `claims` count (10) with 2-minute claim spacing. Migration `20261003010000_ai_judgment_claims.sql`. See §2.
- `bbca40a` The AI-11 override is capped at the approvals given plus the eligible approvers who can still vote (never below the policy), for votes and `requiredApprovalsEffective`. The auto-approval tick approves a pending request nobody is left to vote on once its approvals meet the policy, with an `approval_requirement_reconciled` activity. See §5 AI-11.
- Docs: the "role-routed request = empty `eligible_approver_ids`" claim was wrong and is corrected here, in design §5.1, in the AI-11 task, and in the `extra_approver_requirement` doc comment.

Repo practice for this project: commit directly on `main` in each repo (the README's branch-per-task rule is not used here). Stage files by explicit path; both repos have unrelated local changes.

## 2. What exists and how to plug a feature in

### Backend (`feature-toggle-backend/src/`)

| Piece | Where | Use it for |
|---|---|---|
| `TypesafeConfig` (`[typesafe]`) | `config.rs` | Model `jev-1.13.0` pinned, `timeout_ms` 3000, `max_in_flight` 16. Zero values fall back to defaults. `max_in_flight` is the global cap on concurrent API calls. |
| `JudgmentClient` trait, `MockJudgmentClient`, `HttpJudgmentClient` | `judgment/client.rs` | Sync calls: `client.evaluate(state, questions)`. Retries 429/529/connect/timeout (also body-read timeouts) at most twice. `JudgmentError::log_label()` is the only thing safe to log. |
| Wire types, question builders | `judgment/types.rs` | `Question::noul`, `noul_with`, `choice` (2–255 options), `score` (2–10 levels); `Answers::noul/choice/score`; `RequestParts`. |
| `JudgmentKind`, `SubjectType` | `judgment/mod.rs` | `JudgmentKind::feature()` gives the team toggle (`AiFeature`) for a kind. |
| `AiRuntime` | `judgment/mod.rs`, registered as `web::Data<AiRuntime>` | `runtime.client` (sync endpoints), `runtime.judgments` (async pipeline), `runtime.available()`. Both are `None` without `TYPESAFE_API_KEY`. |
| `JudgmentService`, `JudgmentHandler` | `judgment/service.rs` | Async judgments: `submit(team_id, kind, subject_type, subject_id, input)`; `team_enabled(team_id, AiFeature::...)`. |
| Repositories | `database/ai.rs` | `TeamAiSettingsRepository` (registered as `web::Data<Box<dyn TeamAiSettingsRepository>>`), `AiJudgmentRepository` (`get_for_subject`, `get_for_subjects` for list endpoints). Both have unconditional `#[automock]`, so integration tests can use the mocks. |
| Retry sweep | `scheduler/ai_judgment_retry.rs` | Runs every 60 s, only when a key is set. |
| REST | `rest/ai.rs` | `GET /ai/status`; `GET`/`PUT /teams/{team_id}/ai-settings` (PUT is system-admin only); `POST /teams/{team_id}/ai/justification-check` (AI-20); `POST /teams/{team_id}/ai/feature-suggestions` (AI-30); `POST /teams/{team_id}/ai/flag-kind/backfill` (AI-30); `POST /teams/{team_id}/features/nl-search` (AI-40). Approval DTOs in `rest/approval.rs` carry `aiRisk`, `requiredApprovalsEffective` and the policy `aiRiskMode` (AI-10/11); feature DTOs carry `flagKind`, `flagKindSource`, `flagKindConfidence` and the list takes a `flagKind` filter (AI-30). |

**Adding an async judgment (AI-10, AI-20, AI-30):**

1. Write `judgment/<kind>.rs` with a `JudgmentHandler`. Keep `build` and `derive` pure (snapshot tests on the request, table tests on `derive`). `apply` must re-check that its subject is still fresh, as design §3.4 says.
2. Register it in `lib.rs::run`: today the service is built as `Arc::new(JudgmentService::new(...))`. Change that to `JudgmentService::new(...).with_handler(Arc::new(YourHandler::new(...)))` and then wrap it in `Arc`. Handlers cannot be added after the `Arc` exists.
3. **Wiring order (done in AI-10).** The trigger is in `ApprovalLogicImpl::maybe_create_stage_change_request` (`logic/approval.rs`), but `lib.rs::run` builds `approval_logic` before `judgment_service`. AI-10 moved the `team_ai_settings_repository` / `judgment_service` block above the logic services and registers `ApprovalRiskHandler` there; add your handler to the same `with_handler` chain. Pass `Option<Arc<JudgmentService>>` into the logic constructor (see `approval_logic_with_notifications` for the pattern: tests pass `None`). The same applies to the feature handlers in AI-20 and AI-30 if they submit from logic code.
4. At the call site: skip when the service is `None`, check `team_enabled(team_id, kind.feature())`, then `submit(...)`. Never let a submit error fail or slow the user's request: log it and continue.

**Adding a sync endpoint (AI-20 pre-check, AI-30 suggestions, AI-40 NL search):** take `web::Data<AiRuntime>` and `web::Data<Box<dyn TeamAiSettingsRepository>>`. Return HTTP 200 with `{"available": false}` when `runtime.client` is `None`, the team toggle is off, or the call fails. Return 400 only for invalid input (design §6).

### Concurrency: sync calls versus the background pipeline (final-review fix `166a573`)

- `HttpJudgmentClient` holds `max_in_flight` permits for all callers. A caller waits for a permit at most `timeout_ms`; if none frees up, `evaluate` returns `JudgmentError::Busy` (log label `busy`) without sending anything. `JudgmentError::reached_api()` is false only for `Busy` and `Unavailable`. Sync endpoints then answer `{"available": false}` as for any other failure, instead of hanging.
- `JudgmentService` has its own semaphore with `async_in_flight(max_in_flight)` = half of `max_in_flight` (at least 1) permits, taken around the API call. Background runs (approval risk, justification recording, flag kind, backfill) can therefore hold at most half of the global permits; the other half always stays free for the sync endpoints. Wired in `lib.rs` with `.with_async_in_flight(...)`.
- A run started by `submit` waits at most `SUBMIT_QUEUE_WAIT` (60 s) for a background permit. If it cannot start, it returns `RunOutcome::Deferred` and leaves the row `pending` without spending an attempt; the retry sweep runs it after 2 minutes. Because 60 s is shorter than the sweep's 2-minute pending threshold, a row the sweep may claim is not also started by its submit task, so a 500-row backfill no longer calls the API twice for the same row. Sweep runs wait for a permit without a limit (they already claimed the row).
- With `max_in_flight = 1` there is no reserve: background and sync share the one permit (sync still waits at most `timeout_ms`).

### Retry and attempt semantics (changed from the original design text; design §3.4 is updated)

- `attempts` counts runs that reached the API (fix 2026-10-03). `submit` (and a new input) writes the row with `attempts = 0`. A run holding its background permit calls `start_attempt` right before `client.evaluate`: an atomic `UPDATE ... SET attempts = attempts + 1 WHERE id AND input_hash AND status <> 'done' AND attempts < 3`. When it changes no row (newer input, already done, or 3 attempts used), the run makes no API call and returns `RunOutcome::Skipped`. When `evaluate` fails with `Busy` or `Unavailable` (nothing sent), the run calls `refund_attempt` and marks the row failed. A crash between `start_attempt` and the end of the call keeps the attempt counted. So at most 3 runs of one input reach the API, also across nodes; runs that waited for a permit (`Deferred`, `Busy`) spend nothing.
- The sweep claims rows with `claim_retryable(50)`: pending rows older than 2 minutes and failed rows, only while `attempts < 3` and `claims < 10` (`MAX_CLAIMS`), and only 2 minutes after the row's last claim (`claimed_at`). Claiming increments `claims` and sets `claimed_at` (`FOR UPDATE SKIP LOCKED`); it does not touch `attempts`. `claims` is the bound that stops a row whose runs never get a permit from being claimed forever: with the 2-minute spacing it is retried for at least 20 minutes, then left alone. The spacing also keeps another node (or the next tick) from claiming a row whose run is still in progress. A new input resets `attempts`, `claims` and `claimed_at` (migration `20261003010000_ai_judgment_claims.sql`).
- `mark_failed` does not change `attempts` and never overwrites a `done` row.
- `mark_done` is guarded by `input_hash` **and** `status <> 'done'`. A second run of the same input returns `RunOutcome::Stale`, so `apply` runs once.
- Before each retry the sweep checks the team toggle for the kind. When it is off, the row is marked failed ("skipped: AI feature turned off for this team") and no data is sent.
- Every backend node runs the sweep. `SKIP LOCKED`, the claim spacing, the claim count and the atomic `start_attempt` keep this bounded.

### UI (`feature-toggle-ui/src/`)

| Piece | Where | Use it for |
|---|---|---|
| `getAiStatus`, `getTeamAiSettings`, `updateTeamAiSettings` | `api/ai.ts` | Add each later endpoint's call to this module in the task that owns it. |
| `useAiFeatures(teamId)` | `hooks/useAiFeatures.ts` | `const ai = useAiFeatures(selectedTeam?.id)`. A flag is true only when the server has a key and the team toggle is on. Errors give all false. Hide AI UI when the flag is false. |
| `invalidateAiFeatures(teamId)` | same | Marks the cached settings stale. Mounted consumers do not refetch until they remount (see §4). |
| Settings page | `pages/AiSettingsPage.tsx`, route `/settings/ai` | Admin-only, team-scoped. |

`useSharedQuery` now handles a failed background fetch (mount or poll): the error is stored in `error` and no unhandled rejection occurs. `refetch()` still rejects to its caller.

## 3. Environment and verification

- **Database:** local Postgres, user `postgres`; the password is in your shell's `DATABASE_URL`. Do not commit it.
  - Main dev DB `feature_toggle` has no seed data. About 10 seed-dependent backend tests fail there; this is not caused by the AI work.
  - Use the test DB `feture_toggle_test` (migrated and seeded) for the full suite:
    ```bash
    export DATABASE_URL="${DATABASE_URL%/*}/feture_toggle_test"
    cargo test -p feature-toggle-backend
    ```
  - To reset it: `psql "$DATABASE_URL" -c 'DROP SCHEMA public CASCADE; CREATE SCHEMA public;'`, then `sqlx migrate run --database-url "$DATABASE_URL" --source feature-toggle-backend/migrations` and `psql "$DATABASE_URL" -f init.sql`.
- **Run the backend for an end-to-end check** without clashing with a dev server: write a TOML with `http_addr = "127.0.0.1:18180"` and `grpc_addr = "127.0.0.1:15151"` outside the repo, then run `FEATURE_TOGGLE_CONFIG=<that file> ./target/debug/feature-toggle-backend` from `feature-toggle/` (it needs `log4rs.yaml` in the working directory). On an empty DB, create the admin with `POST /api/v1/admins` (`api-test-admin` / `password123`, as `api-tests` does), then `POST /api/v1/auth/login`; the token is in `.token`. Start with `env -u TYPESAFE_API_KEY` to check the no-key path.
- **TypeSafe key:** `TYPESAFE_API_KEY` is set in the shell. Never write it to a file, log, fixture or commit. Live smoke test: `cargo test -p feature-toggle-backend --test typesafe_live_test -- --ignored`. The live tuning tests in each feature task (`#[ignore]`, design §7) use the same key.
- **UI:** use pnpm, never npm: `pnpm lint`, `pnpm build`, `pnpm test:run` in `feature-toggle-ui/`.
- **Contracts:** after any DTO or endpoint change, run `./scripts/export-contracts.sh`, copy `feature-toggle-backend/contracts/generated/contract-hashes.json` to `contracts/baseline/`, then `./scripts/check-contract-compat.sh`. Only the baseline file is tracked.
- **Migrations:** the latest is `20261003010000_ai_judgment_claims.sql` (after `20261002030000_ai_judgments`, `..040000_approval_policy_ai_risk_mode`, `..050000_feature_flag_kind`, `..060000_approval_request_required_approvers_override`). New ones must sort after it. The final-review fix waves added no migration; the known-issue fixes of 2026-10-03 added `ai_judgment_claims`.
- After code changes run `graphify update .` in the repo you changed (`graphify-out/` is not committed).

## 4. Known gaps left open (decide when you touch the area)

Backend:

- **NL search owner filter** uses `f.owner ILIKE %owner%` (inherited from the list endpoint): the chip value is not an exact match, and `_`/`%` in an owner name act as wildcards. A tag named `unspecified` is offered but ignored; owner grouping is case-sensitive.
- **Retry sweep re-claim.** A claimed row is not claimed again for 2 minutes (`claimed_at`). A sweep run that is still going after that (a long batch waiting for background permits) can be claimed again by another node; `start_attempt` still keeps the API runs of one input at 3.
- **Rule-decided justification results:** a crash between `upsert_pending` and `mark_done` in `record_rule_result` leaves a pending row the sweep then sends to the API.
- **Flag kind:** re-classification runs only on create and update (not bulk tag actions or rollback); an unknown stored kind maps to `None` silently (the CHECK constraint prevents it); the backfill scans all judged rows per call.
- **AI-11 vote window:** a vote reads the request and counts who can still vote, then records; an override (or an approver change) that lands in between is counted against the old requirement. `enforce_extra_approver` errors skip the activity write (fail open). A request whose `eligible_approver_ids` is empty (created without a database pool, or a legacy row from before routing) gets `required + 1` with no cap; admin override and cancel are the escapes there.
- **Approval reconciliation** runs on the auto-approval tick (60 s), so a request nobody is left to vote on is approved up to a minute after the last approver leaves. It uses the database pool path only.
- When a key is set but the HTTP client cannot be built, `lib.rs` logs "disabled (no TYPESAFE_API_KEY)" after the real `error!` line. The message is misleading.
- `GET /teams/{team_id}/ai-settings` has no team-membership check, the same as the neighbouring team GET routes. Any authenticated user can read another team's four toggles. `GET` for a team id that does not exist returns 200 with everything off.
- The live smoke test's `expect` prints the error's `Debug` text, which can include a response body snippet; this only reaches local test output.

UI:

- **AI-41:** the NL search request has no abort or timeout (a slow call keeps "Searching…" until it returns); the "Ask" item is first, so Enter runs the AI search instead of the best fuzzy match (as the brief asked).
- `invalidateAiFeatures` does not make already-mounted consumers refetch (the palette now refetches on open, `74242a7`; other consumers update on next mount).
- `AiSettingsPage` keeps the previous team's values on screen (with disabled switches) while a newly selected team loads or after a load error.
- AI-21 minors: no blur/check-now trigger (debounce only), the cache key omits feature key and team, in-flight requests are not de-duplicated.

## 5. Notes per task

- **AI-10 (done, `69ef888`):** pattern to copy: `judgment/approval_risk.rs` (pure `build_input`, `build`, `derive`; `apply` re-checks that its subject exists), a `submit_*` helper in the logic that skips on `None`/team toggle/policy and swallows errors, `load_ai_risk` in `rest/approval.rs` for batch reads (`map_request_with_policy` takes the summary). `AiJudgmentRepository` is a `web::Data`. Live tuning test and fixture: `tests/approval_risk_live_test.rs`, `tests/fixtures/ai/approval_risk.json` (accuracy 0.92). Mode `off` skips; `advisory` only records; the enforcing modes are AI-11. Details in the task's handoff log.
- **Final-review fixes (2026-10-03):** reasons are recorded after the edge broadcast and after the change commits (pattern: collect `FreezeOverride`s, record them next to `record_flag_kind`); `map_ai_risk` reports a pending row older than 10 minutes as `failed`; approval list and stream use `rest::approval::load_policies`; `ApprovalLogicImpl` publishes the request event before `submit_risk_assessment` and again when it queued an assessment. Handler test helper `judgment::justification::test_support::recording_runtime_with_probe` lets a test observe what happened before a reason was submitted. Full report: `.superpowers/sdd/HANDOFF/final-backend-fix-report.md`.
- **AI-11 (done, `8cde72c`, signed off 2026-10-02; user decisions 2026-10-03 in `40ebcdf`; known-issue fix `bbca40a`):** the override is `min(required_approvers + 1, eligible approver count)` when the request lists eligible approvers, `required_approvers + 1` when the list is empty. Routing resolves named approvers and role members into the list, so the cap applies to role-based policies; the list is empty only for a request created without a database pool or a legacy row from before routing. Votes and `requiredApprovalsEffective` use `effective_required_approvals` (`logic/approval.rs`): `max(policy required, min(override, approved + remaining))`, where `remaining` is the snapshot approvers who still qualify (same SQL as routing: `database::approval::approver_qualifies_sql`) and have not voted. `AutoApprovalScheduler::reconcile_capped_requests` approves a pending request with an override, no remaining approver and `approved >= policy required` via `ApprovalLogic::approve_capped_request` (one guarded `UPDATE`, change executed, activity `approval_requirement_reconciled`). When the cap leaves nothing to add, no override is written and the `approval_risk_assessed` metadata gets `"extra_approver_skipped": "no_additional_eligible_approver"`. `require_extra_approver` with a done `high` judgment also blocks auto-approval, like `gate_auto_approve`. `approval_requests.required_approvers_override` is set in `ApprovalRiskHandler::apply` (SQL guard `status = 'pending' AND required_approvers_override IS NULL` via `set_required_approvers_override`) and read by `apply_vote(_tx)` and `required_approvals_effective`. Auto-approval is held by a `NOT (...)` clause in `list_requests_due_for_auto_approval` for both enforcing modes. Both use only `done` judgments with `derived->>'level' = 'high'`; a closed request is never changed. DB tests: `tests/database/approval_risk_enforcement_test.rs`.
- **AI-20 (done, `ddd3a6d`):** pattern to copy for recording: `judgment::justification::record_justification` (skips on no service, toggle off, blank input; swallows errors) called from REST handlers after the write, with `Option<web::Data<AiRuntime>>` as the extractor so existing apps and tests need no AI data; `rest::ai::record_reason` wraps it. `JudgmentService::record_rule_result` stores a rule-decided `done` row and runs `apply`; reuse it for any rule shortcut. `create_activity(_tx)` ids are threaded out only on the paths AI-20 needs (`emergency_*_in_tx` return a tuple, `update_feature_in_tx` returns `FeatureUpdateOutcome`, `enforce_freeze_*` return `Option<FreezeOverride>`). `ActivityLogRepository::merge_activity_metadata` adds a key to an entry's metadata. Handler test helper: `judgment::justification::test_support::recording_runtime`. Live fixture `tests/fixtures/ai/justification.json` (accuracy 1.00, 34 cases). Details in the task's handoff log.
- **AI-30 (done, `f928d65`, fixes `cf3e23f`):** `model::FlagKind`, `FlagKindSource` and `FlagKindFilter` live in `model.rs` (the REST layer reuses them; `FlagKind` is in `ApiDoc`). Columns `flag_kind`, `flag_kind_source`, `flag_kind_confidence` are on the entity, `FeatureResponse`, and every `SELECT` list in `database/feature.rs`. `flag_kind` filter: `FeatureLogic::get_features_with_offset_filtered` and the repository take `Option<FlagKindFilter>` after `approval_status` (AI-40 reuses it). `PATCH` takes `flagKind` as `Option<Option<_>>` (absent, `null`, value); only a changed value counts (`resolve_flag_kind_update`). Pattern to copy for a record-after-commit trigger: `judgment::flag_kind::record_flag_kind` plus `rest::ai::record_flag_kind`, called from `create_feature` and `update_feature` (update calls it after the edge broadcast). `JudgmentService::judgment_for` / `judgments_for` read stored judgments so a trigger can skip an unchanged input. `judgment::flag_kind::test_support` has a recording judgment service for handler tests. Live fixture `tests/fixtures/ai/flag_kind.json` (accuracy 1.00, 27 cases). Details in the task's handoff log.
- **AI-31 (done, `335b2f6`, signed off 2026-10-02):** `logic::feature::stale_reasons` (free `pub(crate)` fn, also used by `feature_tx.rs`; `FeatureLogicImpl::stale_reasons` delegates) and `stale_predicate_sql` (`database/feature.rs`) skip the three inactivity rules for `FlagKind::PERMANENT` (`ops`, `permission`, `config`; `FlagKind::is_permanent`). Keep them equivalent: parity test `logic::feature::stale_rules_tests::rust_rules_and_sql_stale_filter_agree` (kinds incl. NULL x 10 rule scenarios). Any new stale rule must change both and add a scenario row.
- **AI-40 (done, `86dedbb`, fixes `e3c3611`):** endpoint `POST /teams/{team_id}/features/nl-search` lives in `rest/ai.rs` (`nl_search_features`); the whole two-call flow is `judgment::nl_search::search(client, features_repo, feature_logic, team_id, query, limit)`, so handler and live test share one path. Needs `web::Data<Box<dyn FeatureRepository>>` and `web::Data<Box<dyn FeatureLogic>>` besides the AI data. Response `{available, filtersApplied, results: [{feature, relevance | null}]}`; `feature` is the list item shape (`feature_base_response`, now `pub(crate)`). `filtersApplied` values use the REST enum spelling (`ARCHIVED`, `CONTEXTUAL`), so the UI can pass them straight back to the list endpoint as query values; only applied filters are present. New repository methods `get_team_owner_candidates` and `get_features_by_usage_filtered` (the sort `evaluation_count_30d DESC, key` is internal: `FeatureOrder` in `database/feature.rs`), logic method `FeatureLogic::search_features_by_usage`. Live fixture `tests/fixtures/ai/nl_search.json` (25 features, 28 queries), run `--test nl_search_live_test -- --ignored --nocapture` (needs `DATABASE_URL`; it creates and deletes its own team). Filter accuracy 24/28 = 0.86. Details in the task's handoff log.
- **UI tasks (all done):** read the flag with `useAiFeatures(selectedTeam?.id)`. Mock `@/api/ai` in tests. Use design-token classes only. Avoid `waitFor` right after a fake-timer test in the same file; flush with `act` instead (see `hooks/useSharedQuery.test.tsx`).
