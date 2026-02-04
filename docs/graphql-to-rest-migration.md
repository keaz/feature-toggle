# GraphQL to REST Migration Tasks (Incremental)

Goal: migrate domain-by-domain so each change is small, backend + UI are updated together, and we can commit a working state after each phase.

## 0) Global decisions (one-time)
- [x] REST base path/versioning: /api/v1
- [x] Pagination/filter conventions: offset + limit
- [x] Standard error envelope: { error, message, code, details } + HTTP status mapping
- [x] Streaming transport: WebSocket
- [x] Auth contract: no refresh tokens; login response matches current (token, isTemporary, user.id); token expiry uses current method
- [x] Keep /graphql during migration (optionally behind a flag)
- [x] OpenAPI docs paths: /api/v1/openapi.json + /docs

## 1) Shared REST foundation (one-time)
Backend
- [x] Add REST routing module (e.g., src/rest) and register routes in lib.rs
- [x] Add REST DTOs (serde) decoupled from GraphQL schema
- [ ] Port GraphQL validation rules to REST validators
- [x] Implement consistent error mapping to HTTP JSON responses
- [x] Add common paging helpers + query parsing helpers
- [x] Add utoipa OpenAPI docs (OpenApi derives + /openapi.json + Swagger UI endpoint)

Frontend
- [x] Add REST client wrapper (fetch/axios) with auth header + 401 handling
- [x] Add runtime config for REST base URL + stream URL
- [x] Add shared REST DTO types to replace GraphQL schema types

## 2) Phase A - Environments (backend + UI)
Backend
- [x] GET /teams/{teamId}/environments (filters + pagination)
- [x] GET /environments/{id}
- [x] POST /teams/{teamId}/environments
- [x] PATCH /environments/{id}
- [x] DELETE /environments/{id}
- [x] Add REST tests for environment endpoints
- [x] Apply GraphQL validator rules for environments
- [x] Use transaction manager pattern for environment write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in EnvironmentTable + CreateEnvironmentModal + DeleteConfirmationModal
- [x] Replace env GraphQL queries/mutations with REST calls
- [x] Remove env types from src/graphql/schema.ts usage
- [x] Update tests to mock REST for environment flows

Checkpoint
- [ ] Commit: env REST migration working

## 3) Phase B - Pipelines (backend + UI)
Backend
- [x] GET /teams/{teamId}/pipelines (filters + pagination)
- [x] GET /pipelines/{id}
- [x] POST /teams/{teamId}/pipelines
- [x] PATCH /pipelines/{id}
- [x] Add REST tests for pipeline endpoints
- [x] Apply GraphQL validator rules for pipelines
- [x] Use transaction manager pattern for pipeline write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in PipelineTable + PipelineCreate + PipelineFlow
- [x] Replace pipeline GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for pipeline flows

Checkpoint
- [ ] Commit: pipeline REST migration working

## 4) Phase C - Contexts (backend + UI)
Backend
- [x] GET /teams/{teamId}/contexts (filters + pagination)
- [x] GET /contexts/{id}
- [x] POST /teams/{teamId}/contexts
- [x] PATCH /contexts/{id}
- [x] DELETE /contexts/{id}
- [x] Add REST tests for context endpoints
- [x] Apply GraphQL validator rules for contexts
- [x] Use transaction manager pattern for context write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in ContextsPage + ContextCreate + ContextTable
- [x] Replace context GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for context flows

Checkpoint
- [ ] Commit: context REST migration working

## 5) Phase D - Clients (backend + UI)
Backend
- [x] GET /teams/{teamId}/clients (filters + pagination)
- [x] GET /clients/{id}
- [x] POST /teams/{teamId}/clients
- [x] PATCH /clients/{id}
- [x] Add REST tests for client endpoints
- [x] Apply GraphQL validator rules for clients
- [x] Use transaction manager pattern for client write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in ClientTable + ClientCreate
- [x] Replace client GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for client flows

Checkpoint
- [ ] Commit: client REST migration working

## 6) Phase E - Features + stages + rollout (backend + UI)
Backend
- [x] GET /teams/{teamId}/features (filters + pagination)
- [x] GET /features/{id}
- [x] POST /teams/{teamId}/features
- [x] PATCH /features/{id}
- [x] POST /features/{id}/emergency-disable
- [x] POST /features/{id}/emergency-enable
- [x] POST /stages/{id}/request-change (deploy/rollback)
- [x] GET /features/pending-approvals (team filter)
- [x] GET /features/active-kill-switches (team filter)
- [x] Add REST tests for feature endpoints
- [x] Apply GraphQL validator rules for features/stages/rollout
- [x] Use transaction manager pattern for feature/stage write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in FeatureTable + FeatureCreate + FeatureRollout
- [x] Replace feature GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for feature flows

Checkpoint
- [ ] Commit: feature REST migration working

## 7) Phase F - Stage criteria + rule groups (backend + UI)
Backend
- [x] GET /stages/{stageId}/criteria
- [x] PUT /stages/{stageId}/criteria (setStageCriteria)
- [x] PUT /criteria/{criteriaId}/variant-allocations (setVariantAllocations)
- [x] POST /rule-groups
- [x] PATCH /rule-groups/{id}
- [x] DELETE /rule-groups/{id}
- [x] Add REST tests for criteria/rule endpoints
- [x] Apply GraphQL validator rules for criteria/rule groups
- [x] Use transaction manager pattern for criteria/rule write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in CompoundRuleBuilder + related components
- [x] Replace criteria GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for criteria flows

Checkpoint
- [ ] Commit: criteria REST migration working

## 8) Phase G - Approvals + policies (backend + UI)
Backend
- [x] GET /teams/{teamId}/approval-requests (filters + pagination)
- [x] POST /approval-requests/{id}/approve
- [x] POST /approval-requests/{id}/reject
- [x] POST /approval-requests/{id}/cancel
- [x] GET /teams/{teamId}/approval-policies
- [x] GET /approval-policies/{id}
- [x] POST /teams/{teamId}/approval-policies
- [x] PATCH /approval-policies/{id}
- [x] DELETE /approval-policies/{id}
- [x] Add REST tests for approvals/policies
- [x] Apply GraphQL validator rules for approvals/policies
- [x] Use transaction manager pattern for approval write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in ApprovalsPage + ApprovalPoliciesPage + ApprovalPolicyFormModal
- [x] Replace approval GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for approvals

Checkpoint
- [ ] Commit: approvals REST migration working

## 9) Phase H - Teams, users, roles, auth (backend + UI)
Backend
- [x] GET /teams (admin all vs user-scoped)
- [x] POST /teams
- [x] PATCH /teams/{id}
- [x] GET /users (pagination + team filter)
- [x] GET /users/{id}
- [x] POST /users (register user)
- [x] POST /admins (create admin)
- [x] PATCH /users/{id}
- [x] POST /users/{id}/teams
- [x] POST /users/{id}/roles
- [x] GET /users/{id}/roles
- [x] GET /roles
- [x] POST /roles
- [x] DELETE /roles/{id}
- [x] POST /auth/login
- [x] POST /auth/logout
- [x] POST /auth/reset-password
- [x] POST /auth/users/{id}/temporary-password
- [x] GET /auth/status (replacement for applicationStatus)
- [x] Add REST tests for auth/users/roles/teams
- [x] Apply GraphQL validator rules for teams/users/roles/auth
- [x] Use transaction manager pattern for team/user/role/auth write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in Login + CreateAdmin + UserEdit + RolesPage + TeamsPage + header/team context
- [x] Replace auth + user GraphQL queries/mutations with REST calls
- [x] Update tests to mock REST for auth/users/roles

Checkpoint
- [ ] Commit: auth/users/roles/teams REST migration working

## 10) Phase I - Metrics + analytics + activity (backend + UI)
Backend
- [x] POST /teams/{teamId}/metrics (create metric)
- [x] GET /teams/{teamId}/metrics
- [x] GET /metrics/by-feature
- [x] GET /metrics/experiment-results
- [x] GET /metrics/evaluations/summary
- [x] GET /metrics/evaluations/rates
- [x] GET /metrics/evaluations/by-feature
- [x] GET /metrics/evaluations/count
- [x] GET /features/rollout-metrics (already implemented in Phase F)
- [x] GET /activity/recent
- [x] GET /metrics/feature-growth
- [x] Keep or relocate /metrics/track (align payload names with REST)
- [x] Add REST tests for metrics/activity
- [x] Apply GraphQL validator rules for metrics/activity
- [x] Use transaction manager pattern for metrics/activity write endpoints (repo_tx + *_tx)

Frontend
- [x] Replace GraphQL usage in dashboard hooks and components (metrics, alerts, activity, analytics)
- [x] Replace metrics GraphQL queries with REST calls
- [x] Update tests to mock REST for dashboard flows

Checkpoint
- [ ] Commit: metrics REST migration working

## 11) Phase J - Real-time updates (streaming)
Backend
- [x] Implement SSE or WebSocket endpoints for live updates:
  - evaluationSummary
  - evaluationRatesWithPeriod
  - evaluationsByFeatureLive
  - systemMetrics
  - recentActivities
  - featureGrowth
  - evaluationDashboard
  - approvalRequestsForTeam
- [x] Define auth for stream endpoints (JWT header or query token)
- [x] Reuse existing broadcast channels (evaluation_events_tx, approval_events_tx)
- [x] Verify transaction pattern coverage for any new write endpoints (if introduced)

Frontend
- [x] Replace GraphQL subscriptions with SSE/WS hooks
- [x] Update UI components to use stream hooks + REST initial loads

Checkpoint
- [ ] Commit: streaming REST migration working

## 12) Phase K - Cleanup
Backend
- [ ] Remove graphql module (query/mutation/schema/subscription/validator) once parity reached
- [ ] Remove async-graphql dependencies from Cargo.toml and Cargo.lock
- [ ] Delete GraphiQL endpoint and /graphql route
- [ ] Confirm all REST write endpoints use transaction manager pattern before cleanup

Frontend
- [ ] Remove @apollo/client, graphql, graphql-ws, @graphql-tools/* dependencies
- [ ] Remove src/graphql/* and Apollo client wiring
- [ ] Update runtime config keys (GRAPHQL_* -> REST_*)

Docs + scripts
- [ ] Update populate_test_data.js and perf-test scripts to REST
- [ ] Update DOCKER.md and other docs referencing /graphql

Checkpoint
- [ ] Commit: GraphQL removal and cleanup
