# AI-31: Stale rules skip permanent flag kinds

| Field | Value |
|---|---|
| Type | Feature |
| Status | Not started. **Needs maintainer sign-off before merge.** |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | AI-30 |
| Behavior change | **Yes, user-visible.** Flags of kind `ops`, `permission`, or `config` stop showing as stale for inactivity or for being disabled. "Expired" still marks them stale. |
| Design | [design.md §5.3, "Stale rules"](../design.md#53-flag-kind-ai-30-ai-31-ai-32) |

## Goal

Remove stale-flag false positives for long-lived flags. Kill switches and entitlements are often idle or disabled by design.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| What | Where |
|---|---|
| Rust rules: "Expired", "No recent evaluations", "No evaluations in 90 days", "Disabled for 90+ days"; only `active`/`deprecated` lifecycle can go stale on evaluation rules | `logic/feature.rs`: `FeatureLogicImpl::stale_reasons` |
| Computed on read | `logic/feature.rs`: `map_entity_to_api_feature` (`is_stale`) |
| SQL mirror for the `stale` list filter | `database/feature.rs`: `stale_predicate_sql`, used in `push_feature_filters` |

## Changes

1. In `stale_reasons`: if `flag_kind` is `ops`, `permission`, or `config`, skip the three non-expiry rules. Keep "Expired". The source does not matter (AI or user).
2. In `stale_predicate_sql`: wrap the three non-expiry branches with `(f.flag_kind IS NULL OR f.flag_kind NOT IN ('ops','permission','config'))`. Keep the expiry branch unchanged.
3. Make sure the entity carries `flag_kind` into `stale_reasons` (added in AI-30).
4. Add one doc comment on both functions saying they must stay equivalent.

## Tests

- Rust: each permanent kind with old age and no evaluations is not stale; the same with expiry is stale ("Expired" only); `release`, `experiment`, and null keep current behavior.
- SQL (DB test): the `stale=true` filter returns the same set as the Rust rules for a fixture of features covering each kind and rule. Write this as a parity test.

## Acceptance criteria

- [ ] Maintainer sign-off is recorded in the PR.
- [ ] Rust and SQL results match in the parity test.
- [ ] Features with a null `flag_kind` behave exactly as before.

## Out of scope

UI wording (the stale reasons list already renders whatever the API returns).

## Handoff log

_No entries yet._
