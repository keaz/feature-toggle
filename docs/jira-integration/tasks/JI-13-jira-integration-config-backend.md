# JI-13: Jira integration configuration (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-10 |
| Behavior change | Additive. New tables and admin API. Nothing acts on Jira events yet (JI-15). |
| Design | [design.md §3.7](../design.md#37-jira-integration-and-status-rules-ji-13) |

## Goal

Let a team admin configure, in FluxGate, how Jira statuses map to rollout actions: which Jira field names the environment, which environments trust Jira as approver, and the status rules. Rules are data, so teams can change them without code or Jira changes (decision J10).

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Team-admin authorization | `logic/policy.rs`: `evaluate_team_admin_or_admin` (used for `PolicyAction::ManageSystemClients`), `route_policy_for_request` |
| Shadow user pattern | `database/system_client.rs`: `create_system_client` inserts a `users` row (`auth_source = 'system'`, password `SYSTEM_CLIENT_NO_LOGIN`, not admin) and role rows |
| Secret handling | `middleware/jwt_guard.rs`: `hash_token` (SHA-256). Random bytes: see how system-client or SSO login codes generate them (`database/sso_login_code.rs`, `logic/sso_*`) |
| Environments | `database/environment.rs` (`get_environments`) |
| Feature links | `database/external_link.rs` (JI-10) |
| Settings API example | `rest/ai.rs` team AI settings; SSO provider CRUD in `rest/sso*.rs` |
| Activity log | `database/activity_log.rs` |

## Changes

1. **Migration** `<timestamp>_jira_integrations.sql`: `jira_integrations` and `jira_status_rules` exactly as design §3.7.
2. **Repository** `database/jira_integration.rs` (`#[automock]`): integration CRUD, `get(id)`, `list_for_team(team_id)`, `replace_rules(integration_id, rules)` (one transaction, delete then insert, keeps `position` order), `list_rules(integration_id)`, `set_secret_hash(id, hash)`.
3. **Logic** `logic/jira_integration.rs`:
   - Create: generate the secret (32 random bytes, base64url, no padding), store its SHA-256, create the shadow user (`username = jira-integration-<id>`, `auth_source = 'system'`, no password login, not admin, `Requester` role only, **no** `Approver` role), set `actor_user_id`. Return the secret once.
   - Rotate secret: new secret, returned once.
   - Update: name, `jira_base_url` (http/https), `environment_field` (`labels` or `^customfield_\d+$` or a system field name made of `[a-z_]`), `environment_aliases` (map of non-empty string → existing environment name of the team), `jira_approved_environment_ids` (must belong to the team), `feature_key_field` (same format as `environment_field`, optional), `enabled`.
   - Rules: `jira_status` 1–100 chars, trimmed; `action ∈ {request, approve, deploy, rollback}`; `environment_ids` belong to the team; duplicates (same status ignoring case + action) rejected with 400.
   - Delete: removes rules, events (later JI-15) and disables the shadow user (`enabled = false`); keep the user row for the audit trail.
   - Every create, update, rotate, rule change and delete writes an activity row (`jira_integration_*`), never containing the secret.
4. **REST** `rest/jira_integration.rs`, tag `Jira`:
   - `GET /teams/{team_id}/jira-integrations`, `POST /teams/{team_id}/jira-integrations` (201 with `secret`)
   - `GET /jira-integrations/{id}`, `PATCH /jira-integrations/{id}`, `DELETE /jira-integrations/{id}`
   - `POST /jira-integrations/{id}/rotate-secret` (200 with `secret`)
   - `GET /jira-integrations/{id}/rules`, `PUT /jira-integrations/{id}/rules` (whole list)
   - Response DTO never contains `secret_hash`.
   - Authorization: team admin of the team or system admin; system clients denied. Add routes to `route_policy_for_request` (new `PolicyAction::ManageJiraIntegrations` evaluated with `evaluate_team_admin_or_admin`), and teach `RequestScopeResolver`/`resolve_team_id_for_path` the `jira-integrations/{id}` path.
   - Register in `ApiDoc`; update the contract baseline.

## Tests (write first)

- Validation table tests (field names, aliases, statuses, duplicate rules, foreign environments).
- Logic with mocks: create returns a secret whose SHA-256 is stored; rotate changes the hash; the shadow user has no `Approver` role.
- DB integration test: create, replace rules keeps order, delete disables the shadow user, cascade.
- REST: team admin allowed, plain user 403, system client 403 (policy denied), other team's admin 403; `secret` only on create and rotate.
- `approver_qualifies_sql` never counts the integration's shadow user (no `Approver` role); assert in the DB test.

## Done when

- A team admin can create an integration, set field, aliases, Jira-approved environments and rules, and rotate the secret.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend`, `./scripts/check-contract-compat.sh` pass.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

(empty)
