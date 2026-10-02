# AI-30: Flag kind (column, classification, suggestions, backfill, filter)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in f928d65 |
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

### 2026-10-02, Claude (AI-30 implementation)

**What changed** (commit f928d65):

- Migration `20261002050000_feature_flag_kind.sql`: the three `features` columns from design §4.4 (CHECK constraints on kind and source). No broadcast, no proto change.
- `model.rs`: `FlagKind` (snake_case, `ALL`, `as_str`, `FromStr`, `ToSchema`), `FlagKindSource`, `FlagKindFilter` (`Kind(_)` or `Unclassified`, parses the query value). The REST layer reuses these types. They are the entity, model and DTO types, so no mirror enums. `ApiDoc` lists `FlagKind` and `FlagKindSource`.
- Plumbing: entity `Feature`, `FeatureWithStageRow`, every `SELECT` list in `database/feature.rs` (there are six, not one), `model::Feature`, `FeatureResponse` (`flagKind`, `flagKindSource`, `flagKindConfidence`). About 30 struct literals in tests were fixed.
- User choice: `CreateFeatureRequest.flagKind: Option<FlagKind>`; `UpdateFeatureRequest.flagKind: Option<Option<FlagKind>>` via a small `deserialize_present` helper (no `serde_with` in the crate). `resolve_flag_kind_update` (pure, in `database/feature.rs`) decides: absent or equal keeps all three columns; a different value or `null` sets `source = user`, confidence NULL. Both `create_feature` (non-tx) and `create_feature_tx` insert the kind with source `user`; `update_feature` (shared by the tx path) applies the rule. The non-tx logic mapping in `logic/feature.rs` passes the field too.
- `judgment/flag_kind.rs`: `content_hash`, `build_input` (`{feature: {key, description, purpose, tags, feature_type}, content_hash}`), `build` (one Choice, 6 options with the design §5.3 criteria), `classify` and `derive` (`{kind | null, confidence, probabilities}`; null for `unknown`, confidence below `MIN_KIND_CONFIDENCE` 0.5, a missing answer, or a name that is not a kind), `FlagKindHandler` (`apply`: reload, skip on a user source, skip when the content hash changed, skip on a null kind, else `FeatureRepository::set_ai_flag_kind`, the guarded UPDATE), `record_flag_kind` (trigger), `backfill`, `suggestion_parts`, `suggested_tags`. Constants: `MIN_KIND_CONFIDENCE` 0.5, `MIN_TAG_PROBABILITY` 0.6, `MAX_SUGGESTED_TAGS` 5, `MAX_TAG_CANDIDATES` 50, `BACKFILL_LIMIT` 500. Handler registered in `lib.rs::run` in the same `with_handler` chain.
- Trigger: `rest::ai::record_flag_kind` (a no-op without an `AiRuntime`), called in `create_feature` after the transaction commits and in `update_feature` after `broadcast_feature_update`. It skips on a user source or a toggle that is off, reads the stored judgment with the new `JudgmentService::judgment_for`, and submits only when none exists or its `input.content_hash` differs. Errors are logged and dropped; the response never changes.
- `POST /api/v1/teams/{team_id}/ai/feature-suggestions` (`rest/ai.rs`): body `{key, description?, purpose?, tags?}`; response `{available, kind: {value, probabilities, confidence} | null, tags: [{tag, probability}]}`. `{available: false}` (no other fields) for no client, toggle off, tag read error, or client error. 400 only for a blank or over-255-character key, or more than 50 body tags. Body tags are trimmed, lowercased and de-duplicated; the same list is the exclusion list for candidates.
- `POST /api/v1/teams/{team_id}/ai/flag-kind/backfill`: admin only (`ensure_admin`, 403 otherwise). Returns `{queued: n}`; `0` when there is no judgment service or the team toggle is off (no data sent). Picks up to 500 features, then submits those without a `done` judgment for the current content hash (so a pending or failed row is submitted again, and an edited feature too).
- `flagKind` list filter: `FeatureListQuery.flagKind: Option<String>`; an invalid value gives 400 before the logic is called. `push_feature_filters` adds `f.flag_kind = $n` or `f.flag_kind IS NULL`. The value is threaded through `get_features_with_offset_filtered` (logic and repository), `get_features_windowed`, and `count_filtered_features`. `get_features_filtered` (not used by REST) passes `None`.
- New `FeatureRepository` methods: `get_features_needing_flag_kind`, `get_team_tag_candidates` (SQL `unnest` with exclusion, `ORDER BY COUNT(*) DESC, tag`, limit applied after the exclusion), `set_ai_flag_kind` (the guarded UPDATE; false when a user chose or cleared the kind).
- `JudgmentService::judgment_for` and `judgments_for` (thin reads of the judgment store).
- AI-10 follow-up: `approval_risk::build_input` now sends the feature's `flag_kind` (was hard-coded null); the snapshot test sets `ops`, and a new test covers null.
- Contract baseline updated (`contracts/baseline/contract-hashes.json`).

**Decisions and behavior to know:**

- Backfill selection also excludes `flag_kind_source = 'user'`: a user who cleared a kind sets a NULL kind with source `user`, and AI would skip it anyway, so there is no point in spending a call.
- `archived_at IS NULL` is the archive test (not `lifecycle_stage`).
- Content hash: SHA-256 (`service::input_hash`) over `{key, description, purpose, tags}`. Description and purpose are trimmed, blank counts as absent, and text is cut at 1000 characters (input and hash use the same cleaned values). Tags are hashed as stored (already sorted and de-duplicated on write).
- A kind that AI wrote stays until a later judgment replaces it. An edit that makes the answer `unknown` or low confidence keeps the old kind (apply skips on null).
- The trigger runs only in `create_feature` and `update_feature`. Other paths that change tags or content (bulk tag action, version rollback, CSV or template flows) do not re-classify; the backfill only helps features whose kind is still NULL.
- The suggestions state omits `feature_type` (the create form has no type yet); the async state includes it (`simple` or `contextual`).
- Suggestion payloads are not stored and not logged; only `log_label()` is logged on failure.
- The retry sweep reruns a failed row through `FlagKindHandler::build` from the stored input; a stale row is dropped by `apply`'s hash check.

**Verified:**

- `cargo fmt`, `cargo clippy --all-targets`: no new warnings (the repo's existing ones remain). `cargo test -p feature-toggle-backend` on `feture_toggle_test`: 675 unit tests, 256 integration tests, 25 grpc tests and the other suites all pass, except that `rest::operational_safety::authorization_tests::reschedule_with_a_reason_records_against_the_scheduled_change` failed in 3 of 5 full runs (404 "pending scheduled change not found": a due change that another test's scheduler run can take; it passes alone and in the other runs). It is unrelated to this task.
- New tests: `judgment::flag_kind` (hash, input shape, wire snapshots for the kind request and the suggestion request, derive table, thresholds, `needs_submit`, tag threshold/limit/order, `apply` skips and write, trigger: new, content change, owner only, user source, AI source, toggle off, no service; backfill), `database::feature::tests::flag_kind_update_follows_the_user_choice_rules`, `rest::feature::types::tests` (absent, null, value), `rest::feature::tests` (each filter forwarded, 400 for invalid, create and update handler triggers with the real DB), `rest::ai::tests::flag_kind_endpoints` (suggestions off, no client, client error, mapping with exclusion list and limit, null kind, blank key; backfill 403, queue, off), integration `flag_kind_test` (create with kind, update rules, AI guard, each filter and `unclassified`, tag candidates, backfill selection).
- Live: `cargo test -p feature-toggle-backend --test flag_kind_live_test -- --ignored --nocapture`. Fixture `tests/fixtures/ai/flag_kind.json`: 27 labelled features (5 release, 4 experiment, 5 ops, 4 permission, 5 config, 4 unknown). Accuracy 27/27 = 1.00 with the design thresholds unchanged (min asserted 0.75). Confidence is 1.0 for the clear cases; the key-only `unknown` cases score 0.84 to 1.0 as `unknown`. A second live test sends the suggestions shape (kind plus 50 tag questions in one request): the API accepts it, `ops` at 1.0, and the matching tag `payments` scored 0.63, only just above the 0.6 cut-off, while the unrelated ones scored below. The fixture is easy; tags near the cut-off need more data before the 0.6 threshold is trusted.

**For later tasks:**

- AI-31 reads `feature.flag_kind` (entity) in `FeatureLogicImpl::stale_reasons` and `stale_predicate_sql`; the field is already on the entity and every select list.
- AI-32 (UI): `FeatureResponse` has `flagKind`, `flagKindSource`, `flagKindConfidence` (confidence only when the source is `ai`). `PATCH` takes `flagKind` as absent, `null`, or a kind; resend the current value freely. Suggestions endpoint and backfill endpoint are above; backfill returns `{queued}` and the kinds arrive later in the background.
- AI-40 can add a `flag_kind` Choice; filtering by `FlagKindFilter` is in the repository already (`unclassified` for NULL).
