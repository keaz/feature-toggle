# JI-12: By-key read and request-change endpoints (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-11 |
| Behavior change | Additive. Two endpoints. **Optional** since the 2026-10-03 replan: the inbound rules (JI-15) cover the main flow. Build it for status read-back and per-transition Automation rules, or skip it and record that in `HANDOFF.md`. |
| Design | [design.md §3.4](../design.md#34-by-key-endpoints-ji-12) |

## Goal

Jira knows a feature key and an environment name, not FluxGate UUIDs. Let an automation read a feature's stage status and request a stage change in one call each.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Stage request handler | `rest/feature.rs`: `request_stage_change` (after JI-11 it passes `StageChangeMeta`) |
| Get feature handler | `rest/feature.rs`: `get_feature` (`#[get("/features/{id}")]`), `build_feature_response(feature, feature_repo, env_logic, ...)` |
| Feature by key | `database/feature.rs`: `get_feature_by_key(team_id, key)` (check exact vs case-insensitive; `get_features_by_key_ignore_case` also exists) |
| Environments | `database/environment.rs`: `get_environments(team_id, name_filter, active_filter)` (check whether `name_filter` is a substring/ILIKE match; you need an exact, case-insensitive match) |
| Stages | `database/feature.rs`: `get_feature_stages(feature_id)` (each stage has `environment_id`) |
| Team scoping | `logic/authorization.rs`: `parse_scoped_resource` resolves `/teams/{id}/...` to the team; `JwtGuard` enforces it for system clients |
| Scope mapping | `middleware/jwt_guard.rs`: `system_client_scope_allowed` (paths with `features`: GET needs `admin:read`/`evaluate`, POST needs `flag:write`) |
| Route order | `rest/feature.rs::configure` registers static paths before `/features/{feature_id}` |

## Changes

1. **Refactor first (no behavior change):** move the body of `request_stage_change` after stage id parsing into `async fn perform_stage_change(stage_uuid, jwt_user, body, deps...) -> Result<FeatureResponse, RestError>`. The existing handler calls it. Existing tests must pass unchanged.
2. **Resolver** (logic or a small helper in `rest/feature.rs`): `resolve_stage_by_key(team_id, feature_key, env_name) -> Result<(Feature, Uuid /*stage*/), RestError>`:
   - feature: exact key in the team (archived features resolve too; the stage path decides what is allowed);
   - environment: active, `lower(name) = lower(env_name)`, in the team. Add a repository fn `get_environment_by_name(team_id, name)` if no exact lookup exists;
   - stage: the feature's stage whose `environment_id` matches.
   - 404 with `feature_not_found`, `environment_not_found` or `stage_not_found` in the error message.
3. **Endpoints** (register in `configure` and `ApiDoc`, tag `Features`):
   - `GET /teams/{team_id}/features/by-key/{key}` → `FeatureResponse` exactly as `get_feature` builds it.
   - `POST /teams/{team_id}/features/by-key/{key}/environments/{env_name}/request-change`, body `StageChangeRequestBody` → resolve, then `perform_stage_change`. Same role, freeze, approval and broadcast behavior as the stage route.
   - URL-decode `key` and `env_name` (Actix path extraction does this; add a test with a space in the environment name).
4. Contracts: export and update the baseline.

## Tests (write first)

- Handler tests with mocks: GET by key returns the feature; unknown key 404 `feature_not_found`; unknown environment 404 `environment_not_found`; environment without a stage for that feature 404 `stage_not_found`.
- POST by key reaches the same logic call as the stage route with the resolved stage id and the `StageChangeMeta` from the body.
- Environment name match ignores case and does not match substrings (`prod` must not match `production`).
- Existing `request_stage_change` tests pass unchanged after the refactor.

## Done when

- A `flag:write` system client of the team can request `DEPLOYMENT_REQUESTED` by key and read the result by key; another team's client gets 403.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend`, `./scripts/check-contract-compat.sh` pass.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

(empty)
