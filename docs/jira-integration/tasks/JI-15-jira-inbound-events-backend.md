# JI-15: Inbound Jira events and the rule engine (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in `120b73d` |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-13, JI-14 |
| Behavior change | Additive. One public endpoint authenticated by the integration secret, one event log table and read API. |
| Design | [design.md §3.9](../design.md#39-inbound-events-and-the-rule-engine-ji-15) |

## Goal

Receive Jira issue status changes, decide from the team's rules what to do for each linked feature and each environment named on the issue, do it through JI-14, and keep a visible record of every event and its results.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Public paths in the guard | `middleware/jwt_guard.rs` (public path list; `middleware/mod.rs`: `routed_path`, `is_public_sso_path` as an example of a strict public matcher) |
| Integration config | `database/jira_integration.rs`, `logic/jira_integration.rs` (JI-13) |
| External actions | `logic/external_change.rs` (JI-14) |
| Links | `database/external_link.rs`: `feature_ids_for_key` (JI-10) |
| Schedulers (cleanup) | `scheduler/` (for example the expired session-token cleanup) |
| Body size limits | how other handlers limit JSON size (`web::JsonConfig`) |

## Changes

1. **Migration** `<timestamp>_jira_integration_events.sql`: `jira_integration_events` as design §3.9, index on `(integration_id, received_at DESC)` and on `(integration_id, delivery_hash)`.
2. **Payload parsing** `logic/jira_events.rs` (pure, no I/O):
   - `parse_event(json) -> Result<ParsedEvent, ParseError>` with `issue_key`, `target_status`, `status_changed: bool`, `jira_actor {account_id, display_name}`, `fields: serde_json::Map`, `delivery_marker` (changelog id or `fields.updated`).
   - Webhook shape: `webhookEvent` present; `status_changed` only when `changelog.items[].field == "status"`; target = that item's `toString`.
   - Automation shape: top-level `key` + `fields`, or `{issue, user}`; target = `fields.status.name`; `status_changed = true`.
   - `field_values(fields, field_name) -> Vec<String>`: string, `{value}`, `{name}`, arrays of these; `labels` is an array of strings.
3. **Rule engine** `logic/jira_rules.rs`:
   - `resolve_environments(values, aliases, team_environments) -> (Vec<EnvTarget>, Vec<String /*unknown*/>)`.
   - `matching_rules(rules, status) -> Vec<&Rule>` (enabled, status equal ignoring case and surrounding spaces, `position` order).
   - For each rule × feature × environment (skipping environments outside a rule's `environment_ids` filter): `apply_external_action(.., trusted_approval = env ∈ jira_approved_environment_ids)`. Collect `{featureKey, environment, rule, action, outcome, from, to, reason}`. One failure does not stop the others.
4. **Endpoint** `POST /api/v1/integrations/jira/{integration_id}/events` in `rest/jira_events.rs`:
   - Add an exact public-path matcher for this route (POST only, `{integration_id}` a UUID) so `JwtGuard` lets it through without a JWT; nothing else under `/integrations/` becomes public.
   - Auth: `Authorization: Bearer <secret>` or `X-FluxGate-Jira-Secret`; compare SHA-256 in constant time; unknown, disabled or wrong secret → 401 with one generic body.
   - Body limit 1 MiB. Invalid JSON or no issue key → 400 (still stored as an event with `error`).
   - Not a status change → 200 `{eventId, results: [], ignored: "no status change"}`.
   - Idempotency per design §3.9 (`delivery_hash`, 10 minutes).
   - Store the event with results; respond 200 `{eventId, results, unknownEnvironments}`.
   - Rate limit: if the repo has one for public endpoints, apply it; otherwise note it in the handoff.
5. **Read API**: `GET /api/v1/jira-integrations/{id}/events?offset&limit` (team admin or admin), newest first, `PageMeta`.
6. **Cleanup**: delete events older than 30 days in an existing daily scheduler.
7. Register in `ApiDoc`; update the contract baseline.

## Tests (write first)

- `parse_event`: fixtures for a Jira Cloud webhook (status change and non-status change), a Data Center webhook, Automation "Jira format" (bare and wrapped). Put fixtures under `tests/fixtures/jira/`.
- `field_values` and `resolve_environments` table tests: option object, multi-select, labels, alias hit, case differences, unknown value.
- `matching_rules`: case and spaces, disabled rule, order.
- Handler tests with mocks: wrong secret 401, unknown integration 401, valid event calls `apply_external_action` once per (rule, feature, env), duplicate delivery returns stored result without calling it, guard lets the route through without a JWT but not `/api/v1/integrations/jira/x/other`.
- DB integration test: end-to-end through the real logic on the seeded DB: "Ready for Release" + env QA (trusted) → `DEPLOYMENT_APPROVED`; "Done" → `DEPLOYED`; event rows stored.

## Done when

- A curl with a Jira-shaped body drives approve and deploy for a linked feature in a Jira-approved environment, and the event log shows the results.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend`, `./scripts/check-contract-compat.sh` pass.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

### 2026-10-03: done in `120b73d`

**What changed**

- Migration `20261004040000_jira_integration_events.sql`: `jira_integration_events` as design §3.9 plus `unknown_environments TEXT[]`, `unknown_features TEXT[]` and `ignored TEXT` (so a repeated delivery can return the whole stored response). Indexes `(integration_id, received_at DESC)`, `(integration_id, delivery_hash)` and `(received_at)` for the cleanup. Cascades on integration delete.
- `logic/jira_events.rs` (pure): `parse_event(&Value) -> Result<ParsedEvent, ParseError>`, `field_values(fields, name)`, `delivery_hash(issue_key, status, env_values, marker)` (SHA-256 of a JSON array; environment values sorted). `JiraActor { account_id, display_name }`: Cloud `accountId`, Data Center `key` then `name`.
- `logic/jira_rules.rs`: `resolve_environments(values, aliases, team_environments)`, `matching_rules(rules, status)`, `RuleEngine { external_change, links, features, environments }::run(integration, rules, event, status) -> EventResults { results: Vec<RuleResult>, unknown_environments, unknown_features }`. `RuleResult` is camelCase: `featureId`, `featureKey`, `environmentId`, `environment`, `ruleId`, `ruleStatus`, `action`, `outcome` (`applied`, `no_op`, `refused`, `error`), `from`, `to`, `reason`, `approvalRequestId`.
- `database/jira_event.rs`: `JiraEventRepository` (`#[automock]`): `insert`, `find_recent_delivery(integration_id, hash, since)` (skips rows with an `error`), `list(integration_id, offset, limit) -> (rows, total)`, `delete_older_than(cutoff)`. Registered as `web::Data<Box<dyn JiraEventRepository>>`. `EnvironmentRepository` is now registered as `web::Data` too.
- `rest/jira_events.rs`: `POST /api/v1/integrations/jira/{integration_id}/events` (`web::resource` with `web::PayloadConfig::new(1 MiB)`) and `GET /api/v1/jira-integrations/{id}/events?offset&limit` (`PageMeta`, newest first). Both in `ApiDoc`, tag `Jira`. Contract baseline updated.
- `middleware::is_public_jira_event_path`: POST, exactly `/api/v1/integrations/jira/{uuid}/events`. `JwtGuard` lets it through; every other `/api/v1/integrations/...` path still needs a JWT.
- `TokenCleanupScheduler::with_jira_events(repo)` deletes events older than `JIRA_EVENT_RETENTION_DAYS` (30). Wired in `lib.rs::run`.

**Endpoint behavior**

- Auth first: secret from `Authorization: Bearer` or `X-FluxGate-Jira-Secret`; integration must exist and be enabled; `subtle::ConstantTimeEq` on the SHA-256 hex. Every failure (bad id, no secret, unknown, disabled, wrong) is 401 `unknown integration or wrong secret`, and nothing is stored.
- Body over 1 MiB: 413 from actix before the handler (not stored).
- Not JSON, or no issue / bad key / Automation body without `fields.status.name`: 400, stored with `error`.
- Webhook without a `status` changelog item: 200 `{eventId, results: [], ignored: "no status change", ...}`, stored with `ignored`.
- Repeated delivery (same hash within 10 minutes, first one without `error`): 200 with the stored `eventId` and results, `duplicate: true`; nothing runs, nothing is stored.
- Otherwise: run the engine, store the event, 200 `{eventId, results, unknownEnvironments, unknownFeatures, duplicate: false}`. If features or environments cannot be read (database error), the event is stored with `error` and the response is 500.

**Decisions taken in this task**

- Feature resolution: linked features (`feature_ids_for_key`, key from `feature_scope`) first, then keys from `feature_key_field` that are not already linked (`get_feature_by_key`); keys that match nothing go to `unknownFeatures` (new response field).
- Environment resolution: alias keys compared trimmed and ignoring case; an alias pointing at an inactive or foreign environment is unknown (no fallback to the name). Otherwise the one active environment with that name; a name shared by several active environments is unknown.
- Order: rule (by `position`) × feature × environment. A rule's `environmentIds` filter skips environments outside it. A rule with an unknown action (cannot be saved) is skipped.
- `trusted_approval = environment ∈ jira_approved_environment_ids`; JI-14 refuses an untrusted `approve`.
- Idempotency is a read-then-write check, not a lock: two identical deliveries at the same instant can both run. The JI-14 state table makes the second a `no_op` (backend is single instance).
- Rate limiting: the repo has none for public endpoints, so none was added. Before exposing this route publicly, put a rate limit in front (reverse proxy or an actix middleware). Recorded as an open point.
- Cleanup runs in the hourly token cleanup scheduler (there is no daily one).

**Verified**

- `cargo fmt`; `cargo clippy --all-targets`: no warnings in changed files.
- `cargo test -p feature-toggle-backend` on `feture_toggle_test`: all pass (lib 845, integration_test 320, others). `SQLX_OFFLINE=true cargo build --all-targets` ok (no `query!` macros added). `./scripts/check-contract-compat.sh` ok.
- New tests: `logic::jira_events::tests` (8, fixtures in `tests/fixtures/jira/`: Cloud status / non-status webhook, Data Center webhook, Automation bare and wrapped), `logic::jira_rules::tests` (7), `rest::jira_events::tests` (11, mocks: 401 cases, per-target calls, duplicate, ignored, 400, 1 MiB limit, event log paging), `rest::jira_events::flow_tests` (2, real logic and DB: "Ready for Release" approves, duplicate replays, "Done" deploys, `deploy` before approval refused), `tests/database/jira_event_test.rs` (3), `middleware::tests::the_jira_event_path_is_public_for_post_with_a_uuid_only`, guard and policy route tests extended, token cleanup test extended.
- Mutation checks: removing the payload limit, the guard wiring, or the `error IS NULL` / time window filters each fails a test.
- Manual: backend on `127.0.0.1:18180` against the test DB, through `JwtGuard`: curl with a Jira webhook body, wrong secret → 401; "Ready for Release" → `DEPLOYMENT_APPROVED`; "Done" (header secret) → `DEPLOYED`, stage enabled; `GET /jira-integrations/{id}/events` lists both; `POST /integrations/jira/{id}/other` without JWT → 401.
