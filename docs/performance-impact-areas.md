# Performance Impact Areas (Rust)

This document captures code paths that are likely to create measurable performance impact as data volume or request concurrency grows.

## Implementation status (2026-02-07)

- 1. N+1 feature dependency loading: Addressed with batched dependency hydration in `feature-toggle-backend/src/database/feature.rs`.
- 2. N+1 client web-origin loading: Addressed with batched origin loading in `feature-toggle-backend/src/database/client.rs`.
- 3. Per-activity sequential lookup amplification: Addressed with request-scope lookup caches in `feature-toggle-backend/src/streaming.rs`, `feature-toggle-backend/src/rest/metrics.rs`, and `feature-toggle-backend/src/rest/stream.rs`.
- 4. `%term%` `ILIKE` scan risk: Deferred per request (no change made).
- 5. Regex compilation in hot path: Addressed with regex compile cache in `evaluation-engine/src/lib.rs`.
- 6. Re-sorting allocations on each evaluation: Addressed by skipping sort/copy when already sorted in `evaluation-engine/src/lib.rs`.
- 7. Recursive dependency evaluation without memoization: Addressed with per-evaluation memoization in `evaluation-engine/src/lib.rs`.
- 8. Extra cloning in pipeline row mapping: Addressed by removing clone-heavy mapping in `feature-toggle-backend/src/database/pipeline.rs`.
- 9. Full-table deduper retain under lock: Addressed with incremental queue-based expiry cleanup in `feature-toggle-backend/src/cluster/mod.rs`.
- 10. Count + data double query in paginated endpoints: Addressed with windowed/CTE single-query pagination in `feature-toggle-backend/src/database/client.rs`, `feature-toggle-backend/src/database/feature.rs`, and `feature-toggle-backend/src/database/pipeline.rs`.

## High impact

### 1. N+1 query pattern when loading feature dependencies
- Evidence:
  - `feature-toggle-backend/src/database/feature.rs:1227`
  - `feature-toggle-backend/src/database/feature.rs:1314`
  - `feature-toggle-backend/src/database/feature.rs:1397`
  - `feature-toggle-backend/src/database/feature.rs:449`
- Why it matters:
  - `get_features*` loads base feature rows, then performs one extra dependency query per feature.
  - For large pages this scales as `1 + N` DB round trips.
- Optimization direction:
  - Batch dependency fetch with `WHERE feature_id = ANY($1)` and build a map in-memory.

### 2. N+1 query pattern for client web origins
- Evidence:
  - `feature-toggle-backend/src/database/client.rs:240`
  - `feature-toggle-backend/src/database/client.rs:339`
  - `feature-toggle-backend/src/database/client.rs:430`
  - `feature-toggle-backend/src/database/client.rs:127`
- Why it matters:
  - List endpoints load clients first, then call `load_web_origins` per web client.
  - This becomes expensive for teams with many browser clients.
- Optimization direction:
  - Fetch origins in one query (`client_id IN (...)`) and group by `client_id`.

### 3. Activity listing performs per-item async lookups (compounded N+1)
- Evidence:
  - `feature-toggle-backend/src/rest/metrics.rs:1224`
  - `feature-toggle-backend/src/rest/metrics.rs:1230`
  - `feature-toggle-backend/src/rest/metrics.rs:1245`
  - `feature-toggle-backend/src/streaming.rs:24`
  - `feature-toggle-backend/src/rest/metrics.rs:401`
- Why it matters:
  - For each activity row, code may call multiple repository/logic methods sequentially.
  - This can produce high tail latency and DB load for large pages.
- Optimization direction:
  - Resolve team/entity metadata in bulk (preload maps) and avoid per-item repository calls.
  - If per-item calls are still required, use bounded concurrency instead of fully sequential awaits.

### 4. Wildcard text filters likely force expensive scans
- Evidence:
  - `feature-toggle-backend/src/database/feature.rs:1189`
  - `feature-toggle-backend/src/database/client.rs:211`
  - `feature-toggle-backend/src/database/pipeline.rs:308`
- Why it matters:
  - `ILIKE '%term%'` is usually not index-friendly on standard btree indexes.
  - Costs grow quickly with table size.
- Optimization direction:
  - Add trigram indexes (`pg_trgm`) or switch to prefix-search where possible.

## Medium impact

### 5. Regex compilation during rule evaluation
- Evidence:
  - `evaluation-engine/src/lib.rs:326`
- Why it matters:
  - `Regex::new` inside evaluation path compiles patterns repeatedly.
  - Expensive when regex operators are used frequently.
- Optimization direction:
  - Precompile and cache regex patterns (for example by criterion ID + pattern string).

### 6. Re-sorting allocations on each evaluation
- Evidence:
  - `evaluation-engine/src/lib.rs:238`
  - `evaluation-engine/src/lib.rs:239`
- Why it matters:
  - `to_vec + sort` happens for each call to weighted variant selection.
  - Extra allocations and `O(n log n)` work in hot path.
- Optimization direction:
  - Persist allocations pre-sorted at write time, or sort once when materializing feature data.

### 7. Recursive dependency evaluation without memoization
- Evidence:
  - `evaluation-engine/src/lib.rs:535`
  - `evaluation-engine/src/lib.rs:536`
- Why it matters:
  - Shared dependency subgraphs are re-evaluated repeatedly.
  - Deep graphs can increase CPU and stack pressure.
- Optimization direction:
  - Add memoization keyed by feature ID within one evaluation execution.

### 8. Extra cloning in pipeline row mapping
- Evidence:
  - `feature-toggle-backend/src/database/pipeline.rs:249`
  - `feature-toggle-backend/src/database/pipeline.rs:269`
- Why it matters:
  - Clones entire row vector and stage vector before returning.
  - Adds avoidable allocations on list/get endpoints.
- Optimization direction:
  - Iterate by value directly and avoid `.clone().split_off(0)` / `stages.clone()`.

### 9. Deduper cleanup does full-table retain under mutex
- Evidence:
  - `feature-toggle-backend/src/cluster/mod.rs:205`
  - `feature-toggle-backend/src/cluster/mod.rs:216`
- Why it matters:
  - `retain` scans all entries while holding async mutex when size threshold is crossed.
  - Can create latency spikes under burst traffic.
- Optimization direction:
  - Use a more incremental eviction strategy (time-wheel/queue) and reduce lock hold time.

### 10. Paginated endpoints issue count query + data query every request
- Evidence:
  - `feature-toggle-backend/src/database/client.rs:269`
  - `feature-toggle-backend/src/database/feature.rs:1244`
  - `feature-toggle-backend/src/database/pipeline.rs:361`
- Why it matters:
  - Doubles DB round trips for every page read.
  - Expensive for high-frequency UI polling.
- Optimization direction:
  - Consider optional count, cached count, or single-query approaches (`COUNT(*) OVER()`).

## Quick validation plan

1. Add request-level timers around the affected endpoints (`/metrics/activities`, feature/client list APIs).
2. Capture query counts per request and identify N+1 signatures.
3. Run EXPLAIN ANALYZE for the `ILIKE` queries on realistic data volume.
4. Benchmark evaluation-engine with and without regex/sort caching.
