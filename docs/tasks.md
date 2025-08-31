# Feature Toggle Platform Improvement Tasks

A logically ordered, actionable checklist covering architectural and code-level improvements. Check items off as they are completed.

1. [ ] Define and document system architecture boundaries
   - [ ] Extract a high-level architecture diagram (backend, edge server, evaluation engine, DB, clients) and add to docs/architecture.md
   - [ ] Clearly define responsibilities and data contracts between crates: feature-toggle-backend, feature-edge-server, evaluation-engine, feature-toggle-shared
   - [ ] Introduce crate-level READMEs summarizing purpose, APIs, and dependencies

2. [ ] Establish configuration and environment management standards
   - [ ] Adopt a single configuration crate/module using config + envy with typed structs and defaults
   - [ ] Document required env vars for each service (ports, DB URL, session secrets, CORS, gRPC bind address)
   - [ ] Fail-fast on invalid/missing configuration with clear error messages

3. [ ] Improve error handling and result types across services
   - [ ] Standardize Error enums with thiserror and map to HTTP/gRPC/GraphQL error surfaces consistently
   - [ ] Ensure sensitive/internal errors are not leaked to clients; add user-facing codes/messages
   - [ ] Add tracing-friendly error contexts (anyhow/context or eyre with tracing-error)

4. [ ] Logging, tracing, and observability
   - [ ] Adopt tracing + tracing-subscriber with JSON output option and request IDs
   - [ ] Propagate trace IDs across Actix, async-graphql, and tonic gRPC
   - [ ] Add key spans: DB queries, GraphQL resolvers, gRPC handlers, evaluation steps
   - [ ] Expose Prometheus metrics (HTTP/gRPC latency, SQLx query duration, cache hits, evaluation outcomes)

5. [ ] Database schema, constraints, and migrations
   - [ ] Review foreign keys and ON DELETE behavior for features_pipeline_stages/parent relationships; add missing indexes
   - [ ] Ensure uniqueness constraints where required (feature keys per team, client ids, context keys)
   - [ ] Convert ad-hoc init.sql operations into idempotent sqlx migrations where appropriate
   - [ ] Add NOT NULL and CHECK constraints for enum-like columns (e.g., stage status)
   - [ ] Add migration tests or verification step in CI to apply and rollback

6. [ ] SQL performance and N+1 mitigation
   - [ ] Audit sqlx queries for N+1 patterns in GraphQL resolvers (batch load with lookahead or dataloader)
   - [ ] Add appropriate B-Tree/GIN indexes (feature_key, team_id, environment_id, context_key)
   - [ ] Use EXPLAIN ANALYZE on heavy queries and document findings

7. [ ] GraphQL API hardening and ergonomics
   - [ ] Validate inputs (IDs, enums, pagination params) and return typed errors
   - [ ] Implement consistent pagination (cursor-based where applicable) and default limits
   - [ ] Fix TODOs and replace test stubs with real logic or feature-gated mocks
   - [ ] Add field resolvers where lookahead is used to avoid over-fetching
   - [ ] Add auth/authorization checks (admin/user/team scoping) via middleware/guards

8. [ ] gRPC API evolution and compatibility
   - [ ] Freeze current .proto in a versioned directory; introduce semantic versioning process
   - [ ] Add comments on deprecations (already present for feature_id); add migration timeline
   - [ ] Generate code in build.rs and enforce regeneration checks in CI
   - [ ] Define error codes mapping (tonic::Status) and ensure consistent use

9. [ ] Evaluation engine correctness and performance
   - [ ] Add unit tests for bucketing stability (sticky key changes, boundary conditions 0/100)
   - [ ] Add property tests for rollout distribution and monotonicity
   - [ ] Benchmark evaluate() with Criterion; optimize hot paths (hashing, allocations)
   - [ ] Support optional seed for deterministic test scenarios
   - [ ] Document expected behavior for missing context/bucketing keys

10. [ ] Feature dependency handling
    - [ ] Add cycle detection for dependencies at creation/update time
    - [ ] Provide clear errors for missing/disabled dependencies during evaluation
    - [ ] Add integration tests for multi-level dependency trees

11. [ ] Concurrency, transactions, and consistency
    - [ ] Review transaction boundaries in repository methods (create/update/delete feature and stages)
    - [ ] Add appropriate isolation levels and retries for serialization conflicts
    - [ ] Consider optimistic locking (version column) for concurrent updates

12. [ ] Caching strategy
    - [ ] Introduce in-memory cache for feature-by-key lookups in backend with TTL + invalidation on writes
    - [ ] Evaluate sidecar/edge snapshot bootstrapping path and delta updates (already in proto); document semantics
    - [ ] Add cache metrics and hit/miss counters

13. [ ] Security hardening
    - [ ] Store and rotate session/auth secrets securely; avoid default dev secrets in production
    - [ ] Validate and hash passwords with argon2 parameters from config; add password policy
    - [ ] Implement client credential validation in gRPC handlers; throttle failed attempts
    - [ ] Add input validation to prevent injection in SQL/GraphQL (use bind parameters everywhere)
    - [ ] Add rate limiting on public endpoints (IP + client-based)

14. [ ] Access control and multi-tenancy
    - [ ] Enforce team scoping on all data access paths (GraphQL and gRPC)
    - [ ] Add middleware to inject team context derived from session/client credentials
    - [ ] Add tests to ensure cross-team data access is impossible

15. [ ] API documentation and SDKs
    - [ ] Generate GraphQL schema documentation (SDL) and publish in docs
    - [ ] Document gRPC services with examples; provide sample curl/grpcurl invocations
    - [ ] Create a minimal client SDK shape (shared types) in feature-toggle-shared and publish crate docs

16. [ ] Testing strategy
    - [ ] Add unit tests for repositories using sqlx::test or a test DB container
    - [ ] Integration tests for GraphQL queries/mutations with Actix test server
    - [ ] gRPC integration tests for Evaluate/GetFeatureByKey and streaming paths
    - [ ] Golden tests for evaluation-engine inputs/outputs
    - [ ] Seeded test data via migrations/fixtures instead of ad-hoc init.sql

17. [ ] CI/CD pipeline
    - [ ] Add GitHub Actions (or preferred) with steps: fmt, clippy (deny warnings), build, test, audit, coverage
    - [ ] Add sqlx offline feature with .sqlx data and verify in CI
    - [ ] Build and push Docker images with reproducible tags; scan images for vulnerabilities

18. [ ] Code quality and style
    - [ ] Enforce rustfmt and clippy across workspace; fix existing lints
    - [ ] Break up large modules (e.g., database/feature.rs) into smaller cohesive modules
    - [ ] Replace magic strings/enums with typed enums/newtypes
    - [ ] Add doc comments for public items and examples for complex functions

19. [ ] Runtime robustness
    - [ ] Add timeouts and cancellation for HTTP/gRPC handlers and DB calls
    - [ ] Implement retries with backoff for transient DB errors
    - [ ] Graceful shutdown hooks for Actix and tonic servers; drain in-flight requests

20. [ ] Docker and local development DX
    - [ ] Ensure docker-compose sets sane defaults (healthchecks, resource limits)
    - [ ] Add Makefile or justfile for common tasks (run, test, lint, migrate)
    - [ ] Hot-reload/dev containers with bind mounts and cargo-watch

21. [ ] Data export/import and backups
    - [ ] Provide minimal export/import scripts for features and contexts (team-scoped)
    - [ ] Document backup/restore procedures and retention

22. [ ] Pagination and filtering consistency
    - [ ] Standardize pagination (cursor or offset) and expose consistent parameters across list endpoints
    - [ ] Add total count endpoints or include page info in GraphQL types

23. [ ] De-dup shared types and contracts
    - [ ] Move shared DTOs between backend and edge into feature-toggle-shared
    - [ ] Align evaluation-engine input/output structs with gRPC/GraphQL contracts

24. [ ] Backwards compatibility and migrations
    - [ ] Plan removal of deprecated proto fields (feature_id) and add adapters during transition
    - [ ] Write data migrations to populate any new required columns with defaults

25. [ ] Feature flags for platform features
    - [ ] Use the product to control rollout of new APIs and evaluation changes; add self-hosted flags

26. [ ] Housekeeping and documentation
    - [ ] Update ReadME.md to reflect multi-crate workspace, how to run each service, and dependency versions
    - [ ] Add CONTRIBUTING.md with coding standards and PR checklist
    - [ ] Add CODEOWNERS and issue templates to guide contributions
