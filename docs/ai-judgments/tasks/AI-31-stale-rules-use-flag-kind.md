# AI-31: Stale rules skip permanent flag kinds

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 335b2f6 (maintainer sign-off 2026-10-02) |
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

- [x] Maintainer sign-off is recorded (2026-10-02, in the Handoff log; work committed directly on `main`).
- [x] Rust and SQL results match in the parity test.
- [x] Features with a null `flag_kind` behave exactly as before.

## Out of scope

UI wording (the stale reasons list already renders whatever the API returns).

## Handoff log

### 2026-10-02, Claude (AI-31 implementation)

**Sign-off:** the maintainer gave sign-off on 2026-10-02 to implement and commit on `main`.

**What changed** (commit 335b2f6):

- `model.rs`: `FlagKind::PERMANENT` (`ops`, `permission`, `config`) and `FlagKind::is_permanent`.
- `logic/feature.rs`: `stale_reasons` is now a free `pub(crate)` function; `FeatureLogicImpl::stale_reasons` delegates to it. For a permanent kind only "Expired" applies; the three rules "No recent evaluations", "No evaluations in 90 days" and "Disabled for 90+ days" are skipped. Source (AI or user) does not matter.
- `logic/feature_tx.rs`: had a third private copy of the rules (used by the create/update-in-tx response mapping). It had the same drift risk, so it now calls the shared function. Without this, a create or update response would still list inactivity reasons for an `ops` flag.
- `database/feature.rs`: `stale_predicate_sql` wraps the three inactivity branches in `(f.flag_kind IS NULL OR f.flag_kind NOT IN ('ops', 'permission', 'config'))`; the expiry branch is unchanged. Doc comments on both say they must stay equivalent.

**Verified:**

- RED: before the change, the three Rust tests failed. With only the Rust change in place, the parity test failed for all 15 permanent-kind scenario rows with an inactivity rule (Rust not stale, SQL stale). GREEN after the SQL change.
- Tests (`logic::feature::stale_rules_tests`): permanent kinds with age and no evaluations are not stale; with expiry they are stale with "Expired" only; release, experiment and NULL keep the old behavior; a matrix of 10 scenarios x 6 kinds (NULL plus 5) checks the exact reason list; the DB parity test creates the 60 features, then compares the Rust result with `stale=true` and `stale=false` list filters. `database::feature::tests::stale_predicate_lists_the_permanent_kinds` ties the SQL kind list to `FlagKind::PERMANENT`.
- `cargo fmt`; `cargo clippy --all-targets` shows the same warnings as before the change; full `cargo test -p feature-toggle-backend` on `feture_toggle_test`: 722 unit, 265 integration, 25 grpc, all pass.
- No DTO, endpoint or migration change, so no contract step.

**For later:**

- The UI (AI-32, `isStaleExemptKind` in `src/lib/flagKind.ts`) lists the same three kinds; the sets match.
- Existing stale flags of these kinds stop being stale at the next read; nothing is stored, so no backfill is needed.
- A new stale rule must be added in `stale_reasons` and `stale_predicate_sql`, plus a row in the `SCENARIOS` table of the parity test.

