# B19: A dependency with non-boolean variant values blocks its dependents

| Field | Value |
|---|---|
| Type | Bug or design gap (**needs maintainer decision**) |
| Severity | Medium |
| Status | Reported by a verification agent with a code reference. Confirm intended semantics before changing anything. |
| Crate | `evaluation-engine` |
| Behavior change | Yes, if changed |
| Related | [B13](B13-dependency-bucketing-uses-root-key.md) (same code, do together) |

## Problem

The dependency pass test treats any non-boolean value as failed:

```rust
// evaluation-engine/src/lib.rs:739
if !dep_result.value.as_bool().unwrap_or(false) {
```

If dependency D is a multivariate flag that serves string, number or object values, every user who receives such a value fails D's check, and F is blocked with `DEPENDENCY_EVALUATION_FAILED`.

## Decide first

What does "F depends on D" mean for a non-boolean D? Options:
1. **D is enabled and served a value** (any variant, not a disabled or default result). Use the evaluation reason or variant, not the value.
2. **D's value is truthy** (non-empty string, non-zero number, and so on). Ad hoc; not recommended.
3. **Keep boolean-only**, but reject non-boolean dependencies at create/update time (`feature-toggle-backend/src/logic/dependency_graph.rs`), so users cannot configure a dependency that always fails.

Option 3 is the smallest change and does not change existing evaluations, except that saving such a configuration fails with a clear error. Check existing data for non-boolean dependencies before enforcing it.

## Tests

`evaluation-engine/tests/evaluation_tests.rs`: D with string variants and F depending on D. Assert the result the chosen semantics require. For option 3, add a backend logic test that rejects the dependency.

## Acceptance criteria

- Semantics documented in the engine and enforced by tests.
- `cargo test -p evaluation-engine` and `cargo test -p feature-toggle-backend` pass.
