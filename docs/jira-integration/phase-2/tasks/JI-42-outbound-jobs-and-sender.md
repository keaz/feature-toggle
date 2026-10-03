# JI-42: Outbound jobs, sender, retry and pause (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-41 |
| Behavior change | Additive. A new scheduler sends queued jobs to Jira. Nothing enqueues jobs until JI-43, except tests. |
| Design | [design.md §3.2, J22](../design.md#32-outbound-jobs-and-the-sender-ji-42) |

## Goal

A durable outbox for Jira write-back, and a worker that delivers it with retries, ordering per issue and a pause on bad credentials.

## Current code (verify first)

| Piece | Where |
|---|---|
| Jira client and config | `logic/jira_client.rs`, `config.rs` `JiraConfig`, `Config::jira_ui_base_url` (JI-41) |
| Scheduler pattern | `scheduler/token_cleanup.rs` (struct + `new` + `run` loop with `tokio::time::interval`), registered in `scheduler/mod.rs`, spawned in `lib.rs` ~316-327 |
| Retention hook | `TokenCleanupScheduler::with_jira_events` and `JIRA_EVENT_RETENTION_DAYS` |
| Stage statuses | `FeatureRepository::get_feature_stages(feature_id)` (no ORDER BY: sort by `order_index`); environment names via `logic::get_environment_map` (`logic/mod.rs` ~128), as `rest/feature.rs` `load_stage_data` (~341-372) does |
| Paged list pattern | `rest/jira_events.rs` `list_jira_events` (`offset`, `limit` default 50, max 200) |
| Write-back off | `update_jira_writeback_in_tx` (JI-41) |

## Interfaces

- Consumes: `JiraClient`, `JiraEdition`, `JiraResponse`, `JiraTransportError`, `client_for` (JI-41), `JiraConfig`, `Config::jira_ui_base_url`.
- Produces:
  - Migration `<ts>_jira_outbound_jobs.sql`: the table and three indexes, copied verbatim from design §3.2.
  - `database/jira_outbound_job.rs`:
    ```rust
    pub struct NewOutboundJob { pub integration_id: Uuid, pub issue_key: String, pub feature_id: Option<Uuid>,
                                pub kind: OutboundKind, pub payload: serde_json::Value, pub dedupe_key: String }
    pub enum OutboundKind { Comment, RemoteLink, RemoteLinkDelete }   // as_str: "comment" | "remote_link" | "remote_link_delete"
    #[derive(sqlx::FromRow)] pub struct OutboundJobRow { pub id: Uuid, pub integration_id: Uuid, pub issue_key: String, pub feature_id: Option<Uuid>,
        pub kind: String, pub payload: serde_json::Value, pub dedupe_key: String, pub status: String, pub attempts: i32,
        pub next_attempt_at: DateTime<Utc>, pub last_error: Option<String>, pub created_at: DateTime<Utc>, pub sent_at: Option<DateTime<Utc>> }  // in database/entity.rs
    #[automock] #[async_trait] pub trait JiraOutboundJobRepository {
        async fn enqueue(&self, job: NewOutboundJob) -> Result<bool, Error>;          // false on conflict
        async fn claim_due(&self, limit: i64, lease: chrono::Duration) -> Result<Vec<OutboundJobRow>, Error>;
        async fn mark_sent(&self, id: Uuid, note: Option<String>) -> Result<(), Error>;
        async fn mark_retry(&self, id: Uuid, attempts: i32, next_attempt_at: DateTime<Utc>, error: String) -> Result<(), Error>;
        async fn mark_dead(&self, id: Uuid, attempts: i32, error: String) -> Result<(), Error>;
        async fn release(&self, ids: &[Uuid], next_attempt_at: DateTime<Utc>) -> Result<(), Error>;
        async fn list(&self, integration_id: Uuid, status: Option<String>, offset: i64, limit: i64) -> Result<(Vec<OutboundJobRow>, i64), Error>;
        async fn retry_dead(&self, integration_id: Uuid, id: Uuid) -> Result<Option<OutboundJobRow>, Error>;  // None when not dead or not found
        async fn delete_finished_before(&self, sent_before: DateTime<Utc>, dead_before: DateTime<Utc>) -> Result<u64, Error>;
    }
    pub trait JiraOutboundJobRepositoryTx {
        async fn enqueue_tx(&self, conn: &mut PgConnection, job: NewOutboundJob) -> Result<bool, Error>;
        async fn cancel_pending_tx(&self, conn: &mut PgConnection, integration_id: Uuid, reason: &str) -> Result<u64, Error>;
    }
    pub fn jira_outbound_job_repository(pool: PgPool) -> Box<dyn JiraOutboundJobRepository>;
    pub fn jira_outbound_job_repository_tx(pool: PgPool) -> JiraOutboundJobRepositoryImpl;
    ```
    - `enqueue` is `INSERT ... ON CONFLICT DO NOTHING`, with no target, so both unique constraints apply.
    - `claim_due` runs in one statement: `UPDATE ... SET next_attempt_at = now() + lease WHERE id IN (SELECT id ... WHERE status = 'pending' AND next_attempt_at <= now() AND <integration active> ORDER BY created_at LIMIT $1 FOR UPDATE SKIP LOCKED) RETURNING *`.
      - "Integration active": `enabled`, `writeback_enabled`, and `writeback_paused_reason IS NULL`.
      - The lease (5 min) stops a crashed tick from losing jobs.
  - `logic/jira_writeback.rs` (pure, unit-tested):
    ```rust
    pub const MAX_ATTEMPTS: i32 = 6;
    pub fn retry_delay(attempts_done: i32) -> Option<chrono::Duration>;  // 1->30s, 2->2m, 3->10m, 4->1h, 5->6h, >=6 -> None
    pub enum Outcome { Sent, Retry { delay: chrono::Duration }, Dead, PauseIntegration }
    pub fn classify(kind: OutboundKind, result: &Result<JiraResponse, JiraTransportError>, attempts_done: i32) -> Outcome;
    pub fn comment_body(edition: JiraEdition, lines: &[String]) -> serde_json::Value;
    pub fn remote_link_global_id(feature_id: Uuid) -> String;               // "fluxgate:feature:<id>"
    pub fn remote_link_title(feature_key: &str, stages: &[(String, String)]) -> String;  // (environment name, status) in pipeline order; at most 255 chars
    pub fn remote_link_body(feature_id: Uuid, title: &str, url: &str) -> serde_json::Value;
    pub fn error_note(resp: &JiraResponse) -> String;                       // "<status>: <body_excerpt>"
    ```
  - `JiraClient` gains `add_comment(issue, body)`, `put_remote_link(issue, body)` and `delete_remote_link(issue, global_id)`. All return `Result<JiraResponse, JiraTransportError>`.
  - `scheduler/jira_writeback_sender.rs`: `JiraWritebackSender::new(pool, ui_base_url: Option<String>, interval)`, `run(self)` loop, `run_once(&self) -> SendCounts { sent, retried, dead, paused }`. Spawned in `lib.rs` with a 5 s interval.
  - REST (tag `Jira`, `ApiDoc`):
    - `GET /api/v1/jira-integrations/{id}/outbound-jobs?status=&offset=&limit=` returns `JiraOutboundJobsResponse { items, total }`.
    - `POST /api/v1/jira-integrations/{id}/outbound-jobs/{jobId}/retry` returns the job, or 409 `job is not dead`.
    - `JiraOutboundJobResponse { id, issueKey, featureId, kind, status, attempts, nextAttemptAt, lastError, createdAt, sentAt }`. The payload is not returned.

## Classification table (implement exactly)

| Result | Outcome |
|---|---|
| 2xx | `Sent` |
| 404 and kind `remote_link_delete` | `Sent` |
| 401, 403 | `PauseIntegration` (job `dead`, integration `writeback_paused_reason = "Jira returned <status> at <RFC 3339 now>"`) |
| 429 | `Retry { max(retry_delay, Retry-After) }`, or `Dead` when out of attempts |
| 5xx, transport error | `Retry { retry_delay }`, or `Dead` when out of attempts |
| 3xx, 404 (other kinds), other 4xx | `Dead` |

## Sender rules

- **Batch:** claim 50 jobs and group them by `(integration_id, issue_key)`. Within a group, send in `created_at` order. If a job gets `Retry`, release the rest of its group to the same `next_attempt_at`, so later comments never overtake it.
- **Client:** decrypt the credential once per integration per tick (`client_for`). When decryption fails (key changed or missing), pause the integration with `"credential cannot be decrypted"`, and release its jobs.
- **`comment`:** `client.add_comment(issue, comment_body(edition, payload.lines))`.
- **`remote_link`:**
  - `feature_id` is NULL or the feature is gone: `mark_sent` with note `"skipped: feature deleted"`.
  - The feature has no `jira` link to `issue_key` any more: `mark_sent` with note `"skipped: issue no longer linked"`.
  - No UI base URL: `mark_dead("ui base URL is not configured")`.
  - Otherwise build the title from the current stages, then call `put_remote_link`.
- **`remote_link_delete`:** `delete_remote_link(issue, remote_link_global_id(feature_id))`. It needs `feature_id` from the payload (`payload.featureId`), because the column may be NULL after a delete.
- **`last_error`:** always `error_note(..)` or a fixed message. Never format a `reqwest::Error` with its URL, and never format a request.
- **Logs:** `log::warn!` with the job id, integration id, issue key and status only.
- **Write-back off:** in `update_jira_writeback_in_tx` (JI-41), when `enabled` goes from true to false, call `cancel_pending_tx(conn, id, "write-back disabled")`. That sets the pending jobs to `dead`.
- **Retention:** `TokenCleanupScheduler::with_jira_outbound_jobs(repo)`. Delete `sent` jobs older than 30 days and `dead` jobs older than 90 days.

## Steps

- [ ] **Step 1: failing pure tests** in `logic/jira_writeback.rs`:
  ```rust
  #[test] fn retry_delays_follow_the_schedule() {
      let d: Vec<_> = (1..=6).map(retry_delay).collect();
      assert_eq!(d, vec![Some(Duration::seconds(30)), Some(Duration::minutes(2)), Some(Duration::minutes(10)),
                         Some(Duration::hours(1)), Some(Duration::hours(6)), None]);
  }
  #[test] fn classify_table()   // one assert per row of the classification table, incl. 429 with Retry-After 3600 at attempt 1 -> Retry 3600s
  #[test] fn cloud_comment_is_adf_with_one_paragraph_per_line()
  // comment_body(Cloud, ["a","b"]) == json!({"body":{"type":"doc","version":1,"content":[
  //   {"type":"paragraph","content":[{"type":"text","text":"a"}]},{"type":"paragraph","content":[{"type":"text","text":"b"}]}]}})
  #[test] fn data_center_comment_is_plain_text() // == json!({"body":"a\nb"})
  #[test] fn remote_link_title_lists_stages_in_order_and_fits_255()
  // ("flag-x", [("qa","DEPLOYED"),("prod","DEPLOYMENT_REQUESTED")]) == "FluxGate: flag-x · qa DEPLOYED · prod DEPLOYMENT_REQUESTED"
  // 40 stages -> len <= 255 and ends with "…"
  #[test] fn remote_link_body_uses_the_global_id()
  // == json!({"globalId":"fluxgate:feature:<id>","application":{"type":"io.fluxgate","name":"FluxGate"},
  //           "object":{"url":url,"title":title}})
  ```
- [ ] **Step 2:** run `cargo test -p feature-toggle-backend jira_writeback`. Expected: compile failure. Implement. Pass.
- [ ] **Step 3: migration and repository.** Add a DB test file `tests/database/jira_outbound_job_test.rs`:
  ```rust
  #[tokio::test] async fn enqueue_is_idempotent_on_dedupe_key()
  #[tokio::test] async fn only_one_pending_remote_link_per_issue_and_feature()
  #[tokio::test] async fn claim_due_skips_paused_and_disabled_integrations()
  #[tokio::test] async fn claim_due_leases_jobs_so_a_second_claim_gets_none()
  #[tokio::test] async fn retry_dead_resets_attempts_and_rejects_non_dead()
  #[tokio::test] async fn cancel_pending_marks_jobs_dead()
  #[tokio::test] async fn delete_finished_before_keeps_pending()
  ```
  Write the tests, check they fail, implement, then check they pass.
- [ ] **Step 4: client calls.** Add `add_comment`, `put_remote_link` and `delete_remote_link` to `JiraClient`. Test each with wiremock: method, path, body and auth header. `delete_remote_link` sends `?globalId=<url-encoded>`.
- [ ] **Step 5: failing sender tests.** In `scheduler/jira_writeback_sender.rs`, use the DB and wiremock as Jira, and a team, feature and integration created in the test:
  ```rust
  #[tokio::test] async fn comment_job_is_sent_and_marked_sent()
  #[tokio::test] async fn server_error_schedules_a_retry_with_backoff()                 // 500 -> attempts 1, next_attempt_at ~ now+30s
  #[tokio::test] async fn rate_limited_job_honours_retry_after()                         // 429 Retry-After: 120 -> ~ now+120s
  #[tokio::test] async fn unauthorized_pauses_the_integration_and_keeps_other_jobs_pending()  // REVIEW FOCUS 3
  #[tokio::test] async fn redirect_is_not_followed()                                     // REVIEW FOCUS 4: 302 -> dead, no call to Location
  #[tokio::test] async fn last_error_never_contains_the_credential()                      // REVIEW FOCUS 4: Jira 400 body echoes "Authorization: Basic <b64>" -> last_error has no token and no b64 value
  #[tokio::test] async fn remote_link_for_missing_feature_is_dropped()                   // REVIEW FOCUS 5: feature deleted -> sent, note "skipped: feature deleted", no HTTP call
  #[tokio::test] async fn remote_link_title_reflects_current_stage_status()
  #[tokio::test] async fn a_retry_holds_back_later_jobs_of_the_same_issue()
  #[tokio::test] async fn sixth_failure_is_dead()
  ```
  For the credential echo test, `last_error` stores the first 300 characters of the body. If the body echoes the token, it would leak. So `error_note` must also remove the decrypted credential and its Basic base64 form from the excerpt (`excerpt.replace(secret, "***")`). Add a unit test for that in `jira_writeback.rs` too.
- [ ] **Step 6:** implement the sender and spawn it in `lib.rs`. The tests pass.
- [ ] **Step 7: REST.** Write the tests first: list paging and status filter, retry 200 then 409, plain user 403. Then implement and register them.
- [ ] **Step 8: write-back off and retention.**
  - Add `writeback_off_cancels_pending_jobs` (a REST test) and a cleanup scheduler test.
  - Implement both.
- [ ] **Step 9: checks.**
  - Run `cargo fmt`, `cargo clippy --all-targets` and the full backend test suite.
  - Export the contracts, copy them to the baseline, then run the contract check.
  - Run `graphify update .`.
- [ ] **Step 10:** commit `feat(jira): outbound job queue and write-back sender (JI-42)`. The message notes the contract baseline update.

## Done when

- Jobs inserted by hand (SQL or test) reach a wiremock Jira, in order per issue.
- Retries follow the schedule. 401 pauses the integration. Nothing secret reaches `last_error` or logs.
- All checks pass. Handoff log, `../HANDOFF.md` and the README table updated.

## Handoff log

(empty)
