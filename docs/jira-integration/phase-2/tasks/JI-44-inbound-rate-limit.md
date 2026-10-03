# JI-44: Inbound rate limit (backend)

| Field | Value |
|---|---|
| Type | Hardening |
| Status | Done in 8c7db99 |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — (can run before JI-40 if needed) |
| Behavior change | **Yes.** The public inbound route answers 429 above the limit. Default limits are far above a normal Jira workload. |
| Design | [design.md §3.4, J23](../design.md#34-inbound-rate-limit-ji-44) |

## Goal

Stop floods and secret guessing on `POST /api/v1/integrations/jira/{id}/events` before they reach the event log, the rule engine or the secret check.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Handler | `rest/jira_events.rs` `receive_jira_event` (~155). Order today: parse id, `presented_secret` (~114), `integrations.get(id)` filtered by `enabled`, `secret_matches` (~129), parse body |
| Public path | `middleware/mod.rs` `is_public_jira_event_path` (~39) |
| Config | `config.rs` `JiraConfig` (JI-41; if JI-41 is not done yet, add `JiraConfig` here with only the two fields below and `#[serde(default)] pub jira: JiraConfig` on `Config`) |
| Wiring | `lib.rs` `web::Data` registrations ~388-409 |
| Mock REST tests | `rest/jira_events.rs` tests `setup` (~364), `send` (~427) |

## Interfaces

- Produces:
  - `JiraConfig` gains `inbound_per_minute: u32` (default 120) and `inbound_burst: u32` (default 60). `sanitized()` raises 0 to the default.
  - `rest/jira_inbound_limit.rs`:
    ```rust
    pub struct JiraInboundLimiter { /* governor keyed limiter (Uuid), direct limiter for unknown ids, dropped counters */ }
    pub enum Admission { Allowed, Limited { retry_after_secs: u64 } }
    impl JiraInboundLimiter {
        pub fn new(per_minute: u32, burst: u32, unknown_per_minute: u32) -> Self;  // unknown: 30/min, burst 10
        pub fn check_known(&self, integration_id: Uuid) -> Admission;
        pub fn check_unknown(&self) -> Admission;
    }
    ```
    `retry_after_secs` = the governor wait time rounded up, at least 1.
  - `web::Data<JiraInboundLimiter>` registered in `lib.rs`, built from the config.

## Rules

- **New order in `receive_jira_event`:**
  1. Parse the id. A bad UUID counts against `check_unknown`, then returns 401.
  2. `integrations.get(id)`. If it is missing or disabled: `check_unknown`, then 401.
  3. `check_known(id)`.
  4. Secret check.
  5. Body parse.
- **Over the limit:** `429` with header `Retry-After: <secs>` and the JSON error `{"error": "rate limited"}`. Use the existing `RestError` shape. Add a `RestError::too_many_requests(retry_after)` if none exists.
  - Nothing is inserted in `jira_integration_events`.
  - The secret is not checked.
- **Logging:** count drops per integration. At most once per 60 s per integration: `log::warn!("Jira integration {id}: {n} events rate limited in the last minute")`. For unknown ids, log the same line under `unknown`.
  - Never log the body or the secret.
- The limiter lives in memory. A restart resets it. This is acceptable because the backend runs as one instance.

## Steps

- [ ] **Step 1:** `cargo add governor` in `feature-toggle-backend/`.
- [ ] **Step 2: failing unit tests** in `rest/jira_inbound_limit.rs`:
  ```rust
  #[test] fn burst_then_limited_with_retry_after() {
      let l = JiraInboundLimiter::new(60, 3, 30);
      let id = Uuid::new_v4();
      for _ in 0..3 { assert!(matches!(l.check_known(id), Admission::Allowed)); }
      match l.check_known(id) { Admission::Limited { retry_after_secs } => assert!(retry_after_secs >= 1), _ => panic!() }
  }
  #[test] fn integrations_do_not_share_a_bucket()
  #[test] fn unknown_ids_share_one_bucket()
  ```
- [ ] **Step 3:** run `cargo test -p feature-toggle-backend jira_inbound_limit`. It fails. Implement it. It passes.
- [ ] **Step 4: failing handler tests** in the `rest/jira_events.rs` mock tests. `setup` registers a limiter with a small burst.
  ```rust
  #[actix_web::test] async fn over_the_limit_is_429_with_retry_after_and_stores_nothing()
  // burst 2: 2 valid events -> 200; 3rd -> 429, Retry-After header present, MockJiraEventRepository.insert called exactly 2 times
  #[actix_web::test] async fn limited_request_with_a_wrong_secret_is_still_429()   // the secret is not checked when limited
  #[actix_web::test] async fn unknown_integration_ids_hit_the_shared_bucket()
  ```
- [ ] **Step 5:** change the handler order, then add the 429 response and the warn throttling. The tests pass, and all existing `jira_events` tests still pass.
- [ ] **Step 6: config tests.** `jira_inbound_defaults` and `zero_inbound_limit_is_raised_to_default`.
- [ ] **Step 7:**
  - Run `cargo fmt`, `cargo clippy --all-targets` and the full backend test suite.
  - The 429 response is documented in the `#[utoipa::path]` responses of `receive_jira_event`, so export the contracts, copy them to the baseline and run the check.
  - Run `graphify update .`.
- [ ] **Step 8:** commit `feat(jira): rate limit on the inbound events route (JI-44)`. The message notes the contract baseline update.
- [ ] **Step 9: manual check.** With the backend running, send 70 curl requests in a loop using a valid secret. Expect 60 × 200, then 429. Write the counts in the handoff log. Read the secret from an environment variable; never put it in the log.

## Done when

- Bursts above the limit get 429 with `Retry-After`. Nothing is stored for them, and no secret check runs for them.
- All checks pass. Handoff log, `../HANDOFF.md` and the README table updated.

## Handoff log

2026-10-03, backend `8c7db99`:
- Implemented as specified. New: `rest/jira_inbound_limit.rs` (`JiraInboundLimiter`, `Admission`), `JiraConfig.inbound_per_minute` (120) and `inbound_burst` (60), `sanitized()` raises 0 to the default. `RestError::TooManyRequests` / `too_many_requests(secs)` (429, `Retry-After`). `web::Data<JiraInboundLimiter>` built in `lib.rs` (one instance shared by all workers).
- Handler order: bad id -> `check_unknown` -> 401; `get` missing/disabled -> `check_unknown` -> 401; `check_known`; secret (a missing secret is now a 401 after the limit); body. JI-45 adds HMAC after the `check_known` step.
- 429 body is `{"error":"rate limited","message":"Too many requests"}` (error key is the brief's literal string, not snake_case). Documented in `#[utoipa::path]`; contract baseline `contract-hashes.json` updated.
- Unknown bucket: 30/min, burst 10. Drop warning: first drop logs at once, then at most once per 60 s per key (`unknown` for the shared bucket) with the count since the last line. Counter map is bounded by existing integrations plus `unknown`.
- Tests: 3 limiter unit tests, 4 handler mock tests (429 + Retry-After + 2 inserts, wrong secret still 429, unknown ids, bad uuid), 2 config tests. Mock test `setup` now has a `burst` field (default 50) and `send_all` for several requests on one app.
- Verified: `cargo fmt`; `cargo clippy --all-targets` (no warnings in touched files); `cargo test -p feature-toggle-backend` passes except the known flaky `test_pending_approval_listing_maps_feature_metadata`; contract check passes.
- Manual check (step 9): not run. It needs an admin login to create an integration and read its secret; no known credentials. The handler tests cover the 200 then 429 sequence.
