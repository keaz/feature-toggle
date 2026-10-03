# JI-43: Capture results into the outbox (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 84b2611 |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-40, JI-42 |
| Behavior change | **Yes, visible in Jira.** With write-back on, FluxGate starts posting comments and remote links. Off by default. |
| Design | [design.md §3.3, J20, J21](../design.md#33-capture-ji-43) |

## Goal

Fill `jira_outbound_jobs` from:
- **Source 1:** the inbound Jira event handler. One comment per processed event.
- **Source 2:** a scheduler that reads `activity_log` with a cursor. Comments and remote-link jobs for every other change of a linked feature.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Inbound handler | `rest/jira_events.rs` `receive_jira_event` (~155-281): every insert on the pool; duplicates return before any insert (~239-244); the engine outcome is `EventResults { results, unknown_environments, unknown_features }` (`logic/jira_rules.rs` ~108-130) |
| Event repository | `database/jira_event.rs`: `insert(NewJiraEvent)` (~52), pool only; `NewJiraEvent` (~36) |
| Activity repository | `database/activity_log.rs`: `get_activities` is DESC only, no team filter. A new query is needed. Indexes include `(activity_type, created_at DESC)`. |
| Links | `database/external_link.rs`: `list_for_feature(feature_id)` (~36), `feature_scope(feature_id) -> {team_id, key}` (~34) |
| Integrations | `database/jira_integration.rs` `list_for_team(team_id)`; every row has `actor_user_id` |
| User names | `database/user.rs` `get_user_by_id` (~85) |
| Activity row shapes | JI-40 handoff log and the JI-40 interface; kill switch rows `logic/feature_tx.rs` ~935-1051 (entity feature, `metadata.feature_id`, `reason`); link rows `logic/external_link_tx.rs` ~96-130 (`metadata.feature_id`, `external_key`, `system`) |
| Outbox | `JiraOutboundJobRepository(Tx)`, `NewOutboundJob`, `OutboundKind` (JI-42) |

## Interfaces

- Consumes: JI-42 repository and types; JI-40 activity types and metadata keys.
- Produces:
  - `NewJiraEvent` gains `id: Uuid`, set by the caller with `Uuid::new_v4()`. `insert` uses it.
  - `JiraEventRepository::insert_with_jobs(&self, event: NewJiraEvent, jobs: Vec<NewOutboundJob>) -> Result<JiraEventRow, Error>`. It runs in one transaction and enqueues with `enqueue_tx`.
  - `ActivityLogRepository::list_window(&self, types: &[&str], from: DateTime<Utc>, to: DateTime<Utc>, limit: i64) -> Result<Vec<ActivityLog>, Error>`. Filters `created_at >= from AND created_at <= to AND activity_type = ANY(types)` and orders by `created_at, id` ascending.
  - Migration `<ts>_jira_writeback_cursor.sql`: `jira_writeback_cursor` from design §3.3.
  - `logic/jira_capture.rs` (pure, unit-tested):
    ```rust
    pub const CAPTURE_TYPES: &[&str];           // the 12 types of the mapping table in design §3.3 (feature_updated included; filtered below)
    pub fn event_comment_lines(jira_status: &str, outcome: &EventResults) -> Option<Vec<String>>;   // None when nothing to report
    pub struct CaptureContext {
        pub feature_key: String,
        pub issue_keys: Vec<String>,                 // jira links of the feature (for link rows: only the row's external_key)
        pub integrations: Vec<WritebackTarget>,      // write-back-enabled integrations of the feature's team
        pub jira_actor_ids: HashSet<Uuid>,           // actor_user_id of every Jira integration (all teams)
        pub actor_name: Option<String>,              // resolved name of row.actor_id
    }
    pub struct WritebackTarget { pub integration_id: Uuid, pub comments: bool, pub remote_link: bool }
    pub fn activity_comment_lines(row: &ActivityLog, ctx: &CaptureContext) -> Option<Vec<String>>;
    pub fn plan_jobs(row: &ActivityLog, ctx: &CaptureContext) -> Vec<NewOutboundJob>;
    ```
  - `scheduler/jira_writeback_capture.rs`: `JiraWritebackCapture::new(pool, interval)`, `run_once(&self) -> Result<usize, Error>` (jobs enqueued). Spawned in `lib.rs`, 5 s.

## Rules

**Source 1** (in `receive_jira_event`):
- Only for events that ran the engine: no duplicates, no ignored events, no parse errors.
- Only when the receiving integration has `writeback_enabled` and `writeback_comments`.
- Then `insert_with_jobs(event, vec![comment job])`:
  - The comment job has `issue_key` = the event's issue, `feature_id = None`, `payload = {"lines": [...]}` and `dedupe_key = format!("event:{}", event.id)`.
  - In every other case, `insert` as today.
- `event_comment_lines` output:
  ```
  FluxGate: Jira status 'Ready for Release'
  flag-x · qa: approve applied
  flag-x · prod: deploy refused: not approved
  Unknown environment values: Staging-EU
  Unknown features: flag-y
  ```
  It returns `None` when there are no results and no unknown values.

**Source 2** (`run_once`, one transaction per tick):
1. Read the cursor. If there is none, insert `now()` and return 0.
2. `rows = list_window(CAPTURE_TYPES, cursor - 60 s, now() - 2 s, 500)`. Drop `feature_updated` rows that have no `metadata.target_version_id`.
3. Build the context for each row:
   - `feature_id` comes from `metadata.feature_id`. Skip the row if it is missing or not a UUID.
   - `feature_scope` gives the team and key. Skip the row if the feature is gone, except for `external_link_removed`: then use `metadata.feature_key` and `metadata.team_id`.
   - `issue_keys`:
     - For `external_link_added` and `external_link_removed`: `[metadata.external_key]`, only when `metadata.system == "jira"`.
     - Otherwise: every link of the feature with `system = "jira"`.
   - Cache the per-team integrations, the global `jira_actor_ids` and the user names for the tick.
4. `plan_jobs` returns jobs. `enqueue_tx` each one.
5. If any rows were read, set the cursor to the newest `created_at` read. Commit.
6. If 500 rows came back, run again immediately (loop) until fewer than 500 come back.

**`plan_jobs` mapping.** For each `issue × integration`:
- **Comment kinds:** `stage_approved`, `stage_rejected`, `stage_deployed`, `stage_rollbacked`, `approval_request_cancelled`, `kill_switch_activated`, `kill_switch_deactivated`.
  - A `comment` job, if the target has `comments` and `row.actor_id` is not in `jira_actor_ids`. Dedupe key `activity:<rowId>:<integrationId>:<issue>`.
  - A `remote_link` job, if the target has `remote_link`. Dedupe key `link:<integrationId>:<issue>:<featureId>:activity:<rowId>`.
- **Link-only kinds:** `stage_change_requested`, `approval_request_approved_externally`, `feature_updated` (version rollback) and `external_link_added` get `remote_link` only.
- **`external_link_removed`:** `remote_link_delete`, if the target has `remote_link`. Payload `{"featureId": "<id>"}`, dedupe key `unlink:<rowId>:<integrationId>`.

**`activity_comment_lines`.** The first line is `FluxGate: <feature_key>`. The second line:

| Type | Line |
|---|---|
| `stage_approved` | `Approved for <env> by <actor>` |
| `stage_rejected` | `Rejected for <env> by <actor>` |
| `stage_deployed` | `Deployed to <env> by <actor>` |
| `stage_rollbacked` | `Rolled back in <env> by <actor>` |
| `approval_request_cancelled` | `Approval request for <env> cancelled by <actor>` |
| `kill_switch_activated` | `Kill switch activated by <actor>`; when `metadata.rollback_in_minutes > 0`: `Kill switch scheduled by <actor> in <n> min` (the disable only scheduled it); when `metadata.scheduled_execution == true`: `Kill switch activated (scheduled)` |
| `kill_switch_deactivated` | `Kill switch deactivated by <actor>` |

- `<env>` = `metadata.environment_name`, else `"an environment"`.
- `<actor>` = the row's `actor_name`, else the resolved user name, else `"FluxGate"`.
- Then `Reason: <metadata.reason>` and `Ref: <metadata.external_ref>` when present.
- No other metadata goes into a comment.

## Steps

- [ ] **Step 1: failing pure tests** in `logic/jira_capture.rs`:
  ```rust
  #[test] fn event_comment_lists_each_result_and_unknowns()
  #[test] fn event_comment_is_none_without_results()
  #[test] fn human_approval_gives_comment_and_link()
  #[test] fn jira_made_rows_refresh_the_link_without_a_comment()          // REVIEW FOCUS 2: actor_id in jira_actor_ids
  #[test] fn capture_fans_out_once_per_issue_and_integration()            // REVIEW FOCUS 1: 2 issues x 2 integrations -> 4 comments + 4 links, distinct dedupe keys
  #[test] fn comments_off_gives_links_only_and_link_off_gives_comments_only()
  #[test] fn link_added_targets_only_that_issue()
  #[test] fn link_removed_gives_delete_with_feature_id_payload()
  #[test] fn non_jira_link_rows_give_nothing()                            // metadata.system = "github"
  #[test] fn comment_never_contains_other_metadata()                      // metadata with "secret": "x" and "old_state" -> not in lines
  #[test] fn actor_falls_back_to_resolved_name_then_fluxgate()
  #[test] fn scheduled_kill_switch_says_scheduled()
  ```
- [ ] **Step 2:** run `cargo test -p feature-toggle-backend jira_capture`. It fails. Implement. It passes.
- [ ] **Step 3: Source 1.**
  - First change the `rest/jira_events.rs` mock tests:
    - `processed_event_enqueues_one_comment_when_writeback_is_on`: expects `insert_with_jobs` with one job whose `dedupe_key` is `event:<id>`.
    - `duplicate_and_ignored_events_enqueue_nothing`.
    - `writeback_off_uses_plain_insert`.
  - The tests fail. Implement `NewJiraEvent.id`, `insert_with_jobs` and the handler change. They pass.
  - Add the DB test `insert_with_jobs_is_atomic` to `tests/database/jira_event_test.rs`: a job with a dedupe key already taken still stores the event and does not fail. Make a forced failure (an invalid `kind` through raw SQL) roll back both rows.
- [ ] **Step 4: `list_window`.** Add the DB test `list_window_is_ascending_and_bounded`. It fails. Implement. It passes.
- [ ] **Step 5: failing capture tests** in `tests/database/jira_writeback_capture_test.rs`. Use a real stack like `tests/database/external_change_test.rs`: a team, a feature linked to `PROJ-1`, and an integration with write-back on. No HTTP is needed.
  ```rust
  #[tokio::test] async fn first_run_sets_the_cursor_and_enqueues_nothing()
  #[tokio::test] async fn human_deploy_enqueues_comment_and_remote_link()
  #[tokio::test] async fn rereading_the_overlap_window_does_not_duplicate_jobs()
  #[tokio::test] async fn late_committed_row_inside_the_overlap_is_captured()   // insert an activity row with created_at = cursor - 30s
  #[tokio::test] async fn rows_of_unlinked_features_are_skipped()
  #[tokio::test] async fn other_teams_integrations_get_nothing()
  #[tokio::test] async fn more_than_500_rows_are_drained_in_one_run()
  ```
- [ ] **Step 6:** implement the cursor migration and the capture scheduler. Spawn it in `lib.rs`. The tests pass.
- [ ] **Step 7: end-to-end in one test.** Add `inbound_event_and_activity_give_one_comment_each`: a Jira deploy event through the handler gives exactly one comment, from Source 1, plus one remote link, from Source 2, and no second comment.
- [ ] **Step 8:**
  - Run `cargo fmt`, `cargo clippy --all-targets` and the full backend test suite.
  - Run the contract check. No DTO change is expected; if the check fails, find out why.
  - Run `graphify update .`.
- [ ] **Step 9:** commit `feat(jira): capture results into the write-back outbox (JI-43)`.
- [ ] **Step 10: manual check.** Start the backend on the test DB. Point an integration at a local fake Jira, for example `npx http-echo-server` or a 10-line Node server; never a real site with a real token in a file. Send one inbound event and do one human approval in FluxGate. Record the requests the fake Jira received in the handoff log.

## Done when

- With write-back on, a Jira event gives one comment, and a human approval, deploy, rollback, cancel or kill switch on a linked feature gives one comment per issue per integration plus a remote link refresh.
- Jira-made changes give no second comment. Re-reads give no duplicates.
- All checks pass. Handoff log, `../HANDOFF.md` and the README table updated.

## Handoff log

2026-10-03, backend `84b2611`:
- Implemented as specified. New: `logic/jira_capture.rs` (pure), `scheduler/jira_writeback_capture.rs` (spawned in `lib.rs`, 5 s), migration `20261004070000_jira_writeback_cursor.sql`, `NewJiraEvent.id`, `JiraEventRepository::insert_with_jobs`, `ActivityLogRepository::list_window`. No DTO or endpoint change; contract check passes unchanged.
- Deviations:
  - `list_window` returns `Vec<ActivityLogRow>` with `sqlx::Error` (the trait's real types) and takes `types: &[&'static str]` (mockall cannot mock a non-static slice of borrowed strs).
  - The `insert_with_jobs_is_atomic` forced failure is a job for an unknown integration (foreign key violation), not an invalid `kind` through raw SQL: `NewOutboundJob.kind` is typed, so the repository cannot send an invalid one.
  - Source 2 comment jobs set `feature_id` (Source 1 comments keep `None`, as the brief says). The sender does not read it for comments.
  - Batch loop (fix round 1): every batch of a run reads from `cursor - 60 s` and pages by keyset `(created_at, id) > (last row)` (`ActivityLogRepository::list_window_after`), so more than 500 rows with one timestamp are all read. `to` is the database clock minus the lag, not `Utc::now()`.
  - The cursor is set with `GREATEST(old, newest)`, so a window of only overlap rows never moves it back. One transaction per batch (jobs and cursor together).
  - `JiraWritebackCapture::with_lag(Duration)` (default 2 s) exists for tests; production uses the default.
  - `jira_actor_ids` is read with `SELECT actor_user_id FROM jira_integrations` in the scheduler (no repository method).
  - Paused integrations still get jobs enqueued; the sender holds them until resume.
- Fix round 1 (review): a `remote_link_delete` job has `feature_id = NULL` (payload carries `featureId`): a removed link whose feature was deleted before the tick used to fail the foreign key and block every later tick. Each activity row's jobs are stored under a savepoint; a row whose jobs fail is rolled back, logged (row id and error code only) and skipped, and the cursor still advances. Tests: `link_removed_after_feature_deleted_enqueues_delete_without_feature_id`, `failing_row_does_not_block_the_cursor` (a test trigger fails one issue key), `more_than_500_rows_with_the_same_created_at_are_all_captured`.
- Tests: 14 pure tests in `logic/jira_capture.rs`; 3 new handler mock tests; `inbound_event_and_activity_give_one_comment_each` (flow test, real DB); `insert_with_jobs_is_atomic`; 11 in `tests/database/jira_writeback_capture_test.rs` (the capture tests are `#[serial(jira_capture)]`, the cursor is one global row).
- Verified: `cargo fmt`; `cargo clippy --all-targets` (no warnings in touched files); `cargo test -p feature-toggle-backend` passes (918 lib, 349 integration); `./scripts/check-contract-compat.sh` passes with no baseline change.
- Manual check with a fake Jira (step 10): not run. Reason: it needs a logged-in human and a sealed credential, and the seeded users have no known password. Covered instead by the flow test (real handler, rule engine, DB and capture) and the JI-42 sender tests against a mock Jira. JI-50's api-tests fake-Jira run should do the end-to-end check.
- For JI-50: Source 1 comments dedupe on `event:<eventId>`; Source 2 on `activity:<rowId>:<integrationId>:<issue>` and `link:<integrationId>:<issue>:<featureId>:activity:<rowId>`.
