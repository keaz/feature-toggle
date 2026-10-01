# AI-30: Flag kind (column, classification, suggestions, backfill, filter)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Not started |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-01 |
| Behavior change | Additive: new feature fields, a new list filter, two new endpoints. Stale behavior does not change (that is AI-31). |
| Design | [design.md §4.4, §5.3](../design.md#53-flag-kind-ai-30-ai-31-ai-32) |

## Goal

Store a flag kind (`release | experiment | ops | permission | config`) on each feature. AI fills it when empty; a user choice wins. Also provide create-form suggestions (kind and tags), an admin backfill, and a list filter.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| What | Where |
|---|---|
| Domain model (32 fields; built with struct literals in many tests) | `model.rs`: `pub struct Feature`; `CreateFeatureInput`, `UpdateFeatureInput` (uses `Option<Option<_>>`) |
| Entity and SELECT column list | `database/entity.rs`: `pub struct Feature`; `database/feature.rs`: `FEATURE_SELECT` |
| Create and update repository | `database/feature.rs`: `CreateFeature`, `UpdateFeature`, `create_feature_tx`, `update_feature_tx` |
| Create and update logic | `logic/feature_tx.rs`: `create_feature_in_tx`, `update_feature_in_tx` |
| Handlers | `rest/feature.rs`: `create_feature` (POST `/teams/{team_id}/features`, no broadcast), `update_feature` (PATCH `/features/{id}`, full-body replace, broadcasts) |
| DTOs | `rest/feature/types.rs`: `CreateFeatureRequest`, `UpdateFeatureRequest`, `FeatureListQuery`; REST mapping of `Feature` in `rest/feature.rs` (where `is_stale` and `stale_reasons` are mapped) |
| Entity to model mapping | `logic/feature.rs`: `map_entity_to_api_feature` |
| List filters | `database/feature.rs`: `push_feature_filters` (tag uses array overlap, owner uses ILIKE) |

## Changes

1. **Migration**: the three `features` columns in design §4.4.
2. **Model and plumbing**:
   - Add `enum FlagKind { Release, Experiment, Ops, Permission, Config }` (serde `snake_case`) and `enum FlagKindSource { Ai, User }`.
   - Add `flag_kind: Option<FlagKind>`, `flag_kind_source: Option<FlagKindSource>`, `flag_kind_confidence: Option<f32>` to the entity, `FEATURE_SELECT`, the row mapping, `model::Feature`, and the REST feature DTO.
   - Fix every struct literal that no longer compiles (`cargo test --no-run` lists them).
   - No broadcast or proto change.
3. **User choice** (design §5.3, "User choice"):
   - `CreateFeatureRequest.flag_kind: Option<FlagKind>`.
   - `UpdateFeatureRequest.flag_kind: Option<Option<FlagKind>>`, so absent, null, and a value are all distinguishable. Use `#[serde(default, deserialize_with = ...)]` the way the existing `Option<Option<_>>` fields do, or the `serde_with` double-option pattern already in the crate.
   - When the value is present and differs from the stored one, set the kind, `source = user`, and `confidence = NULL`. When it is absent or equal, change nothing.
4. **Handler** `src/judgment/flag_kind.rs`:
   - `fn content_hash(key, description, purpose, tags) -> String` (SHA-256 over a canonical JSON).
   - `build`: one Choice with the 6 options (5 kinds plus `unknown`) and the exact criteria in design §5.3. State is `{feature: {key, description, purpose, tags, feature_type}}`.
   - `derive`: returns `{kind | null, confidence, probabilities}`. The kind is `null` when the answer is `unknown` or the confidence is below 0.5.
   - `apply`:
     - Reload the feature.
     - Skip when `flag_kind_source = user`.
     - Skip when `content_hash(current)` differs from the input's hash (input carries `content_hash`).
     - Skip when the kind is null.
     - Otherwise run the guarded UPDATE in design §5.3.
5. **Trigger**:
   - After `create_feature` and `update_feature` commit, when the service exists, `team_enabled(flag_kind)`, and source is not `user`, compute `content_hash`.
   - Submit only when no judgment exists for the feature or its input `content_hash` differs.
   - Never block the response.
6. **Suggestions endpoint** `POST /api/v1/teams/{team_id}/ai/feature-suggestions`, in `rest/ai.rs`:
   - Body `{key, description?, purpose?, tags?}`. Response `{available, kind: {value, probabilities, confidence} | null, tags: [{tag, probability}]}`.
   - Tag candidates: the team's distinct tags by use count, excluding tags already in the body, the top 50. Use `SELECT tag, COUNT(*) FROM features, unnest(tags) AS tag WHERE team_id = $1 AND archived_at IS NULL GROUP BY tag ORDER BY 2 DESC LIMIT 50` and adapt it to the schema.
   - One request: the kind Choice plus one Noul per tag (`t0..tN`, instructions object as in design §5.3). Return at most 5 tags with probability at least 0.6.
   - On any failure, or with the setting off, return `{available: false}`.
7. **Backfill** `POST /api/v1/teams/{team_id}/ai/flag-kind/backfill`, admin only:
   - Submit for up to 500 features with `flag_kind IS NULL AND archived_at IS NULL` and no `done` judgment for the current hash.
   - Return `{queued: n}`.
8. **List filter**:
   - `FeatureListQuery.flag_kind: Option<String>` accepts the 5 kinds or `unclassified`; anything else returns 400.
   - In `push_feature_filters`, add `AND f.flag_kind = $n` or `AND f.flag_kind IS NULL`.
   - Thread the value through the logic and repository signatures (`get_features_with_offset_filtered`, `get_features_windowed`).
9. Register endpoints and schemas in `ApiDoc` and update the contract baseline.

## Tests

- Update semantics: absent leaves the value unchanged; the same value keeps `source = ai`; a different value sets `source = user`; null clears it with `source = user`.
- `apply`: skips for a user source, a changed content, an unknown kind, or low confidence; writes in the normal case.
- Trigger: a content change submits; changing only `owner` does not; a user-sourced kind never submits.
- Suggestions: tag candidate selection excludes existing tags; mapping and thresholds work; off returns `available: false`.
- Filter: each kind and `unclassified` work; an invalid value returns 400.
- Live tuning (`#[ignore]`): `tests/fixtures/ai/flag_kind.json` with at least 25 labelled features, at least 4 per kind plus a few `unknown` (for example key `x1` with no description).

## Acceptance criteria

- [ ] New features get an AI kind within seconds when the setting is on and a key is set. Editing the description re-classifies; picking a kind in the API stops AI changes.
- [ ] Backfill classifies existing flags without exceeding rate limits (the semaphore holds).
- [ ] `GET /teams/{id}/features?flagKind=ops` works.
- [ ] With AI off, feature create and update behave exactly as before, except for the new optional fields.
- [ ] Contract baseline is updated; all backend tests pass.

## Out of scope

Stale rules (AI-31). UI (AI-32).

## Handoff log

_No entries yet._
