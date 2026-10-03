# JI-10: Feature external links (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in `f3941e4` |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — (run after JI-01 by project order) |
| Behavior change | Additive. New table, three endpoints, one list filter, two activity types. |
| Design | [design.md §3.2](../design.md#32-external-links-ji-10) |

## Goal

Store which Jira issues belong to a feature, so Jira automations and the UI can find the feature from an issue key and the other way round.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Feature list endpoint | `rest/feature.rs`: `list_features` (`#[get("/teams/{team_id}/features")]`), query `FeatureListQuery` in `rest/feature/types.rs` (has `tag`, `flag_kind`, ...), passed to `get_features_with_offset_filtered` |
| Feature lookup | `database/feature.rs`: `get_feature_by_id`, `get_feature_by_key(team_id, key)` |
| Feature write authorization | How `update_feature` authorizes users (policy `UpdateTeamResource` / `authorize_feature_update` in `logic/policy.rs`). Reuse it for link writes. |
| Activity log | `database/activity_log.rs`: `CreateActivityLog`, `create_activity`, `create_activity_tx`; activity type constants in `utils/activity_logger.rs` (`activity_types`) |
| Sub-resource pattern to copy | `rest/criteria.rs` + its logic/repository, or feature versions (`list_feature_versions`) |
| Repository pattern | `#[automock]` trait + `*RepositoryTx`, factory fn, registered as `web::Data<Box<dyn ...>>` in `lib.rs::run` |
| OpenAPI | `rest/mod.rs`: `paths(...)` and `components(schemas(...))` in `ApiDoc` |
| Feature archive | Features are archived (`lifecycle_stage = 'archived'`), rarely deleted. Links stay on archived features. |

## Changes

1. **Migration** `migrations/20261004000000_feature_external_links.sql` (or later): the table and index in design §3.2.
2. **Repository** `database/external_link.rs`: `ExternalLinkRepository` (`list_for_feature`, `create`, `delete(feature_id, link_id) -> bool`, `feature_ids_for_key(team_id, system, key)`), with `#[automock]`, plus a `_tx` variant for create/delete if you write the activity in the same transaction. Entity `ExternalLinkRow` in `database/entity.rs`. Map unique violation to `Error::Conflict` (or the existing conflict variant).
3. **Logic** `logic/external_link.rs` (+ `external_link_tx.rs` if the pattern needs it):
   - `normalize_jira_key(input) -> Result<String, Error>`: trim, upper-case, match `^[A-Z][A-Z0-9_]+-[1-9][0-9]*$`.
   - `validate_url(input) -> Result<Option<String>, Error>`: blank → `None`; `http`/`https` only; at most 2048 chars.
   - Create and delete write activity rows `external_link_added` / `external_link_removed` with metadata `{feature_id, feature_key, team_id, system, external_key, url}` in the same transaction.
4. **REST** `rest/external_link.rs`:
   - `GET /features/{id}/external-links` → `ExternalLinksResponse { items: Vec<ExternalLinkResponse> }`, `ExternalLinkResponse { id, featureId, system, externalKey, url, createdBy, createdAt }`.
   - `POST /features/{id}/external-links` body `CreateExternalLinkRequest { system, externalKey, url? }` → 201; 400 invalid; 404 feature; 409 duplicate.
   - `DELETE /features/{id}/external-links/{link_id}` → 204; 404 when the link is not on that feature.
   - Users: same authorization as feature update. System clients: allowed by `flag:write` through `JwtGuard` (path has `features`); team checked by `RequestScopeResolver`.
   - Register in `configure` (static paths before `/features/{feature_id}` conflicts, see `rest/feature.rs::configure`) and in `ApiDoc`.
5. **List filter**: `FeatureListQuery.external_key: Option<String>` (`externalKey`). Normalize with `normalize_jira_key` (400 on bad key), then filter `features.id IN (SELECT feature_id FROM feature_external_links WHERE system = 'jira' AND external_key = $n)`. Follow how the `tag` filter is built.
6. Contracts: export and update the baseline.

## Tests (write first)

- Unit: `normalize_jira_key` table test (`proj-123` → `PROJ-123`, ` ABC_1-9 ` ok, `PROJ-0`, `123-4`, `PROJ`, `PROJ-12a` rejected); `validate_url` table test.
- REST handler tests with mocks: create 201, duplicate 409, bad key 400, delete 204 / 404, list.
- DB integration test in `tests/database/` (wire through `tests/database/mod.rs`): create, unique constraint, cascade on feature delete, `feature_ids_for_key` is team-scoped. Use seed UUIDs from `init.sql`.
- List filter: `externalKey` returns only linked features of the team.

## Done when

- Endpoints work for a user and for a `flag:write` system client of the same team; a system client of another team gets 403.
- Activity rows appear on add and remove.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend`, `./scripts/check-contract-compat.sh` pass.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

### 2026-10-03: done in `f3941e4`

**What changed**

- Migration `20261004000000_feature_external_links.sql`: table and index as in design §3.2, plus a named unique constraint `feature_external_links_unique` and `id DEFAULT gen_random_uuid()`.
- `database/external_link.rs`: `ExternalLinkRepository` (`#[automock]`: `feature_scope`, `list_for_feature`, `create`, `delete -> bool`, `feature_ids_for_key`) and `ExternalLinkRepositoryTx` (`feature_scope_tx`, `create_tx`, `delete_tx -> Option<ExternalLinkRow>`). Factories `external_link_repository(pool)` and `external_link_repository_tx(pool)`. `FeatureScope { team_id, key }` is the feature lookup used for 404 and activity metadata. A unique violation maps to `Error::RecordAlreadyExists` (409). Entity `ExternalLinkRow` in `database/entity.rs`.
- `logic/external_link.rs`: `normalize_jira_key`, `validate_url`, `validate_new_link(system, key, url) -> NewExternalLink` (system `jira`, any case). `SYSTEM_JIRA`, `MAX_URL_LENGTH`.
- `logic/external_link_tx.rs`: `create_external_link_in_tx` and `delete_external_link_in_tx`. Each writes the activity row in the same transaction: `entity_type = 'feature'`, `entity_id = feature_id`, metadata `{feature_id, feature_key, team_id, link_id, system, external_key, url}`. `link_id` was added to the metadata listed in the task so the removed row identifies the link.
- `rest/external_link.rs`: the three endpoints, registered in `rest::configure` and `ApiDoc`. The repository is registered in `lib.rs::run`.
- Authorization (`authorize_link_write`): users go through `policy::authorize_feature_update` (system admin, or `Team Admin` member of the feature's team). System clients are detected with `policy_actor_for_request` and allowed when the token's team is the feature's team (`JwtGuard` already checked `flag:write` and the team). `policy_actor_for_request` and `rest_error_from_policy` in `rest/operational_safety.rs` are now `pub(crate)`.
- List filter: `FeatureListQuery.external_key` (`externalKey`), normalized with `normalize_jira_key` (400 on a bad key). `external_key: Option<String>` was added after `flag_kind` to `get_features_with_offset_filtered` (repository and logic traits), `get_features_windowed`, `count_filtered_features` and `push_feature_filters`. Other callers pass `None`.
- Contract baseline updated.

**Verified**

- `cargo fmt`; `cargo clippy --all-targets`: no warnings in new code (existing warnings elsewhere unchanged).
- `cargo test -p feature-toggle-backend --no-fail-fast` on `feture_toggle_test`: all pass (lib 775, integration 296, grpc 25, others).
- `./scripts/check-contract-compat.sh`: pass.
- Tests: unit (`logic::external_link`), REST (`rest::external_link`, `rest::feature::tests::list_features_*external_key*`), DB (`tests/database/external_link_test.rs`, including cascade, team scoping, tx rollback, activity rows, list filter). The filter tests were checked to fail with the filter disabled.
- End to end against a local backend on `:18180` through the real `JwtGuard`: user create 201 / duplicate 409 / bad key 400 / delete 204 then 404; `flag:write` client of the team create 201 and delete 204; `flag:write` client of another team 403 on create, delete and list; `admin:read`-only client 403 on create; `?externalKey=proj-77` returns the linked feature; `?externalKey=bad` 400; four activity rows written.

**For later tasks**

- A system client with only `flag:write` gets 403 on `GET /features/{id}/external-links`: `JwtGuard` requires `admin:read` or `evaluate` for GET on `features` paths (existing rule). A Jira integration token that reads links needs `flag:write` plus `admin:read`.
- No `FeatureUpdate` broadcast: links do not change evaluation.
