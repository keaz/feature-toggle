# B02: Duplicate-key checks reject valid keys because they use substring match

| Field | Value |
|---|---|
| Type | Bug (correctness) |
| Severity | Medium |
| Status | Confirmed (code traced) |
| Crate | `feature-toggle-backend` |
| Behavior change | Yes, intended: keys that only contain an existing key become allowed |
| Related | [B01](B01-feature-key-lookup-substring-match.md) (do B01 first and reuse its exact-key method) |

## Problem

Create and rename conflict checks call `get_features(team_id, Some(key), None)`, which filters with `f.key ILIKE '%key%'`:

```rust
// feature-toggle-backend/src/database/feature.rs:1852 (check_feature_exists, used on create)
let existing_feature = self
    .get_features(input.team_id, Some(input.key.clone()), None)
    .await;
if let Ok(existing_feature) = existing_feature && !existing_feature.is_empty() {
    return Err(Error::RecordAlreadyExists(..));
}
```

```rust
// feature-toggle-backend/src/rest/feature.rs:248 (key-change conflict check)
let existing = logic.get_features(feature.team_id.clone(), Some(key.to_string()), None).await?;
let has_conflict = existing.iter().any(|item| item.id != *feature_id);
```

Two more defects in `check_feature_exists`:
- A DB error is ignored (`if let Ok(..)`), so the check passes silently. The DB `UNIQUE (key, team_id)` constraint still blocks real duplicates.
- `_` and `%` in the key act as wildcards.

## How it fails

1. Team has flag `checkout`.
2. User creates flag `check`.
3. `ILIKE '%check%'` matches `checkout`, and the create fails with "Feature with key 'check' already exists".

The same happens when renaming a flag to `pay` while `payment` exists.

## Fix

1. Use an exact-key lookup in both places. Use the method B01 adds (`get_feature_by_key`).
2. Decide on case handling. Today the check is case-insensitive, so `Checkout` is rejected when `checkout` exists. Keep that rule by comparing `lower(f.key) = lower($2)` in the conflict checks only. This keeps current behavior for case variants and removes only the substring false positives.
3. In `check_feature_exists`, propagate DB errors instead of ignoring them.

## Behavior impact

- Keys that contain an existing key as a substring become allowed (the fix).
- Case-variant duplicates stay rejected (no change).

## Tests

- DB or logic test: with `checkout` present, creating `check` succeeds and creating `checkout` fails with `RecordAlreadyExists`. Creating `Checkout` also fails.
- REST rename test in `feature-toggle-backend/src/rest/feature.rs` tests (uses `expect_get_features` mocks today at about line 2439). Update the mocks to the new method.
- `api-tests/`: create `check` after `checkout` and expect 201.

## Acceptance criteria

- Neither conflict check calls `get_features` with a key filter.
- `cargo test -p feature-toggle-backend` and `pnpm --dir api-tests run test:docker` pass.
