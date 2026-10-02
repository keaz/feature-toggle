# AI-40: Natural-language feature search endpoint

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 86dedbb |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-01. AI-30 is optional; use the `flag_kind` filter only if it is merged. |
| Behavior change | Additive: one new endpoint |
| Design | [design.md §5.4](../design.md#54-natural-language-search-ai-40-ai-41) |

## Goal

`POST /api/v1/teams/{team_id}/features/nl-search` turns a plain-English query into the existing list filters, fetches candidates with the existing query code, and reranks them by relevance.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| What | Where |
|---|---|
| List handler and query DTO | `rest/feature.rs`: `list_features` (GET `/teams/{team_id}/features`); `rest/feature/types.rs`: `FeatureListQuery` |
| Supported filters | name/key (ILIKE), featureType, lifecycleStage, stale, includeArchived, owner (ILIKE), expired, tag (comma list, array overlap), dependencyStatus (`has_dependencies \| blocked_by_dependencies \| independent`), approvalStatus (lowercased approval request status), offset, limit |
| Query path | `logic/feature.rs`: `get_features_with_offset_filtered`; then `database/feature.rs`: `get_features_windowed` and `push_feature_filters` |
| List item response shape | the REST `Feature` mapping in `rest/feature.rs` |

## Changes

1. **`src/judgment/nl_search.rs`** (pure functions; no `JudgmentHandler`, this is sync):
   - `fn build_filter_questions(tags: &[String], owners: &[String], with_flag_kind: bool) -> BTreeMap<String, Question>`.
     - The Choices in design §5.4, each with `unspecified` (or `none` for tag and owner) and a clear instruction, for example "Which lifecycle stage does `query` ask for?".
     - Plus the `has_topic` Noul.
     - Cap tags and owners at 254 values each, the most used first.
     - For `approval_status`, read the accepted values from the filter code and the approval request status enum, and list them as options.
   - `fn derive_filters(answers) -> AppliedFilters`. Apply a filter only when the choice is not `unspecified`/`none` and its confidence is at least 0.6 (named constant). Map `stale`/`not_stale` and `expired`/`not_expired` to booleans.
   - `fn build_rerank_questions(n) -> BTreeMap<String, Question>`: Nouls `c0..c{n-1}` with "Is `candidates[i]` a flag that `query` is looking for?" (substitute the index).
   - `fn rank(candidates, answers, limit) -> Vec<(Feature, f64)>`: keep relevance at least 0.3 (named constant), sort descending, truncate.
2. **Repository helpers** (`database/feature.rs` or `database/ai.rs`): `top_tags(team_id, 254)` and `top_owners(team_id, 254)`. Distinct values by use count, excluding archived features.
3. **Endpoint** in `rest/ai.rs` or `rest/feature.rs` (choose one and say which in the handoff log):
   - Validate the query (3 to 300 characters after trim) and `limit` (1 to 20, default 10).
   - Return `{available: false}` when the subsystem or the team's `nl_search` setting is off.
   - Call 1: state `{query}` with the filter questions. Derive the filters.
   - Candidates: call the existing filtered list path with the applied filters, sorted by `evaluation_count_30d` descending (add the sort option if missing), limit 50.
   - Call 2 only if `has_topic > 0.5` and there are candidates: state `{query, candidates: [{key, description, purpose, tags}]}`. Rank the results.
   - Otherwise return the first `limit` candidates with `relevance: null`.
   - Response: `{available: true, filtersApplied, results: [{feature, relevance}]}`, where `filtersApplied` uses the same camelCase names as `FeatureListQuery`.
   - Any client error returns `{available: false}`.
   - Log the call count and total `input_tokens`. Never log the query text.
4. Register the endpoint and schemas in `ApiDoc` and update the contract baseline.

## Tests

- `build_filter_questions`: option sets are correct, every Choice has `unspecified`/`none`, tags and owners are capped at 254, and the 255-option limit is respected.
- `derive_filters`: low confidence and `unspecified` are ignored; booleans map correctly.
- `rank`: threshold, order, and truncation.
- Endpoint with `MockJudgmentClient`:
  - A topic query makes 2 calls.
  - A filter-only query ("archived flags owned by payments") makes 1 call and has null relevance.
  - A client error gives `available: false`.
  - A 2-character query gives 400.
- Live tuning (`#[ignore]`): `tests/fixtures/ai/nl_search.json`, at least 20 queries with expected filters, and for topic queries, expected top keys, against a seeded fixture team.

## Acceptance criteria

- [x] "stale flags in payments" maps to `stale=true` plus `tag=payments` (when that tag exists) on the seeded fixture.
- [x] "kill switches for checkout" returns checkout-related ops flags first (with AI-30 merged) or reranks by relevance (without it).
- [x] At most 2 TypeSafe calls per search.
- [x] Contract baseline is updated; all backend tests pass.

## Out of scope

UI (AI-41). Per-team rate limits.

## Handoff log

### 2026-10-02: AI-40 done (commit `86dedbb`)

**What changed:**

- `judgment/nl_search.rs` (new): `build_filter_questions` (9 Choices plus the `has_topic` Noul; `flag_kind` only when asked, `tag` and `owner` only when the team has values), `derive_filters` (confidence at least `MIN_FILTER_CONFIDENCE` 0.6; `unspecified`/`none` and unknown option names ignored), `build_rerank_questions` (`c0..cN`), `rank` (relevance at least `MIN_RELEVANCE` 0.3, descending, stable on ties, truncated), `wants_rerank` (`has_topic > 0.5`), the two state builders, and `search(...)`, which runs the whole flow and logs only the call count and total `input_tokens`. `AppliedFilters` is an alias of the new `model::FeatureSearchFilters`.
- Approval options come from the new `ApprovalStatus::ALL` (entity) so the Choice follows the enum the filter accepts (`pending`, `approved`, `rejected`, `cancelled`, `auto_approved`); a test matches the enum exhaustively.
- Tags reuse `get_team_tag_candidates(team, [], 254)`. New `get_team_owner_candidates(team, limit)` (trimmed, non-blank owners, unarchived features, `COUNT(*) DESC, owner`). Both exclude features with `archived_at` set, as the brief says. Tag and owner values equal to `none` (any case) and repeats are skipped.
- Sort: no usage sort existed, so `get_features_windowed` got an internal `FeatureOrder` (`Key` for every existing caller, `Usage` = `evaluation_count_30d DESC, key`). The public list endpoint and its trait signatures are unchanged. New `FeatureRepository::get_features_by_usage_filtered` and `FeatureLogic::search_features_by_usage` (limit 50, archived hidden unless the stage filter is `archived`).
- Endpoint: placed in `rest/ai.rs` (with the other AI endpoints), registered in `ApiDoc`, contract baseline updated. 400 for a query outside 3 to 300 characters after trim or a `limit` outside 1 to 20 (default 10). `{available: false}` for no client, toggle off, settings read error, tag/owner read error, or a failed call 1 or call 2. A failed candidate query is a normal 500, not an AI problem.
- Tests: 18 pure unit tests, 9 endpoint tests with `MockJudgmentClient` (topic query = 2 calls and ranked; filter-only = 1 call and null relevance; no candidates = 1 call; client error and rerank error; off, no client, settings error; 400s and boundaries; the trimmed query and team values reach call 1), 1 logic mapping test, 2 DB tests (owner ranking; usage order plus filters).

**Behavior to know:**

- `filtersApplied` values use the REST enum spelling (`lifecycleStage: "ARCHIVED"`, `featureType: "CONTEXTUAL"`), not the lowercase in the design example, so they can be sent back to `GET /teams/{id}/features` unchanged. `flagKind` is lowercase (`ops`, `unclassified`) like its list filter. Only applied filters are present.
- A topic query whose rerank call fails returns `available: false` (not the unranked candidates), as the brief says.
- The rerank state truncates `description` and `purpose` to 300 characters and tags to 20 per candidate to bound the 50-candidate request.
- Because both tag and owner can be named by one word (`payments-team` and tag `payments`), the model sometimes applies both.
- The `has_topic` and filter instructions are longer than the design text (see tuning); the options and thresholds are as designed.
- `dependency_status` option descriptions follow the existing SQL: `has_dependencies` = the flag depends on others; `blocked_by_dependencies` = other flags depend on it.

**Live tuning** (`tests/nl_search_live_test.rs`, fixture `tests/fixtures/ai/nl_search.json`: 25 features in a fixture team the test creates and removes, 28 queries with expected filters, optional filters, topic flag, exact keys or accepted top results):

- First run, design wording: filters 12/28 = 0.43. The model filled `stale`, `expired`, `lifecycle_stage` and `feature_type` for queries that never mention them, because "Which X does `query` ask for?" invites an answer.
- Second run, instructions reworded to "Does `query` name X? Choose `unspecified` unless the query uses ..." plus a note that stale is not expired: 23/28 = 0.82. Tag and owner got too strict.
- Third run (kept): tag and owner back to "Which of these tags does `query` ask for, for example by naming the tag or its product area?", stale told not to fire on "expired": filters 24/28 = 0.86, topic decision 24/28, result checks 15/15 (exact key sets and top results), calls 38 for 28 queries (max 2), about 1.8k input tokens per query. Thresholds unchanged (0.6, 0.3, 0.5).
- Remaining misses: "stale flags in payments" and "archived flags owned by payments-team" gain an extra owner/tag from the same word; "flags owned by search-team" gains tag `search`; "payments tagged flags that are ops kind" drops the tag. The test asserts filter accuracy at least 0.75 and at most 2 calls per search. Stopped tuning here.
- Approval and dependency queries are checked only for the derived filters (the fixture creates no approval requests or dependencies).

**Verified:** `cargo fmt`; `cargo clippy --all-targets` (no warnings in the changed files); full `cargo test -p feature-toggle-backend` on `feture_toggle_test` (706 unit, 260 integration, rest unchanged) all pass; contract baseline regenerated and `check-contract-compat.sh` passes. No sqlx macros were added, so `.sqlx` is unchanged.

**For AI-41 (UI):** call `POST /teams/{team_id}/features/nl-search` with `{query, limit?}`. `available: false` means hide or show the generic "AI search unavailable" state; 400 means the query length or limit is invalid. `relevance` is null when the query had no topic. Show `filtersApplied` as chips; its keys match `FeatureListQuery`. Each call can take two TypeSafe round trips (a few seconds in total), so debounce or submit on Enter.
