# Code Review Findings (October 2026): Backend and Edge Server

Review of `feature-toggle-backend/`, `feature-edge-server/` and `evaluation-engine/` at commit `390446c`. Each issue has its own file under [`issues/`](issues/) with location, failure scenario, fix, behavior impact, tests and acceptance criteria. The files are written so that one agent can pick up one issue and finish it without reading the others.

## Rules for agents working on an issue

1. **One issue per branch and PR.** Branch name: `fix/<id>-<short-name>`, for example `fix/b09-heartbeat-leak`. Do not fix neighboring issues in the same change, even if they touch the same file.
2. **Check "Depends on"** in the issue header. Do not start an issue until its dependencies are merged.
3. **Re-check the code first.** Line numbers are from commit `390446c` and will drift. Find code by symbol name (`grep -n "fn <name>"`). If the code no longer matches the issue, stop and report.
4. **Do not break existing behavior.** Change only what the issue's "Behavior change" row allows. If a fix needs a visible change the issue does not list, stop and ask.
5. **Issues marked "needs maintainer decision" or "sign-off"** (B13, B19, and parts of B10, B15, B16, S01) need a human answer before you write code.
6. **Write the failing test first**, then the fix. Each issue lists the test to add.
7. **Run before you finish:**
   ```bash
   cargo fmt
   cargo clippy --all-targets
   cargo test -p evaluation-engine
   cargo test -p feature-edge-server
   cargo test -p feature-toggle-backend      # needs migrated + seeded DB, see CLAUDE.md
   ./scripts/check-contract-compat.sh        # only if REST DTOs or proto changed
   ```
8. **Repo gotchas** (details in `/CLAUDE.md`):
   - New or changed `sqlx::query!`/`query_as!`: run `cargo sqlx prepare` in `feature-toggle-backend/` and commit `.sqlx/`.
   - Never edit an applied migration. Add a new timestamped one.
   - Any write that changes evaluable feature state must send a `FeatureUpdate` on the broadcast channel.
   - After changing code, run `graphify update .`.
9. **When done:** set the issue's `Status` row to `Fixed in <PR/commit>` and tick it in the table below.

## Status legend

- **Confirmed**: traced in code by the reviewer.
- **Verified**: traced independently by a second agent, with file and line evidence.
- **Reported**: plausible, but semantics need a decision before you change anything.

## Issues

### Bugs

| Done | ID | Title | Severity | Crate | Behavior change | Depends on |
|---|---|---|---|---|---|---|
| [x] | [B01](issues/B01-feature-key-lookup-substring-match.md) | Feature key lookups use `ILIKE '%key%'` and return the wrong flag | Critical | backend | Yes (intended) | — |
| [ ] | [B02](issues/B02-duplicate-key-check-substring-match.md) | Duplicate-key checks reject valid keys (`check` vs `checkout`) | Medium | backend | Yes (intended) | B01 |
| [x] | [B03](issues/B03-stream-updates-cross-team.md) | Stream updates are not filtered by team | Critical | backend | Yes (intended) | — |
| [x] | [B04](issues/B04-stream-snapshot-deadlock-and-lost-updates.md) | Stream snapshot hangs above 64 features and loses updates | Critical | backend | Minor | — |
| [x] | [B05](issues/B05-ofrep-empty-client-secret.md) | OFREP sends an empty client secret, so OFREP never works | Critical | edge | Yes (intended) | — |
| [x] | [B06](issues/B06-ofrep-auth-error-status.md) | OFREP returns 502 for bad credentials instead of 401/403 | Low | edge | Yes (OFREP only) | B05 |
| [x] | [B07](issues/B07-retry-backoff-math.md) | Retry backoff is base^n (500 ms, 250 s, 34.7 h) | High | edge | Waits only | — |
| [x] | [B08](issues/B08-retry-permanent-grpc-errors.md) | Edge retries permanent gRPC errors | Medium | edge | Fail fast | — |
| [ ] | [B09](issues/B09-heartbeat-task-leak.md) | Heartbeat task leaks on every reconnect | Medium | edge | No | — |
| [x] | [B10](issues/B10-edge-env-overrides-ignored.md) | `EDGE_*` env overrides are silently ignored | High | edge | Yes (env starts applying) | — |
| [ ] | [B11](issues/B11-assignment-push-missing-team-check.md) | `push_user_assignments` writes other teams' assignments | High | backend | Yes (intended) | — |
| [ ] | [B12](issues/B12-evaluation-timestamp-fallback-breaks-dedupe.md) | Missing evaluation timestamp becomes `now()` and breaks dedupe | Low | backend | Small | — |
| [ ] | [B13](issues/B13-dependency-bucketing-uses-root-key.md) | Dependencies bucketed with the root flag key | High | engine | **Yes, user-visible, needs sign-off** | — |
| [ ] | [B14](issues/B14-ofrep-bulk-etag-ignores-context.md) | OFREP bulk ETag ignores context, so 304 returns stale results | Medium | edge | Yes (intended) | B05 |
| [x] | [B15](issues/B15-stale-flags-after-reconnect-or-rename.md) | Stale flags after reconnect or rename; new flags missed | High | backend + edge | Yes (intended) | B01, B03, B04 |
| [x] | [B16](issues/B16-edge-cache-not-team-scoped.md) | Edge cache not team-scoped; bulk lists other teams' flags | High | edge | Yes (intended) | B03 |
| [ ] | [B17](issues/B17-assignment-list-missing-environment-filter.md) | Assignment list by environment returns all environments | Medium | backend | Yes (intended) | — |
| [ ] | [B18](issues/B18-assignment-warmup-caches-true-for-all-variants.md) | Assignment warm-up caches `true` for every variant (masked) | Medium | edge | No (latent) | — |
| [ ] | [B19](issues/B19-non-boolean-dependency-blocks-dependents.md) | Non-boolean dependency blocks dependents | Medium | engine | **Needs decision** | — |

### Performance

| Done | ID | Title | Severity | Crate | Behavior change | Depends on |
|---|---|---|---|---|---|---|
| [ ] | [P01](issues/P01-purge-assignments-hot-path.md) | `purge_assignments_for_feature` is O(all assignments) on hot paths (steps 1–2 fixed on `perf/p01-purge-assignments`; step 3 open) | High | edge | No (steps 1–2) | B18 (step 3 only) |
| [ ] | [P02](issues/P02-snapshot-n-plus-one-queries.md) | N+1 queries (3–5 per stage) on snapshot and Evaluate | High | backend | No | — |
| [ ] | [P03](issues/P03-batched-assignment-upsert.md) | One DB upsert per assignment row | Medium | backend | Minor | B11 |
| [x] | [P04](issues/P04-cache-client-auth-failures.md) | Failed client-info lookups not cached | Low | edge | No | B06, B08 |

### Security

| Done | ID | Title | Severity | Crate | Behavior change | Depends on |
|---|---|---|---|---|---|---|
| [x] | [S01](issues/S01-committed-edge-client-secret.md) | Live-looking client secret committed in `feature-edge-server/config.toml` | High | edge config | Won't fix (old local test secrets) | B10 (for env-based replacement) |

## Suggested order

Work in waves. Issues within one wave are independent and can run in parallel on separate branches.

1. **Wave 1: critical, self-contained**
   - backend: B04, B03, B01, B11
   - edge: B05, B07, B09, B10
   - S01 rotation: human, any time
2. **Wave 2: follow-ups on wave 1**
   - B02, B06, B08, B15, B16, B17, P02
3. **Wave 3: cleanup and performance**
   - B18, P01, P03, P04, B12, B14
4. **Needs decision first:** B13, B19

## Rejected finding

- **Ingest timeout double-counts evaluations:** false. The DB unique index on `ingest_fingerprint` with `ON CONFLICT DO NOTHING` drops retried duplicates. See the background section in [B12](issues/B12-evaluation-timestamp-fallback-breaks-dedupe.md). Do not change the timeout path.
