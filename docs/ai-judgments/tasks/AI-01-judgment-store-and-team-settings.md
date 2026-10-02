# AI-01: Judgment store, team AI settings, `JudgmentService`, and retry sweep

| Field | Value |
|---|---|
| Type | Feature (foundation) |
| Status | Done in db0dc7e (commits 88728a1..db0dc7e) |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-00 |
| Behavior change | No. All team toggles default to off. |
| Design | [design.md §3.4, §4.1, §4.2](../design.md#34-judgmentservice) |

## Goal

Give later tasks one way to run an async judgment: `JudgmentService::submit(...)`. It persists the input, calls Jev, stores raw answers and a derived result, runs a kind-specific apply step, and retries failures. Also add per-team toggles with a REST API.

## Current code (verify first)

| What | Where |
|---|---|
| Latest migration (yours must sort after it) | `feature-toggle-backend/migrations/20260610000000_feature_metadata_bulk_audit_dependency.sql` |
| `teams` table and id types | `migrations/*team*` and `src/database/entity.rs` (`pub struct Team`) |
| Repository pattern to copy | `src/database/approval.rs`: `#[automock]` trait, `clone_box`, `approval_repository(pool)` factory |
| Scheduler pattern | `src/scheduler/auto_approval.rs` and its start in `src/lib.rs::run` (`tokio::spawn(async move { x.start().await })`) |
| Team routes and admin checks | `src/rest/team.rs`; admin-gated settings example: notification settings handlers in `src/rest/` (`/notifications/settings`) |
| Error types | `crate::Error` (`src/lib.rs`), `rest/error.rs` (`RestError`, `From<crate::Error>`) |

## Changes

1. **Migration** `migrations/<timestamp>_ai_judgments.sql`
   - Create `team_ai_settings` and `ai_judgments` exactly as in design §4.1 and §4.2.
   - Match the real types of `teams.id` and the user id column used elsewhere, for example in `approval_requests.requested_by`.
2. **Repositories** `src/database/ai.rs`
   - `TeamAiSettingsRepository`: `get(team_id) -> TeamAiSettings` (all false when no row exists) and `upsert(team_id, settings, updated_by)`.
   - `AiJudgmentRepository`:
     - `upsert_pending(team_id, kind, subject_type, subject_id, input, input_hash) -> AiJudgment`. Use `ON CONFLICT (subject_type, subject_id, kind) DO UPDATE` to reset `status='pending'`, `attempts=0`, `input`, `input_hash`, `raw_answers=NULL`, `derived=NULL`, `error=NULL`, `created_at=NOW()`, `completed_at=NULL`.
     - `mark_done(id, input_hash, model, raw_answers, derived, input_tokens) -> bool`, guarded by `WHERE id=$1 AND input_hash=$2`. Return false when 0 rows.
     - `mark_failed(id, input_hash, error) -> bool`, same guard; increments `attempts`.
     - `get_for_subject(subject_type, subject_id, kind) -> Option<AiJudgment>`.
     - `get_for_subjects(subject_type, ids: &[Uuid], kind) -> Vec<AiJudgment>`, for list endpoints.
     - `list_retryable(limit) -> Vec<AiJudgment>`: `(status='pending' AND created_at < NOW() - INTERVAL '2 minutes') OR (status='failed' AND attempts < 3)`, ordered by `created_at`.
   - Both traits use `#[automock] #[async_trait]` and factory functions like the existing repositories.
3. **Service** `src/judgment/service.rs`
   - Enums `JudgmentKind` (`approval_risk`, `justification`, `flag_kind`) and `SubjectType` (`approval_request`, `feature`, `activity`, `freeze_window`, `scheduled_change`) with `as_str()` and `FromStr`.
   - `trait JudgmentHandler` exactly as in design §3.4.
   - `JudgmentService { client: Arc<dyn JudgmentClient>, repo, settings_repo, handlers: HashMap<JudgmentKind, Arc<dyn JudgmentHandler>> }`.
     - `register(handler)`.
     - `submit(team_id, kind, subject_type, subject_id, input)`: compute `input_hash` (SHA-256 of `serde_json::to_vec(&input)`; use `BTreeMap`-backed values so the key order is stable), `upsert_pending`, then `tokio::spawn(self.clone().run(row))`. It returns right after the upsert and never returns the API result.
     - `run(row)`: `handler.build(&row.input)`, then `client.evaluate`, then `handler.derive`, then `mark_done`. If `mark_done` returned true, call `handler.apply(&row_with_results)`. On error, `mark_failed`. Log errors; never panic.
     - `team_enabled(team_id, feature) -> bool` helper for callers.
     - `retry_tick()`: `list_retryable(50)`, then `run` each one sequentially.
   - Callers hold `Option<Arc<JudgmentService>>`. `None` means the subsystem is unavailable. Construct it in `lib.rs::run` only when the AI-00 client is `Some`.
4. **Scheduler** `src/scheduler/ai_judgment_retry.rs`: every 60 s, call `retry_tick()`. Start it in `lib.rs::run` only when the service exists.
5. **REST** `src/rest/ai.rs`
   - `GET /api/v1/teams/{team_id}/ai-settings` returns `{ available, approvalRisk, justificationCheck, flagKind, nlSearch, updatedAt }`. Use team membership auth like the other team routes.
   - `PUT /api/v1/teams/{team_id}/ai-settings` takes the four booleans. Admin only, with the same check as other admin-only settings endpoints. Write an activity log entry `ai_settings_updated` with the old and new values.
   - Register both in `ApiDoc` and update the contract baseline.

## Tests

- Repository tests in `tests/database/` (follow the existing DB test setup):
  - `upsert_pending` resets an existing row.
  - `mark_done` with a stale hash returns false and changes nothing.
  - `list_retryable` picks the right rows.
- Service unit tests with `MockJudgmentClient` and a fake handler:
  - The success path writes done and calls apply once.
  - The client error path writes failed and does not call apply.
  - A newer `submit` during an in-flight run makes the old run's `mark_done` a no-op, so apply is not called for the old run.
- REST: GET with no row returns all false; PUT as non-admin returns 403; PUT as admin persists.

## Acceptance criteria

- [x] Migrations apply on a clean DB and on a DB with existing data.
- [x] Without a key, the service is not built, the scheduler is not started, and the settings API still works and returns `available: false`.
- [x] With a key and a fake handler registered in a test, a submitted judgment reaches `done` and apply runs once.
- [x] A failed judgment is retried by the sweep at most 3 times in total.
- [x] Contract baseline is updated; all backend tests pass.

## Out of scope

Feature handlers (AI-10, AI-20, AI-30) and UI (AI-02).

## Handoff log

_No entries yet._

### 2026-10-02, Claude (plan 2026-10-02-ai-judgments-foundation)

- Changed: migration `20261002030000_ai_judgments.sql`; `database::ai` (`TeamAiSettingsRepository`, `AiJudgmentRepository`, unconditional automocks); `judgment::service` (`JudgmentHandler`, `JudgmentService`, `RunOutcome`, `input_hash`); `AiJudgmentRetryScheduler` (60 s, started only with a key); `GET`/`PUT /api/v1/teams/{team_id}/ai-settings` (PUT is system-admin only, logs `ai_settings_updated`); contract baseline.
- Verified: migrations applied to an empty DB, then the AI migration applied after seeding with `init.sql`; repository tests against Postgres; service tests with `MockJudgmentClient`; REST tests; full `cargo test -p feature-toggle-backend` green on the seeded test DB (`feture_toggle_test`). The developer DB `feature_toggle` is not seeded, so 10 seed-dependent tests fail there; unrelated to this change.
- Deviations: `AiRuntime.judgments` holds `Option<Arc<JudgmentService>>`; register handlers with `JudgmentService::with_handler` before wrapping in `Arc` (in `lib.rs::run`). `mark_failed` never overwrites `done`. `updated_by` has no FK. PUT for an unknown team returns 404. Repository string args are owned (`String`) for mockall.
- Open: the no-key server start check is verified end to end in the plan's Task 10; see the AI-02 handoff entry.
- Next: feature tasks build a handler, register it in `lib.rs::run`, and call `runtime.judgments` / `team_enabled(team_id, AiFeature::...)` before `submit`.
