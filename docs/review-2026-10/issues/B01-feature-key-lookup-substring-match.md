# B01: Feature key lookups use substring match and can return the wrong flag

| Field | Value |
|---|---|
| Type | Bug (correctness) |
| Severity | Critical |
| Status | Confirmed (code traced) |
| Crate | `feature-toggle-backend` |
| Behavior change | Yes, intended: lookups return only the flag with the exact key |
| Related | [B02](B02-duplicate-key-check-substring-match.md) uses the same filter |

## Problem

`FeatureRepository::get_features(team_id, Some(key), _)` is a list/search query. Its key filter is a case-insensitive substring match:

```rust
// feature-toggle-backend/src/database/feature.rs:1548
if let Some(key) = key {
    query_builder.push(" AND f.key ILIKE ");
    query_builder.push_bind(format!("%{key}%"));
}
```

Several callers use it as a single-flag lookup and take `.pop()`. That is the last row in key order, not the flag whose key equals the request:

| Caller | Location |
|---|---|
| gRPC `Evaluate` | `feature-toggle-backend/src/grpc/mod.rs:867` |
| gRPC `GetFeatureByKey` | `feature-toggle-backend/src/grpc/mod.rs:950` |
| gRPC stream snapshot, keys-filtered | `feature-toggle-backend/src/grpc/mod.rs:1288` (extends all matches, not one) |
| REST evaluation | `feature-toggle-backend/src/rest/evaluation.rs:325` |

The key is also not escaped, so `_` and `%` in a key act as SQL wildcards.

## How it fails

1. Team has flags `checkout` and `checkout-v2`.
2. The edge server calls `GetFeatureByKey("checkout")`.
3. The query returns both rows, ordered by key. `.pop()` returns `checkout-v2`.
4. The edge caches the `checkout-v2` config and serves it for every `checkout` evaluation.

Snapshot variant: an edge subscribed to keys `["a"]` receives every flag whose key contains `a`.

## Fix

1. Add an exact-key method to `FeatureRepository`, for example `get_feature_by_key(team_id, key) -> Result<Option<Feature>, Error>`, using `f.key = $2`. Reuse the same SELECT and row mapping as `get_features` so the returned struct is identical.
2. Switch the four callers above to it. For the snapshot loop, push the single result if present.
3. Keep `get_features` and its `ILIKE` filter unchanged. The admin list/search API depends on substring search.
4. The trait has `#[automock]`. Update mock expectations in unit tests that cover these callers.
5. If the query uses `sqlx::query!`/`query_as!`, run `cargo sqlx prepare` in `feature-toggle-backend/` and commit the `.sqlx/` changes. `QueryBuilder` queries need no cache update.

Case sensitivity: the DB constraint is `UNIQUE (key, team_id)`, which is case-sensitive. Use exact case-sensitive `=` for lookups. Edge servers and SDKs send the key as stored.

## Behavior impact

- Lookups of a key that only exists as a substring of another key now return "not found" instead of the wrong flag.
- Lookups with a different case (`Checkout` vs `checkout`) now return "not found". Check the API tests and SDK fixtures for mixed-case lookups before merging.

## Tests

- DB test in `feature-toggle-backend/tests/database/` (runs through `tests/integration_test.rs`): create `checkout` and `checkout-v2` in one team, then assert that `get_feature_by_key(team, "checkout")` returns the `checkout` row and that `"check"` returns `None`.
- gRPC test in `feature-toggle-backend/tests/grpc_tests*`: `GetFeatureByKey("checkout")` returns key `checkout`.
- REST: add a case in `api-tests/` for `/api/v1` evaluation with overlapping keys.

## Acceptance criteria

- No caller that needs one flag uses `get_features(.., Some(key), ..)` plus `.pop()`.
- The admin list endpoint still supports substring search (existing tests pass).
- `cargo test -p feature-toggle-backend` passes.
