# JI-15: Inbound Jira events and the rule engine (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
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

(empty)
