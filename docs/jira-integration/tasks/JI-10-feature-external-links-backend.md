# JI-10: Feature external links (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
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

(empty)
