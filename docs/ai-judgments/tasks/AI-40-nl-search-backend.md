# AI-40: Natural-language feature search endpoint

| Field | Value |
|---|---|
| Type | Feature |
| Status | Not started |
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

- [ ] "stale flags in payments" maps to `stale=true` plus `tag=payments` (when that tag exists) on the seeded fixture.
- [ ] "kill switches for checkout" returns checkout-related ops flags first (with AI-30 merged) or reranks by relevance (without it).
- [ ] At most 2 TypeSafe calls per search.
- [ ] Contract baseline is updated; all backend tests pass.

## Out of scope

UI (AI-41). Per-team rate limits.

## Handoff log

_No entries yet._
