# Investigation: SSO, Jira integration, client SDKs

Date: 2026-10-01. Scope: how to add (1) SSO authentication and SSO-driven roles, (2) Jira-driven rollout/rollback on top of the existing REST API, (3) complete client libraries for Java/Spring Boot, Rust and JavaScript (vanilla, React, Angular).

Paths: `BE` = `feature-toggle/feature-toggle-backend/`, `EDGE` = `feature-toggle/feature-edge-server/src/`, `ENGINE` = `feature-toggle/evaluation-engine/`, `UI` = `feature-toggle-ui/src/`, `SB` = `fluxgate-springboot/src/main/java/com/fluxgate/starter/`.

---

## 0. Prerequisites found during the investigation

These are existing defects that each feature below depends on. Fix them first.

| # | Defect | Where | Blocks |
|---|--------|-------|--------|
| P1 | Login never checks `users.enabled`; `JwtGuard` does not either; disabling a user does not revoke tokens (`revoke_all_user_tokens` is never called). | `BE/src/logic/user.rs:291-330`, `BE/src/middleware/jwt_guard.rs:367-488` | SSO deprovisioning |
| P2 | Roles are copied into the JWT at login and live 24h (hardcoded). Role changes apply only after re-login. No refresh token. | `jwt_guard.rs:704-731` | SSO role sync |
| P3 | JWT has no `iss`/`aud`; single HS256 key; rotating it logs everyone out. `jwt_secret` in `BE/config.toml` is unused. | `jwt_guard.rs:16-32, 347-350` | Hardening only |
| P4 | System-client management routes (`/teams/{id}/system-clients`, tokens) have no permission check; any logged-in user can mint an M2M token. | `BE/src/rest/system_client.rs` | Jira integration |
| P5 | System-client JWT carries `is_admin: true`; handler-level `is_admin` checks therefore pass for M2M callers (only `PolicyActor` forces false). The shadow `users` row is also `is_admin=TRUE`, and is counted by `admin_exists`. | `jwt_guard.rs:733-759`, `BE/src/database/system_client.rs:195-237` | Jira integration |
| P6 | Scheduled `ENABLE_FEATURE`/`DISABLE_FEATURE` call emergency logic directly, bypassing the role/policy checks on the emergency endpoints. Any JWT (including a `flag:write` system client) can create one. | `BE/src/scheduler/scheduled_changes.rs`, `BE/src/rest/operational_safety.rs:1132-1248` | Jira integration |
| P7 | `PUT /criteria/{id}/variant-allocations` does not broadcast to edge, write a version, or log activity. | `BE/src/rest/criteria.rs:652-726` | Jira % rollout |
| P8 | No self-approval check on approval votes. | `BE/src/logic/approval.rs:468-517` | Jira approvals |
| P9 | OFREP auth is broken: edge passes the header value as `client_id` with an empty secret; backend rejects empty secret; edge returns 502. (Review item B05/B06.) | `EDGE/handlers.rs:850-869`, `BE/src/grpc/mod.rs:984-986` | All SDKs |
| P10 | Edge has no CORS handling. | `EDGE/main.rs:431-446` | Browser SDK |
| P11 | Spring starter sends an old request shape (`feature_key`, list context, body credentials) and reads `enabled`; current edge expects `{flagKey, context:{bucketingKey,...}}` and returns `{value, variant, reason}`. The starter does not work against today's edge. WireMock stubs hide this. | `SB/FeatureEvaluationRequest.java:13-18`, `SB/FeatureEvaluationResponse.java:12` | Java SDK |

---

## 1. SSO integration

### 1.1 Current state

- Username/password only. Argon2 hashes, `POST /api/v1/auth/login` issues an HS256 JWT (24h), its SHA-256 hash is stored in `jwt_tokens` and checked on every request.
- `users.password_hash` is `NOT NULL`. No external-identity columns. No OIDC/SAML/LDAP code, crates or docs anywhere.
- Authorization: superuser flag `users.is_admin`; global roles (`Approver`, `Requester`, `Team Admin`, plus custom roles that only matter in approval policies); team membership in `user_teams`. Route policy table in `BE/src/logic/policy.rs:215-307`.
- `user_roles` and `user_teams` assignment replaces the whole set (`DELETE` then `INSERT`).
- UI stores the token in `localStorage`, decodes it with `jwt-decode`.

### 1.2 Recommended approach: OIDC, backend-for-frontend, keep FluxGate JWT

The backend performs the OIDC Authorization Code flow (confidential client + PKCE), then issues the **existing** FluxGate JWT. `JwtGuard`, `policy.rs`, approvals and the UI token handling stay unchanged. Only login changes.

Why OIDC first:
- Okta, Microsoft Entra ID, Google Workspace, Auth0, Keycloak, Ping and OneLogin all speak OIDC.
- Rust support is mature (`openidconnect` crate: discovery, JWKS, nonce, PKCE). `reqwest` is already a dependency.
- SAML-only IdPs (e.g. ADFS) can be fronted by a broker (Keycloak, Dex, Authentik) that exposes OIDC. Native SAML (`samael`) needs `libxmlsec1` in the image and is not worth it until a customer requires it.

Flow:

```
UI  "Sign in with <provider>"
 └─> GET  /api/v1/auth/sso/{provider}/authorize        (public)
       backend: create state + nonce + PKCE verifier, store server-side (DB row, 10 min TTL)
       302 -> IdP /authorize
IdP ─> GET  /api/v1/auth/sso/{provider}/callback?code&state   (public)
       backend: verify state, exchange code, validate id_token (iss, aud, exp, nonce, JWKS sig)
                resolve/provision user, sync roles+teams from claims (1.3)
                issue FluxGate JWT, store hash in jwt_tokens (existing path)
                create one-time login code (60 s, single use)
       302 -> <ui>/auth/sso/complete?code=<one-time>
UI  ─> POST /api/v1/auth/sso/exchange {code}            (public)
       -> {user, token}   same shape as /auth/login
```

Do not put the JWT itself in the redirect URL (it leaks into browser history and proxy logs).

### 1.3 Data model changes (new migrations)

```sql
-- IdP configuration, managed in admin UI (same pattern as JWT settings)
CREATE TABLE sso_providers (
  id UUID PRIMARY KEY,
  slug VARCHAR(50) UNIQUE NOT NULL,          -- used in URLs
  display_name VARCHAR(100) NOT NULL,
  issuer_url TEXT NOT NULL,                  -- discovery: <issuer>/.well-known/openid-configuration
  client_id TEXT NOT NULL,
  client_secret_enc TEXT NOT NULL,           -- encrypted at rest, or env-var reference
  scopes TEXT[] NOT NULL DEFAULT '{openid,email,profile}',
  groups_claim TEXT NOT NULL DEFAULT 'groups', -- JSON path, e.g. 'realm_access.roles'
  allowed_email_domains TEXT[],
  jit_provisioning BOOLEAN NOT NULL DEFAULT TRUE,
  role_sync_mode VARCHAR(20) NOT NULL DEFAULT 'authoritative', -- authoritative | additive | off
  enabled BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Link users to IdP subjects. Match on (provider, subject), never on email alone.
CREATE TABLE user_identities (
  id UUID PRIMARY KEY,
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider_id UUID NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
  subject TEXT NOT NULL,
  email TEXT,
  last_login TIMESTAMPTZ,
  UNIQUE (provider_id, subject)
);

ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;   -- SSO-only users
ALTER TABLE users ADD COLUMN auth_source VARCHAR(20) NOT NULL DEFAULT 'local'; -- local | sso | system

CREATE TABLE sso_login_states (...);   -- state, nonce, pkce_verifier, provider_id, expires_at
CREATE TABLE sso_login_codes (...);    -- one-time code -> token hash, expires_at, used_at
```

`authenticate_user` must reject a `NULL` hash (SSO-only users cannot use password login).

Account linking: on first SSO login, if no identity row exists and a local user has the same email, link only when the IdP reports `email_verified = true` **and** the provider allows linking. Otherwise create a new user (JIT) or reject (JIT off).

### 1.4 Handling roles via SSO

Mapping table:

```sql
CREATE TABLE sso_group_mappings (
  id UUID PRIMARY KEY,
  provider_id UUID NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
  group_value TEXT NOT NULL,                 -- value seen in the groups claim
  target_type VARCHAR(10) NOT NULL,          -- 'role' | 'team' | 'admin'
  target_id UUID,                            -- roles.id or teams.id; NULL for 'admin'
  UNIQUE (provider_id, group_value, target_type, target_id)
);

ALTER TABLE user_roles ADD COLUMN source VARCHAR(10) NOT NULL DEFAULT 'manual'; -- manual | sso
ALTER TABLE user_teams ADD COLUMN source VARCHAR(10) NOT NULL DEFAULT 'manual';
```

On every SSO login, inside one transaction:
1. Read the groups claim (configurable path; string or array).
2. Compute desired roles, teams, and `is_admin` from `sso_group_mappings`.
3. `authoritative`: delete `source='sso'` rows not in the desired set, insert missing ones; set `is_admin` from mapping. Manual rows are kept.
   `additive`: insert only. `off`: do nothing.
4. Write an `activity_log` entry (`sso_role_sync`) with the diff.
5. Issue the JWT with the new roles.

Required code changes:
- `BE/src/database/role.rs` and `BE/src/database/user.rs` replace-the-whole-set assignment must only replace `source='manual'` rows; otherwise a manual admin edit wipes SSO roles and vice versa.
- UI `UserEdit.tsx`: show SSO-managed roles/teams as locked with an "from <provider>" badge.
- Fix **P1** and **P2** first. Without them, a user removed from the IdP group keeps admin/approver rights for up to 24h, and a disabled user can still log in. Recommended: access token TTL 15-60 min plus a refresh token (`jwt_tokens.token_type = 'refresh'`); on refresh, re-check `enabled` and (optionally) re-read roles from DB.

IdP gotchas:
- Entra ID: groups claim returns object IDs, not names; above ~200 groups it emits an overage claim and you must call Microsoft Graph. Prefer **App Roles** (`roles` claim) for Entra.
- Okta: groups claim must be added on the authorization server with a filter.
- Keycloak: use a group or realm-role mapper; roles land in `realm_access.roles`.
- Google Workspace: no groups claim in the ID token; needs the Admin SDK Directory API or a broker.

Model limitation (not SSO-specific): roles are global, so "Team Admin of team A but only a member of team B" cannot be expressed. If SSO groups are per team (e.g. `fluxgate-payments-admins`), consider per-team roles (`user_roles.team_id NULLABLE`) as a follow-up.

### 1.5 Deprovisioning: SCIM 2.0 (phase 2)

Login-time sync does not remove users who never log in again. SCIM (`/scim/v2/Users`, `/scim/v2/Groups`, bearer token per provider) lets Okta/Entra push create, disable and group changes immediately. On `active=false`: set `enabled=false` and revoke all tokens (needs P1).

### 1.6 Other touch points

- `JwtGuard` public paths: add `/api/v1/auth/sso/*` and `GET /api/v1/auth/sso/providers` (list enabled providers for the login page).
- `AdminGuard`: unchanged. First admin is still created locally; it is the break-glass account.
- Setting "enforce SSO": disables password login for non-admin local users.
- `POST /auth/logout`: optionally redirect to the IdP `end_session_endpoint` (RP-initiated logout).
- Config: allow `client_secret` from env (`FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`) so secrets are not in the DB in K8s deployments.
- UI: SSO buttons on `UI/pages/Login.tsx`, public route `/auth/sso/complete`, admin page Settings → SSO (provider CRUD, "test connection", group mappings, preview of claims from last login).
- Also fix: backend temporary-password redirect points to `/reset-password` (protected), UI public page is `/temporary-password-reset`; `UI/utils/auth.ts` lists a non-existent `Admin` role.
- Contracts: new REST DTOs change contract hashes; update baseline on purpose.

Effort: OIDC login + JIT + role/team sync + admin UI ≈ 2-3 weeks for one engineer, plus P1/P2 ≈ 3-5 days. SCIM ≈ 1-2 weeks.

---

## 2. Jira integration

### 2.1 What the existing API already supports

A `system_client` token with scope `flag:write`, scoped to one team, can already call:

| Action | Call | Approval gated |
|--------|------|----------------|
| Request deploy to an environment | `POST /api/v1/stages/{stageId}/request-change` `{"request":"DEPLOYMENT_REQUESTED"}` | Yes, if a policy matches |
| Request rollback | same, `"ROLLBACK_REQUESTED"` | Yes |
| Execute after approval | same, `"DEPLOYED"` / `"ROLLBACKED"` | Requires `*_APPROVED` state |
| Approve / reject / cancel | `POST /api/v1/approval-requests/{id}/approve\|reject\|cancel` `{comment}` | n/a |
| Change targeting / weights | `PUT /api/v1/stages/{stageId}/criteria` | No |
| Schedule a stage change | `POST /api/v1/features/{id}/scheduled-changes` | `STAGE_CHANGE` yes |
| Config version rollback | `POST /api/v1/features/{id}/versions/{vid}/rollback` | No |

Denied to system clients today (by design): `PATCH /features/{id}`, `emergency-disable/enable` (kill switch).

Stage state machine (`BE/src/validation.rs:51-82`):
`NOT_DEPLOYED → DEPLOYMENT_REQUESTED → DEPLOYMENT_APPROVED → DEPLOYED → ROLLBACK_REQUESTED → ROLLBACK_APPROVED → ROLLBACKED`. Deploy and execute are two separate calls.

Missing pieces:
- No feature ↔ ticket link. Only `features.reference_url` and `tags` exist.
- No ticket/reason field on stage change requests or `approval_requests`.
- No outbound webhooks. Notifications are email (SMTP) and SMS (not actually sent). The wiki documents webhooks that do not exist.
- Stage IDs are opaque UUIDs; Jira knows a feature key and an environment name.

### 2.2 Options

| Option | How | Pros | Cons |
|--------|-----|------|------|
| A. Jira Automation only | Rule: "When issue transitions to X" → "Send web request" to FluxGate with system-client token. | No code in Jira. Works on Cloud and Data Center. Fast. | Token stored in the rule. Stage ID lookup needs a chained request. One-way unless FluxGate can call back. FluxGate admin API must be reachable from Atlassian Cloud. |
| B. Bridge service | Small service (Rust or Node) receives Jira webhooks, calls FluxGate; receives FluxGate webhooks, comments/transitions Jira issues. | Idempotency, retries, mapping config, two-way sync, one place for secrets. Can sit inside the private network and expose only a webhook endpoint. | One more deployable. |
| C. Forge app (Jira Cloud) | Issue panel shows feature status per environment, buttons for deploy/rollback, links approval requests. | Best UX. Atlassian handles auth/hosting. | Cloud only. Most effort. Still needs B-like backend calls. |

Recommendation: **A first, then B** for two-way sync. C only if teams want to operate entirely from Jira.

### 2.3 Recommended design

**Linking.** Short term: convention, no schema change: tag `jira:PROJ-123` on the feature (tags are GIN-indexed and filterable via `GET /teams/{id}/features?tag=`). Medium term: table

```sql
CREATE TABLE feature_external_links (
  id UUID PRIMARY KEY,
  feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
  system VARCHAR(20) NOT NULL,       -- 'jira'
  external_key TEXT NOT NULL,        -- 'PROJ-123'
  url TEXT,
  UNIQUE (system, external_key, feature_id)
);
```

**Ticket reference on changes.** Add optional `externalRef` (and `reason`) to `StageChangeRequestBody`, persist on `approval_requests` and in `activity_log.metadata`. Approvers then see "requested from PROJ-123". Contract baseline update required. This also aligns with the planned AI-20 justification check, which already treats a ticket key as a valid reason.

**Workflow mapping (example):**

| Jira transition | FluxGate call |
|-----------------|---------------|
| "Ready for Staging" | `request-change` `DEPLOYMENT_REQUESTED` on staging stage |
| "Release to Prod" | `request-change` `DEPLOYMENT_REQUESTED` on prod stage |
| (FluxGate approval approved) | webhook → Jira comment + transition "Approved" |
| "Go Live" | `request-change` `DEPLOYED` |
| "Rollback" | `ROLLBACK_REQUESTED`, then `ROLLBACKED` after approval |

**Stage lookup.** With only existing APIs, Automation does two requests: `GET /teams/{teamId}/features?tag=jira:{{issue.key}}` (or by key), pick the stage by environment from the response, then `POST /stages/{id}/request-change`. Optional convenience endpoint to remove this: `POST /api/v1/teams/{teamId}/features/by-key/{key}/environments/{envName}/request-change` (thin wrapper over the same logic, same scope rules).

**Approvals: keep humans in FluxGate.** A system client can call `approve`, but then the audit log shows the bot, not the human, and four-eyes control is lost (and P8 means there is no self-approval check). Options, best first:
1. Approvals stay in FluxGate UI; Jira only requests and executes, and receives status via webhook.
2. If approval must happen in Jira: the bridge (option B) passes the Jira approver's identity, FluxGate maps it to a FluxGate user (by email, or via SSO identity from section 1), and records that user as voter. This needs an "act on behalf of" capability on system clients, gated by a new scope (e.g. `approval:delegate`) and policy flag.

**Outbound webhooks (needed for any two-way flow).** Add `webhook` as a notification channel:
- Tables `webhook_endpoints(id, team_id, url, secret_enc, event_types[], enabled)` and `webhook_deliveries(id, endpoint_id, event, payload, status, attempts, next_attempt_at)`; DB CHECK on channels must be extended.
- Events: `approval.requested|approved|rejected|cancelled`, `stage.deployed|rolled_back`, `kill_switch.activated|deactivated`, `canary.failed`. Source: existing notification types and the `ApprovalRequestEvent` broadcast.
- HMAC-SHA256 signature header, retries with backoff in a scheduler (same pattern as `BE/src/scheduler/`). `reqwest` is already a dependency.
- Jira Automation "Incoming webhook" trigger can consume these without a bridge.

**Security before exposing to Jira.** Fix P4, P5, P6, P7. Use a dedicated system client per Jira project with only `flag:write`, short expiry, and rotate. If Jira Cloud must reach the admin API, restrict ingress to Atlassian's published IP ranges, or run the bridge (B) at the edge of the private network.

**Kill switch from Jira.** Not possible today (system clients denied). The scheduled-changes route is a loophole (P6), not a feature. If wanted, add an explicit `flag:emergency` scope.

Effort: P4-P8 ≈ 1 week. Automation recipe + docs ≈ 2-3 days. `externalRef` + links table ≈ 3-5 days. Webhooks ≈ 1-1.5 weeks. Bridge service ≈ 1-2 weeks. Forge app ≈ 3-4 weeks.

---

## 3. Client libraries

### 3.1 Current state

- Edge client API: `POST /evaluate` (credentials come from edge config; caller credentials ignored), OFREP `POST /ofrep/v1/evaluate/flags[/{key}]` (broken, P9), `GET /health`. No streaming, no config download, no client event endpoint, no CORS.
- Bulk OFREP ETag ignores evaluation context (B14); bulk evaluation is not team-scoped (B16).
- Real-time updates and full flag configs exist only on the backend gRPC `StreamUpdates` (`FeatureFull` with stages, criteria, variants, dependencies).
- `evaluation-engine`: pure Rust, deps only serde/serde_json/sha2/chrono/regex/semver. No I/O. Single entry point `evaluate(&FeatureEvaluationContext, &Feature)`. Likely `wasm32` compatible (not yet built). Some logic lives outside it: protobuf → engine mapping and kill-switch/active inversion (`EDGE/handlers.rs:192-334`), simple-type boolean coercion.
- Java: Spring Boot 3.2 / Java 17 starter exists but is broken against current edge (P11). Other gaps: property prefix mismatch (`fluxgate` vs `feature.toggle.*` in conditions), `@Retryable` cannot fire (exceptions caught; self-invocation), caching/metrics properties unused, boolean-only API, OpenFeature provider coerces all types to boolean and always reports `TARGETING_MATCH`.
- Rust SDK: none. JS SDK: none. `docs/contract-driven-sdk.md` proposes generated SDKs; `docs/tasks.md` has the task unchecked.

### 3.2 Strategy: OpenFeature first, OFREP as the wire protocol

OpenFeature has official OFREP providers for Java, JavaScript (web and server), and other languages, plus framework SDKs for React and Angular. Once the edge speaks correct OFREP, every language gets a working provider for free. FluxGate SDKs then add what generic providers do not: FluxGate auth/config, typed helpers, framework integration, caching policy, and (for Rust) in-process evaluation.

Confirm current package names/versions when starting: `dev.openfeature.contrib.providers:ofrep` (Java), `@openfeature/ofrep-web-provider` and `@openfeature/ofrep-provider` (JS), `@openfeature/react-sdk`, `@openfeature/angular-sdk`, `open-feature` crate and its OFREP contrib provider (Rust).

### 3.3 Edge server work (shared by all SDKs)

1. **Fix OFREP auth (P9).** Define an SDK key format, e.g. `Authorization: Bearer fg_<clientId>.<secret>` or `X-Client-Id` + `X-Client-Secret`. Return 401/403, not 502 (B06). Ensure `/evaluate` and OFREP use the same auth path.
2. **Per-client multi-tenancy.** Today the edge serves one configured client. For a shared edge, evaluate with the caller's client (environment, team scoping), and scope the bulk cache per team (B16).
3. **CORS (P10)** for `WEB` clients: allow origins from the client's `web_origins`; handle preflight; expose `ETag`.
4. **Bulk ETag must include a context hash** (B14), or browser SDK caching is wrong.
5. **Change notifications**: SSE endpoint `GET /ofrep/v1/events` (or OFREP `eventStreams`) emitting "configuration changed" so SDKs re-fetch instead of polling. The edge already receives `FeatureUpdate` from gRPC.
6. **Client events** (optional): `POST /v1/events` for client-side impressions/custom metrics, forwarded via existing `PushEvaluationEvents` / `TrackMetrics`.
7. **Config download for local evaluation** (optional, server SDKs only): `GET /v1/flags/config` returning engine-format features. Never expose this to `WEB` clients (it reveals targeting rules).

### 3.4 Java / Spring Boot

Rewrite rather than patch the transport layer; keep the module and public `FluxGateClient` name where sensible.

- Transport: OFREP single + bulk via Spring `RestClient` (Boot 3.2+) with connection pooling and timeouts. Remove body credentials; send SDK key header.
- API: typed evaluation — `getBoolean/getString/getInteger/getDouble/getObject(key, default, context)` returning value + variant + reason (`FlagEvaluation<T>`). Keep `isEnabled` as a shortcut.
- Context: map `{targetingKey, attributes}`; drop the `environment` attribute (edge uses the client's environment).
- Caching: Caffeine, keyed by `(flagKey, contextHash)`, TTL; optional "bulk prefetch + SSE invalidate" mode.
- Resilience: real retries (Spring Retry outside the bean, or Resilience4j), circuit breaker, default value on failure; never throw from `getBoolean`.
- OpenFeature: make the provider the core (`FeatureProvider` with correct value types, reasons, error codes, `initialize` readiness, `PROVIDER_CONFIGURATION_CHANGED` from SSE). Auto-register with `OpenFeatureAPI` when `fluxgate.openfeature.auto-register=true`.
- Spring integration: single prefix `fluxgate.*`; `@ConditionalOnProperty(prefix="fluxgate")`; `@FeatureToggle("key")` annotation + AOP aspect with fallback method; `ConditionalOnFeature` for beans (evaluated once at startup — document this); Actuator health using cached stream state; Micrometer `fluxgate.evaluations` timer/counter with `flag`, `variant`, `reason` tags.
- Tests: WireMock stubs generated from the edge OpenAPI spec (so shape drift fails tests), plus a Testcontainers test against the real edge + backend images.
- Versions: Spring Boot 3.2 is out of OSS support. Target the current 3.5.x line and test against 4.x; keep Java 17 baseline. Fix README claims (metrics, caching) and root README coordinates.

Effort ≈ 2-3 weeks.

### 3.5 Rust

Rust has a unique advantage: the evaluation engine is already Rust. Offer two modes in one crate `fluxgate`:

- **Remote** (default): OFREP over `reqwest`, async (`tokio`), optional `blocking` feature. Typed API like Java.
- **Local / in-process** (feature `local`): subscribe to backend gRPC `StreamUpdates` (or edge config endpoint, 3.3 item 7), keep features in an `ArcSwap<HashMap>`, evaluate with `evaluation-engine`. Sub-microsecond evaluations, no network per call. Effectively an embedded edge.

Prerequisite refactor: move protobuf → engine mapping, kill-switch inversion and simple-type coercion out of `EDGE/handlers.rs` into the engine (or a new `fluxgate-core` crate) so edge, backend and SDK share them. Publish `evaluation-engine` to crates.io (rename to `fluxgate-engine`). Event batching for local mode can reuse edge code (`PushEvaluationEvents`).

Also provide an OpenFeature `FeatureProvider` impl behind feature `openfeature`. Library conventions: `thiserror` error types, builder-style config, no panics in library code.

Effort ≈ 2 weeks (remote 1 week, local mode +1 week after the refactor).

### 3.6 JavaScript

Packages (monorepo, e.g. `sdk/js` with pnpm workspaces):

- `@fluxgate/js` — vanilla, zero-dependency, ESM + CJS, `fetch` based. Browser pattern: on `init(context)` call OFREP bulk once, keep results in memory, serve `getBoolean/getString/...` synchronously, re-fetch on `setContext`, on SSE "changed", or on poll interval with `If-None-Match`. Fires `ready`/`change`/`error` events. Also exports an OpenFeature web provider.
- `@fluxgate/react` — thin layer: `<FluxGateProvider>` + `useFlag(key, default)` / `useFlagDetails`. Option: build on `@openfeature/react-sdk` instead of custom context, so apps can switch vendors.
- `@fluxgate/angular` — `FluxGateService` (signals + RxJS), `*fluxgateFlag="'key'"` structural directive, route guard `canActivate: [flagGuard('key')]`. Same choice: wrap `@openfeature/angular-sdk` or standalone.
- Node server: `@fluxgate/node` per-request remote evaluation via OFREP single. Optional later: local mode with the engine compiled to WASM (`wasm-bindgen`) — server only, never in the browser.

Security: the browser holds a public `WEB` client key. That is acceptable because evaluation is remote (no rules exposed) and the edge enforces `web_origins` (needs P10). Document that `WEB` keys must have no write capability.

Effort ≈ 2-3 weeks for js + react + angular.

### 3.7 Cross-SDK conformance

- Spec doc: SDK key format, context mapping, default-value semantics, caching rules, event names, error → default behaviour.
- Shared JSON fixtures (feature config + context + expected `{value, variant, reason}`) generated from the engine tests in `ENGINE/tests/evaluation_tests.rs`. Each SDK runs them against a dockerized backend + edge stack (extend `feature-toggle/docker-compose.api-tests.yml` with the edge). Rust local mode also runs them in-process to guarantee bucketing parity (sha256 of `flagKey:targetingKey`).
- Generate request/response types from `contracts/generated/openapi.json` (edge spec) per `docs/contract-driven-sdk.md`; hand-write behaviour.
- CI: publish to Maven Central, crates.io, npm from tags.

---

## 4. Suggested order

1. Prerequisite fixes P1-P11 (≈ 2 weeks). They are bugs today regardless of the new features.
2. Edge SDK foundations: OFREP auth, CORS, ETag, SSE (≈ 1-1.5 weeks).
3. Java starter rewrite and JS core + React (unblocks most app teams).
4. OIDC SSO + role sync.
5. Jira: Automation recipe + `externalRef`, then outbound webhooks, then bridge.
6. Rust SDK (remote, then local mode), Angular package.
7. SCIM, Forge app, per-team roles — on demand.

## 5. Open questions

1. Which IdPs must be supported first (Entra ID, Okta, Keycloak, Google)? Any SAML-only requirement?
2. Jira Cloud or Data Center? (Forge is Cloud only; network reachability differs.)
3. Should approvals stay in FluxGate, or must Jira be the approval system of record?
4. Is the edge expected to be shared across teams/clients, or one edge (sidecar) per client? This decides the scope of 3.3 item 2.
5. Do server SDKs need in-process (local) evaluation, or is the edge sidecar enough?
6. Minimum Spring Boot / Java versions customers run?
