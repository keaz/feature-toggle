# AI judgments: handoff for the remaining work

Date: 2026-10-02. Read this after [`README.md`](README.md) and [`design.md`](design.md), and before you pick a task under [`tasks/`](tasks/). It records what is built, what changed from the design, and what the next tasks must know.

## 1. Status

| Task | State | Where |
|---|---|---|
| AI-00 client, config, `/ai/status` | Done | backend `e9bf1e6..f7ddccb` |
| AI-01 judgment store, team settings, service, retry sweep | Done, plus pre-AI-10 fixes | backend `88728a1..db0dc7e`, fixes `5ec630e` |
| AI-02 UI settings page and `useAiFeatures` | Done | UI `02e7fd3`, `80bb277`, race fix `cb32e5b`, `useSharedQuery` fix `813342a` |
| AI-10 approval risk assessment | Done | backend `69ef888` |
| AI-20 justification check backend | **Ready to start** | backend |
| AI-30 flag kind backend | **Ready to start** | backend |
| AI-40 NL search backend | **Ready to start** (better after AI-30, for the `flag_kind` filter) | backend |
| AI-11 risk enforcement | **Ready to start** (AI-10 done). **Needs maintainer sign-off before merge** | backend |
| AI-31 stale rules use flag kind | Blocked by AI-30. **Needs maintainer sign-off before merge** | backend |
| AI-12 UI approval risk | **Ready to start** (AI-11 only for the extra-approver count) | UI |
| AI-21 UI justification hint | Blocked by AI-20 | UI |
| AI-32 UI flag kind | Blocked by AI-30 | UI |
| AI-41 UI NL search palette | Blocked by AI-40 | UI |

Suggested order: AI-20, AI-30 and AI-40 can run in parallel (AI-10 is done). Each UI task follows its backend task. AI-11 and AI-31 last, after sign-off.

Repo practice for this project: commit directly on `main` in each repo (the README's branch-per-task rule is not used here). Stage files by explicit path; both repos have unrelated local changes.

## 2. What exists and how to plug a feature in

### Backend (`feature-toggle-backend/src/`)

| Piece | Where | Use it for |
|---|---|---|
| `TypesafeConfig` (`[typesafe]`) | `config.rs` | Model `jev-1.13.0` pinned, `timeout_ms` 3000, `max_in_flight` 16. Zero values fall back to defaults. |
| `JudgmentClient` trait, `MockJudgmentClient`, `HttpJudgmentClient` | `judgment/client.rs` | Sync calls: `client.evaluate(state, questions)`. Retries 429/529/connect/timeout (also body-read timeouts) at most twice. `JudgmentError::log_label()` is the only thing safe to log. |
| Wire types, question builders | `judgment/types.rs` | `Question::noul`, `noul_with`, `choice` (2–255 options), `score` (2–10 levels); `Answers::noul/choice/score`; `RequestParts`. |
| `JudgmentKind`, `SubjectType` | `judgment/mod.rs` | `JudgmentKind::feature()` gives the team toggle (`AiFeature`) for a kind. |
| `AiRuntime` | `judgment/mod.rs`, registered as `web::Data<AiRuntime>` | `runtime.client` (sync endpoints), `runtime.judgments` (async pipeline), `runtime.available()`. Both are `None` without `TYPESAFE_API_KEY`. |
| `JudgmentService`, `JudgmentHandler` | `judgment/service.rs` | Async judgments: `submit(team_id, kind, subject_type, subject_id, input)`; `team_enabled(team_id, AiFeature::...)`. |
| Repositories | `database/ai.rs` | `TeamAiSettingsRepository` (registered as `web::Data<Box<dyn TeamAiSettingsRepository>>`), `AiJudgmentRepository` (`get_for_subject`, `get_for_subjects` for list endpoints). Both have unconditional `#[automock]`, so integration tests can use the mocks. |
| Retry sweep | `scheduler/ai_judgment_retry.rs` | Runs every 60 s, only when a key is set. |
| REST | `rest/ai.rs` | `GET /ai/status`, `GET`/`PUT /teams/{team_id}/ai-settings` (PUT is system-admin only). |

**Adding an async judgment (AI-10, AI-20, AI-30):**

1. Write `judgment/<kind>.rs` with a `JudgmentHandler`. Keep `build` and `derive` pure (snapshot tests on the request, table tests on `derive`). `apply` must re-check that its subject is still fresh, as design §3.4 says.
2. Register it in `lib.rs::run`: today the service is built as `Arc::new(JudgmentService::new(...))`. Change that to `JudgmentService::new(...).with_handler(Arc::new(YourHandler::new(...)))` and then wrap it in `Arc`. Handlers cannot be added after the `Arc` exists.
3. **Wiring order (done in AI-10).** The trigger is in `ApprovalLogicImpl::maybe_create_stage_change_request` (`logic/approval.rs`), but `lib.rs::run` builds `approval_logic` before `judgment_service`. AI-10 moved the `team_ai_settings_repository` / `judgment_service` block above the logic services and registers `ApprovalRiskHandler` there; add your handler to the same `with_handler` chain. Pass `Option<Arc<JudgmentService>>` into the logic constructor (see `approval_logic_with_notifications` for the pattern: tests pass `None`). The same applies to the feature handlers in AI-20 and AI-30 if they submit from logic code.
4. At the call site: skip when the service is `None`, check `team_enabled(team_id, kind.feature())`, then `submit(...)`. Never let a submit error fail or slow the user's request: log it and continue.

**Adding a sync endpoint (AI-20 pre-check, AI-30 suggestions, AI-40 NL search):** take `web::Data<AiRuntime>` and `web::Data<Box<dyn TeamAiSettingsRepository>>`. Return HTTP 200 with `{"available": false}` when `runtime.client` is `None`, the team toggle is off, or the call fails. Return 400 only for invalid input (design §6).

### Retry and attempt semantics (changed from the original design text; design §3.4 is updated)

- `attempts` counts runs that started. `submit` writes the row with `attempts = 1`.
- The sweep claims rows with `claim_retryable(50)`: pending rows older than 2 minutes and failed rows, both only while `attempts < 3`. It increments `attempts` as it claims (`FOR UPDATE SKIP LOCKED`). A row runs at most 3 times in total, even when its result can never be stored.
- `mark_failed` does not change `attempts` and never overwrites a `done` row.
- `mark_done` is guarded by `input_hash` **and** `status <> 'done'`. A second run of the same input returns `RunOutcome::Stale`, so `apply` runs once.
- Before each retry the sweep checks the team toggle for the kind. When it is off, the row is marked failed ("skipped: AI feature turned off for this team") and no data is sent.
- Every backend node runs the sweep. `SKIP LOCKED` plus the attempt count keeps this bounded.

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
- **Migrations:** the latest is `20261002030000_ai_judgments.sql`. New ones must sort after it (the task files still name an older baseline).
- After code changes run `graphify update .` in the repo you changed (`graphify-out/` is not committed).

## 4. Known gaps left open (decide when you touch the area)

- `useAiFeatures`: `invalidateAiFeatures` does not make already-mounted consumers refetch, because the `useSharedQuery` fetch effect does not depend on the snapshot. AI-12, AI-21, AI-32 and AI-41 should either accept "updates on next mount" or call the hook's `refetch` after a settings change.
- `AiSettingsPage` keeps the previous team's values on screen (with disabled switches) while a newly selected team loads or after a load error.
- When a key is set but the HTTP client cannot be built, `lib.rs` logs "disabled (no TYPESAFE_API_KEY)" after the real `error!` line. The message is misleading.
- `GET /teams/{team_id}/ai-settings` has no team-membership check, the same as the neighbouring team GET routes. Any authenticated user can read another team's four toggles.
- `GET` for a team id that does not exist returns 200 with everything off (missing row = all off).
- The live smoke test's `expect` prints the error's `Debug` text, which can include a response body snippet; this only reaches local test output.

## 5. Notes per remaining task

- **AI-10 (done, `69ef888`):** pattern to copy: `judgment/approval_risk.rs` (pure `build_input`, `build`, `derive`; `apply` re-checks that its subject exists), a `submit_*` helper in the logic that skips on `None`/team toggle/policy and swallows errors, `load_ai_risk` in `rest/approval.rs` for batch reads (`map_request_with_policy` takes the summary). `AiJudgmentRepository` is a `web::Data`. Live tuning test and fixture: `tests/approval_risk_live_test.rs`, `tests/fixtures/ai/approval_risk.json` (accuracy 0.92). Mode `off` skips; every other mode behaves as `advisory` until AI-11. Details in the task's handoff log.
- **AI-11 (sign-off):** `required_approvers_override` and `gate_auto_approve` use only `done` judgments with `derived->>'level' = 'high'`. Never reopen a closed request.
- **AI-20:** `create_activity` / `create_activity_tx` must return the row id so the judgment can use (`activity`, id) as its subject. The rule check (ticket, URL, `#123`, `INC123`) records `done` without an API call; the service has no helper for that yet, so add a repository method or a handler path that writes `done` directly.
- **AI-30:** adding `flag_kind` fields to `model::Feature` breaks many struct literals in tests; fix them in the same change. `PATCH /features/{id}` is a full-body replace: only a changed `flag_kind` counts as a user choice (`Option<Option<_>>`).
- **AI-31 (sign-off):** keep `FeatureLogicImpl::stale_reasons` and `stale_predicate_sql` equivalent.
- **AI-40:** call 1 sends the team's tag and owner names (the settings page notice already says so). Keep Choice options at most 255 (254 values plus `none`/`unspecified`).
- **UI tasks:** read the flag with `useAiFeatures(selectedTeam?.id)`. Mock `@/api/ai` in tests. Use design-token classes only. Avoid `waitFor` right after a fake-timer test in the same file; flush with `act` instead (see `hooks/useSharedQuery.test.tsx`).
