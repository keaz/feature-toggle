# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

FluxGate: a feature flag platform. Cargo workspace (edition 2024) with five crates, plus TypeScript API tests:

- `feature-toggle-backend` — control plane and source of truth. Actix REST admin API (`/api/v1`, default `:8080`) and tonic gRPC (default `:50051`). Binary `src/bin/export-contracts.rs` exports API contracts.
- `evaluation-engine` — pure flag evaluation library (single `src/lib.rs`). Used by both backend and edge server, so evaluation semantics change in one place only.
- `feature-edge-server` — low-latency evaluation gateway (default `:8081`). Exposes `/evaluate` and OFREP endpoints (`/ofrep/v1/evaluate/flags[/{key}]`). Subscribes to backend gRPC stream, caches features (moka LRU), batches evaluation events/assignments back to backend.
- `feature-toggle-shared` — shared constants.
- `fluxgate-cli` — the `fluxgate` command line client (library `fluxgate_cli` + binary). AWS-style profiles (`~/.fluxgate/config`, `~/.fluxgate/credentials`), cached login sessions with locked refresh (password, browser SSO with a loopback callback + PKCE, or device code approved at the UI `/device` page), and commands over the REST API (`fluxgate api` covers any endpoint). Pure HTTP client: no DB, builds without `DATABASE_URL`. Integration tests drive `fluxgate_cli::run` against `wiremock`. See `fluxgate-cli/README.md`.
- `api-tests/` — Jest + axios end-to-end tests against a running backend (pnpm).

## Commands

Backend needs PostgreSQL and `DATABASE_URL` (not part of TOML config). Local default used by compose/CI: `postgres://postgres:local123@localhost:5432/feature_toggle`.

```bash
cargo run -p feature-toggle-backend          # runs SQLx migrations on startup
cargo build                                  # needs DATABASE_URL or SQLX_OFFLINE=true
cargo clippy --all-targets
cargo fmt

# Tests
cargo test -p evaluation-engine              # pure, no DB
cargo test -p feature-edge-server
cargo test -p fluxgate-cli                   # no DB needed
cargo run -p fluxgate-cli -- --help
cargo test -p feature-toggle-backend         # needs migrated + seeded DB
cargo test -p feature-toggle-backend logic::policy::tests          # single module (lib unit tests)
cargo test -p feature-toggle-backend --test integration_test feature_test   # DB repository tests
cargo test -p feature-toggle-backend --test grpc_tests <name>
make test                                    # cargo test inside Docker with compose Postgres

# Seed DB the way CI does (migrations first, then fixtures)
sqlx migrate run --database-url "$DATABASE_URL" --source feature-toggle-backend/migrations
psql "$DATABASE_URL" -f init.sql

# API tests (Docker stack: Postgres + backend on :18080, migrate + seed, run, tear down)
pnpm --dir api-tests run test:docker
pnpm --dir api-tests run test:docker:keep   # keep stack for debugging
pnpm --dir api-tests exec jest feature.test.ts

# Contracts (OpenAPI + proto); baseline in feature-toggle-backend/contracts/baseline/
./scripts/export-contracts.sh
./scripts/check-contract-compat.sh
```

Docker: `make up` / `make down` / `make logs-backend`; image builds via `make build-backend`, `make build-edge` (see `DOCKER.md`).

## Key gotchas

- **SQLx offline cache**: backend uses `sqlx::query!` macros. Docker builds and contract scripts compile with `SQLX_OFFLINE=true` against `feature-toggle-backend/.sqlx/`. After adding or changing a checked query, regenerate it with `cargo sqlx prepare -- --all-targets` (run in `feature-toggle-backend/` with `DATABASE_URL` set) and commit the `.sqlx` changes, or Docker builds break. Keep `-- --all-targets`: the cache also covers queries in unit and integration tests, so `SQLX_OFFLINE=true cargo clippy --all-targets` works, and a plain `cargo sqlx prepare` deletes those entries as unused.
- **DB tests depend on seed data**: tests in `feature-toggle-backend/tests/database/` (wired through `tests/integration_test.rs` → `mod database;`) use hard-coded UUIDs from `init.sql`. Run migrations and then `init.sql` before running them.
- **Migrations**: add new files to `feature-toggle-backend/migrations/` with a timestamp prefix. Never edit an applied migration. `init.sql` holds seed fixtures only, not schema.
- **Contract compatibility**: changes to REST DTOs/OpenAPI or `proto/evaluation.proto` change contract hashes. `contract_compatibility_test` fails until the baseline is updated on purpose.
- `build.rs` compiles `proto/evaluation.proto` with vendored `protoc`; no system protoc needed. Edge server has its own `build.rs` for the same proto.
- Many clippy lints and `dead_code` are allowed at crate level in `feature-toggle-backend/Cargo.toml`.

## Backend architecture

Startup (`src/lib.rs::run`): load config → PG pool → migrations → build repositories and logic services by hand (no DI framework) → init JWT secret → spawn gRPC server → spawn schedulers → start Actix server. New services must be wired there.

Layers in `feature-toggle-backend/src/`:

- `database/` — repositories. Each one is a `#[automock]` trait (e.g. `FeatureRepository`) plus a `*RepositoryTx` extension trait whose methods take `&mut PgConnection`/transaction. Factories: `feature_repository(pool) -> Box<dyn FeatureRepository>` and `feature_repository_tx(pool)` for the concrete impl. DB row types live in `database/entity.rs`.
- `logic/` — domain services. Pairs of `x.rs` (service trait + impl, pool-based reads) and `x_tx.rs` (free functions `*_in_tx` that perform multi-repository writes inside one transaction, including activity-log entries). `logic::ActorContext` carries who performed the action.
- `rest/` — Actix handlers, request/response DTOs, utoipa OpenAPI registration (all centralized in `rest/mod.rs`; OpenAPI at `/api/v1/openapi.json`, Swagger at `/docs`). Write handlers open a transaction (`TransactionManager::begin` / `pool.begin()`), call `logic::*_tx::*_in_tx`, then commit. `transaction_purity_test` checks that tx methods see uncommitted writes and roll back cleanly.
- `model.rs` — API-facing domain model, mapped to and from `database::entity` types.
- `middleware/` — `JwtGuard` (JWT validation against persisted tokens, system client tokens), `AdminGuard` (bootstrap admin only when none exists), access log. Authorization decisions go through `logic/policy.rs` and `logic/authorization.rs`, and are audited in the activity log.
- `grpc/` — service for the edge server: feature fetch, client info, streaming snapshots and incremental updates, evaluation/assignment ingest (idempotent via ingest fingerprint).
- `scheduler/` — background jobs: kill-switch rollback, scheduled changes, auto-approval, metrics aggregation, canary governance, expired session-token cleanup.
- `cluster/` — optional multi-node replication and DB-backed discovery.

Change propagation: `tokio::sync::broadcast` channel of `grpc::pb::FeatureUpdate` is shared by REST handlers, schedulers, context logic, and gRPC streaming. Any write that changes evaluable feature state must send an update on it, or edge caches go stale. A second broadcast channel carries `FeatureEvaluationEvent` for REST live streams (`rest/stream.rs`).

Rollout safety: `logic/dependency_graph.rs` validates feature dependencies (cycle detection) on create/update, and stage deployments are blocked when a dependency is missing, disabled, or not deployed in the target environment. Approval workflows (`logic/approval.rs`) can gate stage changes.

## Configuration

Backend config (`src/config.rs`) loads first match of: `$FEATURE_TOGGLE_CONFIG`, `feature-toggle-backend/config.toml`, `./config.toml`. Keys: `allowed_origin`, `http_addr`, `grpc_addr`, optional `[cluster]`, optional `[auth]` (`access_token_ttl_minutes` default 30, `refresh_token_ttl_days` default 7), optional `public_base_url` (public backend URL, used for SSO callback URLs; see `docs/sso.md`). SSO secrets come from env: `FLUXGATE_ENCRYPTION_KEY` (base64, 32 bytes) and `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`. The root `config.toml` is the **edge server** config (backend gRPC address, client credentials, flush/retry/cache settings); see `feature-edge-server/CONFIG.md`.

## graphify

This project has a graphify knowledge graph at graphify-out/.

Rules:
- Before answering architecture or codebase questions, read graphify-out/GRAPH_REPORT.md for god nodes and community structure
- If graphify-out/wiki/index.md exists, navigate it instead of reading raw files
- For cross-module "how does X relate to Y" questions, prefer `graphify query "<question>"`, `graphify path "<A>" "<B>"`, or `graphify explain "<concept>"` over grep — these traverse the graph's EXTRACTED + INFERRED edges instead of scanning files
- After modifying code files in this session, run `graphify update .` to keep the graph current (AST-only, no API cost)
