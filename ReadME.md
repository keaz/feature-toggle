# FluxGate Backend Workspace

This repository contains the backend implementation of the FluxGate feature flag management system. The main service is the Rust backend in `feature-toggle-backend`, but the workspace also includes the evaluation engine, edge server, shared types, and API tests that support the full control plane and data plane.

The backend is responsible for:

- managing teams, environments, contexts, clients, features, stages, criteria, variants, approvals, and notifications
- authenticating users and system clients
- enforcing RBAC and team-scoped authorization policies
- serving the REST admin API and OpenAPI spec
- serving the gRPC API used by the edge server
- broadcasting feature updates to connected edge nodes
- collecting and aggregating evaluation and experiment metrics
- enforcing rollout safety, kill switches, approval workflows, and canary rollback gates

## Workspace layout

The workspace is defined in [Cargo.toml](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/Cargo.toml) and currently contains four Rust crates:

- `feature-toggle-backend`: the control-plane service and source of truth
- `evaluation-engine`: the pure evaluation library used by backend and edge
- `feature-edge-server`: the low-latency evaluation gateway that subscribes to backend updates
- `feature-toggle-shared`: shared types/utilities used across crates

There is also an `api-tests` folder for end-to-end API coverage and a `docs` folder for contract and integration documentation.

## Backend architecture

The backend entrypoint is [main.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/main.rs), which delegates to [lib.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/lib.rs). On startup the service:

1. loads configuration
2. opens a PostgreSQL pool from `DATABASE_URL`
3. runs SQLx migrations automatically
4. constructs repositories and logic services
5. initializes JWT secret state
6. starts the gRPC server on a separate task
7. starts background schedulers for rollback, metrics, approvals, and canary governance
8. starts the Actix REST server

Implementation is organized into these layers:

- `src/database`: SQLx-backed repositories and database-facing models
- `src/logic`: application services and domain workflows
- `src/rest`: REST handlers, DTOs, OpenAPI registration, and websocket streams
- `src/grpc`: gRPC service definitions and backend-to-edge streaming
- `src/middleware`: access logging, admin bootstrap guard, and JWT/policy enforcement
- `src/scheduler`: recurring jobs for operational workflows
- `src/cluster`: optional multi-node replication and discovery

## API surface

### REST API

The admin API is exposed under `/api/v1`. Route registration is centralized in [rest/mod.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/rest/mod.rs).

The REST surface covers:

- authentication and password flows
- teams, users, roles, and system clients
- environments, contexts, clients, and pipelines
- feature CRUD, stage changes, kill switches, rollout metrics, and approvals
- criteria/rule groups/variant allocations
- metrics ingestion and analysis
- canary gate configuration and manual analysis
- notifications and live stream endpoints

OpenAPI is generated directly from code and exposed through:

- `GET /api/v1/openapi.json`
- `GET /docs`

### gRPC API

The backend also serves gRPC on port `50051` by default. It is used primarily by the edge server for:

- feature fetch by key
- client metadata lookup
- streaming feature snapshots and incremental updates
- evaluation event and assignment flushes
- metric tracking

The gRPC implementation lives in [grpc/mod.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/grpc/mod.rs).

## Feature flag implementation details

### Evaluation model

Flag evaluation logic is implemented in the shared [evaluation-engine](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/evaluation-engine/src/lib.rs). The backend maps database entities into engine features and uses the same model as the edge server, which keeps evaluation behavior consistent.

The evaluator supports:

- simple and contextual features
- environment-specific stages
- ordered criteria and rule groups
- weighted variant allocation and specific variant selection
- deterministic bucketing
- dependency graph evaluation
- structured dependency-block reasons in evaluation metadata

### Rollout safety

Recent backend changes added stronger rollout controls:

- dependency graph validation during feature create/update
- cycle detection for feature dependencies
- stage deployment requests blocked when required dependencies are missing, disabled, or not deployed in the target environment
- explicit dependency-block reason codes returned by the evaluator

The rollout safety logic lives in [dependency_graph.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/logic/dependency_graph.rs) and is enforced during stage changes in [feature.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/rest/feature.rs).

### Kill switch and rollback

The backend supports emergency disable/enable flows and scheduled rollback enforcement. The rollback scheduler runs in the background and broadcasts feature updates so connected edges can invalidate stale state.

### Canary governance

The backend now supports canary analysis gates stored in the database and evaluated by a scheduler. A gate can compare baseline vs canary variants using a metric, minimum sample size, direction, and regression threshold. Failed gates can automatically trigger the existing emergency disable path.

Relevant files:

- [canary.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/logic/canary.rs)
- [metrics.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/rest/metrics.rs)
- [canary_governance.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/scheduler/canary_governance.rs)
- [20260310000000_rollout_canary_governance.sql](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/migrations/20260310000000_rollout_canary_governance.sql)

## Authentication, authorization, and auditing

The backend has two primary caller types:

- human users authenticated with JWTs
- system clients authenticated for machine-to-machine access

Middleware and policy enforcement live in:

- [admin_guard.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/middleware/admin_guard.rs)
- [jwt_guard.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/middleware/jwt_guard.rs)
- [policy.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/logic/policy.rs)

The implementation includes:

- bootstrap admin creation only when no admin exists yet
- JWT token validation against persisted token records
- system client token support
- centralized policy checks for privileged admin and team-scoped mutations
- audit entries for policy allow/deny decisions and operational events

Activity and audit data are persisted through the activity log repository under `src/database/activity_log.rs`.

## Metrics and experiments

The backend stores evaluation and experiment data and exposes analytics APIs for the UI. The metrics subsystem supports:

- raw metric event tracking
- feature evaluation summaries and rates
- experiment result queries
- feature growth and recent activity views
- scheduled metric aggregation

The main implementation is in [metrics.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/logic/metrics.rs) and [rest/metrics.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/rest/metrics.rs).

## Configuration

The backend config model is defined in [config.rs](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/src/config.rs). Configuration is loaded in this order:

1. `FEATURE_TOGGLE_CONFIG`
2. `feature-toggle-backend/config.toml`
3. `config.toml` in the current working directory

The backend currently reads:

- `allowed_origin`
- `http_addr`
- `grpc_addr`
- optional cluster settings

Database connectivity is not part of the TOML config. It must be provided through `DATABASE_URL`.

Example backend config: [feature-toggle-backend/config.toml](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/config.toml)

## Local development

### Prerequisites

- Rust toolchain
- PostgreSQL
- `DATABASE_URL` pointing to a writable database

### Run locally

From the workspace root:

```bash
export DATABASE_URL=postgres://postgres:password@localhost:5432/feature_toggle
cargo run -p feature-toggle-backend
```

The backend will:

- create the connection pool
- run migrations from `feature-toggle-backend/migrations`
- start HTTP on `0.0.0.0:8080` by default
- start gRPC on `0.0.0.0:50051` by default

### Seed test data

If you want the database populated with the provided seed fixtures after migrations:

```bash
sqlx migrate run --database-url "$DATABASE_URL" --source feature-toggle-backend/migrations
psql "$DATABASE_URL" -f init.sql
```

### Docker

The Docker image and compose setup are already wired for the backend. Relevant files:

- [feature-toggle-backend/Dockerfile](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/Dockerfile)
- [docker-compose.yml](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/docker-compose.yml)
- [backend-entrypoint.sh](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/scripts/backend-entrypoint.sh)

The container entrypoint copies config, runs SQLx migrations, and then starts the backend binary.

## Contracts and SDK generation

This repository now includes contract export and compatibility tooling for SDK generation.

Use:

```bash
./scripts/export-contracts.sh
./scripts/check-contract-compat.sh
```

Generated artifacts include:

- OpenAPI JSON
- protobuf source
- protobuf descriptor set
- a baseline hash manifest for compatibility checks

See [contract-driven-sdk.md](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/docs/contract-driven-sdk.md) for TypeScript and Java SDK generation examples.

## Testing

Useful targeted commands:

```bash
cargo test -p feature-toggle-backend
cargo test -p evaluation-engine
cargo test -p feature-edge-server
```

Focused checks recently added for backend safety work include:

```bash
cargo test -p feature-toggle-backend logic::policy::tests
cargo test -p feature-toggle-backend logic::canary::tests
cargo test -p feature-toggle-backend dependency_graph
./scripts/check-contract-compat.sh
```

## Migration history

The backend schema has evolved from the initial feature/pipeline model into a larger platform that now includes:

- users, roles, user-team membership, JWT tokens, and JWT secret rotation
- feature evaluation storage and analytics
- kill switch support
- feature variants and ordered criteria
- compound targeting rules and variant allocations
- approval workflows
- notifications
- system clients
- canary governance

The migrations live in [feature-toggle-backend/migrations](/Users/kasunranasinghe/Projects/FeatureToggle/feature-toggle/feature-toggle-backend/migrations).

## Summary

If you are working on the control plane, start in `feature-toggle-backend`. If you are working on evaluation semantics, start in `evaluation-engine`. If you are working on low-latency delivery behavior, follow the integration between backend gRPC streaming and `feature-edge-server`.

This workspace is no longer just a CRUD API for feature definitions. It is the backend implementation of a feature flag platform with policy enforcement, rollout governance, live delivery, metrics, and contract-driven integration support.
