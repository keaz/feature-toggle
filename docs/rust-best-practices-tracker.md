# Rust Best Practices Tracker

This document tracks non-database Rust best-practice improvements identified during the repository review.

## Completed in this pass

- [x] Reduced non-DB Clippy warnings in `evaluation-engine`.
- [x] Reduced non-DB Clippy warnings in `feature-edge-server`.
- [x] Implemented `FromStr` support for `StageStatus` in `feature-toggle-shared` and switched tests to trait-backed parsing.
- [x] Removed backend non-DB warnings for unused imports/variables and unnecessary `mut` bindings.
- [x] Reworked transaction flow in key repository methods to stop ignoring commit failures:
  - `feature-toggle-backend/src/database/feature.rs`
  - `feature-toggle-backend/src/database/pipeline.rs`
- [x] Stopped dropping DB write errors during client web-origin writes:
  - `feature-toggle-backend/src/database/client.rs`

## Remaining items

### P0

- [ ] SQLx compile-time schema mismatch for `clients.environment_id` during Clippy/build.
  - Scope: `feature-toggle-backend` SQLx macro queries.
  - Note: explicitly excluded from this pass per request.

### P1

- [ ] Remove panic-prone runtime paths (`unwrap`/`expect`) in non-test code and return typed errors.
  - Candidate modules:
    - `feature-toggle-backend/src/logic/*`
    - `feature-toggle-backend/src/database/mod.rs`
    - `feature-toggle-backend/src/cluster/mod.rs`
    - `feature-edge-server/src/handlers.rs`

- [ ] Unify duplicated repository logic between standard and transactional implementations.
  - Candidate modules:
    - `feature-toggle-backend/src/database/feature.rs`
    - `feature-toggle-backend/src/database/pipeline.rs`
    - `feature-toggle-backend/src/database/client.rs`

### P2

- [ ] Enforce formatting and linting in CI (`cargo fmt --check`, `cargo clippy`).
- [ ] Remove stale backup artifact `feature-edge-server/src/main.rs.backup` after confirming it is not needed.

## Suggested validation commands

```bash
cargo fmt --all -- --check
cargo clippy -p evaluation-engine --all-targets
cargo clippy -p feature-edge-server --all-targets
cargo clippy -p feature-toggle-shared --all-targets
# backend currently expected to fail only on DB-schema-related SQLx errors
cargo clippy -p feature-toggle-backend --all-targets
```
