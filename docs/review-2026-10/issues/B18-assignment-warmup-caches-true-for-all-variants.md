# B18: Edge assignment warm-up caches `true` for every variant (masked today)

| Field | Value |
|---|---|
| Type | Bug (latent, evaluation correctness) |
| Severity | Medium. Hidden today because the startup snapshot purges these entries. It becomes active once P01 or any change stops purging on snapshot. |
| Status | Fixed on branch `fix/b18-assignment-warmup-variants` |
| Crate | `feature-edge-server` |
| Behavior change | No today (masked). Prevents wrong results later. |
| Related | [P01](P01-purge-assignments-hot-path.md) (fix B18 before or together with P01) |

## Problem

`load_user_assignments` (`feature-edge-server/src/grpc_client.rs:150-180`) loads persisted sticky assignments at startup and caches each one as `value: serde_json::json!(true)`, whatever the stored variant:

```rust
// feature-edge-server/src/grpc_client.rs, about 163-176
app.assigned_cache.insert(
    key,
    crate::CachedAssignment {
        value: serde_json::json!(true),
        variant: if a.variant.is_empty() { None } else { Some(a.variant.clone()) },
        ..
    },
);
```

`/evaluate` serves cached assignments before local evaluation (`handlers.rs:722-741`). An assignment whose variant value is `false`, a string or an object would be served as `true`.

Today the full snapshot at stream start runs `purge_assignments_for_feature` for every feature (`grpc_client/stream.rs:80-95`), which wipes these entries. So the warm-up has no effect, good or bad.

## Fix

1. Resolve the real variant value when the feature is known. Options:
   - Store only the variant name at warm-up, and resolve the value from the cached feature config at serve time.
   - Or delay the warm-up until after the first snapshot, and look up the variant's value from the feature.
2. Skip assignments whose variant no longer exists in the feature.

## Tests

- Warm-up with an assignment for variant `off` whose value is `false`. Stop the snapshot purge in the test, evaluate, and assert `false` is returned, not `true`.
- Warm-up with an empty variant: behavior unchanged (`true` for boolean on).

## Acceptance criteria

- Warmed-up assignments return the same value that a fresh evaluation of that variant returns.
- `cargo test -p feature-edge-server` passes.
