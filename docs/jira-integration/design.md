# Jira integration, phase 1: design

Date: 2026-10-03. Decisions are in [`README.md`](README.md#decisions-2026-10-03). Code references are against backend `6c9b332` and UI `dc16a2f`.

## 1. Goal

A team works in Jira. When an issue moves to a status, FluxGate acts on the features linked to that issue, in the environments named on the issue. Example: the issue moves to "Ready for Release" with Environment = QA, so the feature is approved for QA. The issue moves to "Done", so the feature is deployed to QA. Which status does what is configured per team in FluxGate (status rules, §3.7), not hard-coded.

In environments an admin opts in, the Jira status change **is** the approval (decision J12). Elsewhere a rule can only create a pending request, and humans approve in FluxGate as today. Every change records the Jira issue, status and Jira user.

Out of scope: pushing results back to Jira (comments, transitions, webhooks), kill switch from Jira, a Forge app, a bridge service, mapping Jira users to FluxGate users.

## 2. What exists today

- A system client (`/teams/{teamId}/system-clients`, team admin or system admin only) gets a JWT with scopes `evaluate`, `metrics:write`, `admin:read`, `flag:write` (`logic/system_client.rs`). Its JWT carries roles `Requester` and `Approver` and `is_admin: false` (`middleware/jwt_guard.rs`, `create_system_client_jwt_token`). A shadow `users` row with the same id is the actor in the activity log.
- `system_client_scope_allowed` (`middleware/jwt_guard.rs`) maps scopes to routes. `flag:write` allows POST/PUT/PATCH/DELETE on any path with a `features`, `stages`, `approval-requests`, `criteria` or `scheduled-changes` segment. Reads on `features` and `environments` paths need `admin:read` or `evaluate`.
- `RequestScopeResolver` (`logic/authorization.rs`) resolves the team of `/teams/{id}`, `/features/{id}`, `/stages/{id}` and `/approval-requests/{id}` paths, and `JwtGuard` rejects a system client outside its team.
- Stage change: `POST /api/v1/stages/{id}/request-change` with `StageChangeRequestBody { request, freezeOverrideReason }` (`rest/feature/types.rs`). The handler `request_stage_change` (`rest/feature.rs`) checks roles (`RoleAuthorizer::authorize_stage_change_request`), freeze windows, then calls `DeploymentLogic::request_stage_change(stage_id, request, user_id)` (`logic/feature.rs`). That method creates the approval request through `ApprovalLogic::maybe_create_stage_change_request` (`logic/approval.rs`) when a policy matches, updates the stage, sends notifications and writes an activity row (`stage_change_requested`, `STAGE_DEPLOYED`, ...) with metadata `feature_id`, `feature_key`, `stage_id`, `status`, `team_id`, `environment_name`, `environment_id`.
- State machine (`validation.rs`): `NOT_DEPLOYED → DEPLOYMENT_REQUESTED → DEPLOYMENT_APPROVED → DEPLOYED → ROLLBACK_REQUESTED → ROLLBACK_APPROVED → ROLLBACKED`. Request and execute are two calls.
- Votes: `POST /approval-requests/{id}/approve|reject`. `ensure_user_can_vote` (`logic/approval.rs`) blocks self-approval and checks the `Approver` role **from the database** (`user_roles`), not from the JWT. At planning time there was no rule against system clients, and their shadow users hold the `Approver` role (fixed by JI-01).
- Features have `reference_url` and `tags`. No link to an issue tracker. `get_feature_by_key(team_id, key)` exists in the repository.

## 3. Changes

### 3.1 System clients never vote (JI-01)

Deny `approve` and `reject` for system-client actors with 403 `system_client_vote_not_permitted`, at the REST layer and again in `ensure_user_can_vote` (defense in depth, also covers any future caller). `cancel` keeps today's rules, so Jira can withdraw a request it created. System clients are also never eligible approvers (`approver_qualifies_sql`), so routing, notifications and the AI-11 cap ignore them (decision J9). Done in `e1bebfe`; the REST-layer check lives in `JwtGuard`.

### 3.2 External links (JI-10)

```sql
CREATE TABLE feature_external_links (
  id UUID PRIMARY KEY,
  feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
  system VARCHAR(20) NOT NULL CHECK (system IN ('jira')),
  external_key TEXT NOT NULL,          -- 'PROJ-123', stored upper case
  url TEXT,                            -- optional, http(s) only
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (feature_id, system, external_key)
);
CREATE INDEX idx_feature_external_links_key ON feature_external_links (system, external_key);
```

- One feature can link many issues; one issue can link many features.
- Jira key format: `^[A-Z][A-Z0-9_]+-[1-9][0-9]*$` after trim and upper-casing. URL: `http` or `https`, at most 2048 characters.
- API:
  - `GET /api/v1/features/{id}/external-links` → `{items: [ExternalLinkResponse]}`
  - `POST /api/v1/features/{id}/external-links` `{system: "jira", externalKey, url?}` → 201 with the link; 409 when it exists.
  - `DELETE /api/v1/features/{id}/external-links/{linkId}` → 204.
  - `GET /api/v1/teams/{teamId}/features?externalKey=PROJ-123` filters the existing list (system `jira`).
- Write permission: same as updating the feature's team resources for users; `flag:write` for system clients (the path has a `features` segment). Each add and remove writes an activity row (`external_link_added`, `external_link_removed`) on the feature.
- No `FeatureUpdate` broadcast: links do not change evaluation.

### 3.3 `externalRef` and `reason` on stage changes (JI-11)

- `StageChangeRequestBody` gains `externalRef: Option<String>` (trimmed, 1–100 characters, no control characters) and `reason: Option<String>` (trimmed, 1–1000 characters). Blank strings become `None`. Any free-form reference is allowed (Jira key, ServiceNow change, URL); the UI only links it when it is a Jira key that is linked to the feature.
- A new `StageChangeMeta { external_ref, reason }` (in `model.rs`) is passed through `DeploymentLogic::request_stage_change` and `ApprovalLogic::maybe_create_stage_change_request`.
- Migration: `ALTER TABLE approval_requests ADD COLUMN external_ref TEXT NULL, ADD COLUMN request_reason TEXT NULL;`. `ApprovalRequest` entity, `CreateApprovalRequestInput` and `ApprovalRequestResponse` (`externalRef`, `requestReason`) gain the fields.
- Activity metadata for every stage change activity gains `external_ref` and `reason` when present. When an approved request is executed through the approval path, its activity carries the request's `external_ref`.
- Notification metadata for `STAGE_CHANGE_REQUESTED` gains `external_ref`, and the message body appends "Ref: <externalRef>".
- Scheduled stage changes pass `StageChangeMeta { external_ref: None, reason: <scheduled change reason> }`.

### 3.4 By-key endpoints (JI-12)

- `GET /api/v1/teams/{teamId}/features/by-key/{key}` → the same `FeatureResponse` as `GET /features/{id}` (with stages and their status), so Jira can read the state of each environment.
- `POST /api/v1/teams/{teamId}/features/by-key/{key}/environments/{envName}/request-change` with the same body as the stage route. It resolves feature (exact key in the team), environment (active, name equal ignoring case, in the team) and the feature's stage for that environment, then runs **the same code** as `POST /stages/{id}/request-change` (extract a shared function; do not copy it).
- 404 bodies name what was missing: `feature_not_found`, `environment_not_found`, `stage_not_found`.
- Both paths contain `features`, so system clients need `flag:write` (POST) or `admin:read`/`evaluate` (GET). `/teams/{teamId}` scopes the team in `JwtGuard`.

### 3.5 UI (JI-20, JI-21, JI-22)

- Feature page: a "Jira" card listing linked keys (link when `url` is set), add (key + optional URL) and remove, for users who can edit the feature.
- Stage request dialog in `FeatureCreate.tsx`: optional "External reference" and "Reason" inputs, sent with `requestStageChange`.
- Approvals page: show `externalRef` and `requestReason` on the list item and detail.
- Activity feed: show `external_ref` and `reason` from metadata.
- Approvals: "Approved by Jira (<display name>, <issue key>)" when `approvalSource = 'jira'` (JI-21).
- Settings → Jira (JI-22): integrations, environment field and aliases, Jira-approved environments, rules editor, secret rotation, event log.

### 3.7 Jira integration and status rules (JI-13)

```sql
CREATE TABLE jira_integrations (
  id UUID PRIMARY KEY,
  team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  name VARCHAR(100) NOT NULL,
  jira_base_url TEXT,                        -- for links in the UI; optional
  secret_hash TEXT NOT NULL,                 -- SHA-256 of the inbound secret; the secret is shown once
  environment_field TEXT NOT NULL,           -- 'labels' or a field id like 'customfield_10042'
  environment_aliases JSONB NOT NULL DEFAULT '{}', -- {"Production": "prod", "QA env": "qa"}; keys compared ignoring case
  jira_approved_environment_ids UUID[] NOT NULL DEFAULT '{}', -- environments where Jira is a trusted approver
  feature_key_field TEXT,                    -- optional field holding FluxGate feature keys, in addition to links
  actor_user_id UUID NOT NULL REFERENCES users(id), -- shadow user that requests and executes (see below)
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (team_id, name)
);

CREATE TABLE jira_status_rules (
  id UUID PRIMARY KEY,
  integration_id UUID NOT NULL REFERENCES jira_integrations(id) ON DELETE CASCADE,
  jira_status TEXT NOT NULL,                 -- compared ignoring case and surrounding spaces
  action VARCHAR(20) NOT NULL CHECK (action IN ('request', 'approve', 'deploy', 'rollback')),
  environment_ids UUID[],                    -- optional filter; NULL = every environment the issue names
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  position INT NOT NULL DEFAULT 0,
  UNIQUE (integration_id, jira_status, action)
);
```

- One team can have several integrations (for example one per Jira project). Admin of the team (or system admin) manages them, same policy as system clients.
- **Actor.** Requests and executions need a user id (`requested_by`, activity actor). Each integration gets a shadow user, created like a system client's (`auth_source = 'system'`, no password, no admin, roles `Requester` only). It is not a system client and has no token: it can only act through the inbound endpoint. It is never an eligible approver (it has no `Approver` role).
- **Secret.** Generated by FluxGate (32 random bytes, base64url), returned once on create and on rotate, stored as SHA-256.
- Several rules may match one status (for example `approve` and `deploy` on "Done"). They run in `position` order.
- **Environment resolution.** Read `environment_field` from the issue: a string, an option object (`{"value": "QA"}`), an array of either, or `labels` (array of strings). For each value: look it up in `environment_aliases` (ignoring case), else match an active environment of the team by name (ignoring case). Unknown values are reported in the event result, not errors.
- **Feature resolution.** Features linked to the issue key (`feature_external_links`, JI-10) in the integration's team, plus feature keys from `feature_key_field` if set.

### 3.8 Approval by an external system (JI-14)

New logic entry point used only by the rule engine, in `logic/` (for example `logic/external_change.rs`):

```rust
pub struct ExternalActor { pub system: String /* "jira" */, pub account_id: Option<String>, pub display_name: Option<String> }
pub struct ExternalChangeContext { pub actor_user_id: Uuid, pub external_ref: String /* issue key */, pub reason: String /* "Jira status 'Done'" */, pub external_actor: ExternalActor, pub trusted_approval: bool }
pub enum ExternalAction { Request, Approve, Deploy, Rollback }
pub enum ExternalOutcome { Applied { from: String, to: String, approval_request_id: Option<Uuid> }, NoOp { status: String }, Refused { reason: String } }
async fn apply_external_action(feature_id, environment_id, action, ctx) -> Result<ExternalOutcome, Error>
```

Per action, from the current stage status:

| Action | Stage now | Result |
|---|---|---|
| `request` | `NOT_DEPLOYED`, `DEPLOYMENT_REJECTED`, `ROLLBACKED` | `DEPLOYMENT_REQUESTED` through the normal path (approval request when a policy applies), with `externalRef`/`reason` (JI-11) |
| `request` | already requested, approved or deployed | `NoOp` |
| `approve` | `trusted_approval = false` | `Refused("environment not approved by Jira")` |
| `approve` | `NOT_DEPLOYED`, `DEPLOYMENT_REJECTED`, `ROLLBACKED` | request as above, then approve it as Jira → `DEPLOYMENT_APPROVED` |
| `approve` | `DEPLOYMENT_REQUESTED` | approve the pending request as Jira → `DEPLOYMENT_APPROVED` |
| `approve` | `DEPLOYMENT_APPROVED`, `DEPLOYED` | `NoOp` |
| `deploy` | `DEPLOYMENT_APPROVED` | `DEPLOYED` through the normal execute path |
| `deploy` | `DEPLOYED` | `NoOp` |
| `deploy` | anything else | `Refused("not approved")` (decision J14) |
| `rollback` | `DEPLOYED` | `ROLLBACK_REQUESTED`; if `trusted_approval`, approve as Jira and execute → `ROLLBACKED` |
| `rollback` | `ROLLBACK_REQUESTED` / `ROLLBACK_APPROVED` | if `trusted_approval`, continue to `ROLLBACKED`; else `NoOp` |
| `rollback` | not deployed / already rolled back | `NoOp` |

- "Approve as Jira" closes the approval request with `status = 'approved'`, no vote row, `approval_source = 'jira'`, `external_approver` JSON (account id, display name, issue key, status), `executed_at`; it applies the stage change in the same transaction as the vote path does (`execute_change_tx`), publishes the approval event and the edge broadcast, and writes an `approval_request_approved_externally` activity row. When no policy applies to the environment, it moves the stage `DEPLOYMENT_REQUESTED → DEPLOYMENT_APPROVED` directly (check how today's code reaches `DEPLOYMENT_APPROVED` without a policy, and reuse it).
- Migration: `approval_requests.approval_source VARCHAR(20) NOT NULL DEFAULT 'fluxgate'` (`fluxgate`, `jira`, `auto`), `approval_requests.external_approver JSONB NULL`. `ApprovalRequestResponse` gains `approvalSource` and `externalApprover`.
- Freeze windows and rollout dependency checks apply as for any stage change. A blocked change is `Refused` with the reason; no freeze override from Jira.
- The AI-11 extra-approver mode and auto-approval do not apply to a request approved by Jira: it is closed before they run. Record this in the activity metadata (`ai_risk_mode_skipped`), so it is visible.

### 3.9 Inbound events and the rule engine (JI-15)

- `POST /api/v1/integrations/jira/{integrationId}/events`, public in `JwtGuard` (no FluxGate JWT), authenticated by the integration secret: `Authorization: Bearer <secret>` (Automation "Send web request" on Cloud and Data Center) or `X-FluxGate-Jira-Secret: <secret>`. Constant-time compare of the SHA-256. 401 on a wrong secret, 404 on an unknown or disabled integration (same body for both is acceptable; never reveal which).
- Body: the Jira issue in Jira REST format. Accept both shapes:
  - Jira webhook (`webhookEvent: "jira:issue_updated"`, `issue`, `user`, `changelog`): act only when `changelog.items` has a `status` item; the target status is its `toString`.
  - Automation "Issue data (Jira format)" (`key`, `fields`, ...), optionally wrapped as `{"issue": {...}, "user": {...}}`: the target status is `fields.status.name`.
  - Limit the body to 1 MiB.
- Processing (inside the request, it is fast): resolve features and environments (§3.7), match enabled rules on the status, and for each (rule, feature, environment) call `apply_external_action` with `trusted_approval = environment ∈ jira_approved_environment_ids`. Errors of one target do not stop the others.
- Event log `jira_integration_events (id, integration_id, received_at, issue_key, jira_status, jira_actor JSONB, delivery_hash TEXT, results JSONB, error TEXT)`. `results` is a list of `{featureKey, environment, rule, action, outcome, from, to, reason}`. Keep 30 days (cleanup in an existing scheduler). `GET /api/v1/jira-integrations/{id}/events?offset&limit` for the UI.
- Idempotency: `delivery_hash` = SHA-256 of (issue key, status, sorted environment values, `changelog.id` or `issue.fields.updated`). A repeated delivery within 10 minutes returns the stored result and does nothing. The state table in §3.8 is also idempotent (`NoOp`).
- Response: 200 with `{eventId, results}` (also for "no rule matched"), so a Jira rule audit log shows what happened.

### 3.10 Jira setup guide (JI-30)

*Superseded in part by §3.7-3.9: with status rules, the Jira side is one static rule or webhook that sends every status change to the inbound endpoint. JI-30 writes the guide for that setup. The by-key calls below stay as an option (JI-12) for teams that want per-transition Automation rules.*

Docs page `docs/jira-integration/automation-recipe.md`:

1. Create a team system client for the Jira project. Keep its expiry short and rotate it. Store the token as a secret in the Automation rule header (`Authorization: Bearer <token>`).
2. Link the issue: rule on issue create (or manual trigger) → `POST /features/{id}/external-links`, or link in the FluxGate UI.
3. Transition rules, for example:

| Jira transition | Request |
|---|---|
| "Ready for Staging" | `POST /teams/{teamId}/features/by-key/{featureKey}/environments/staging/request-change` `{"request":"DEPLOYMENT_REQUESTED","externalRef":"{{issue.key}}","reason":"{{issue.summary}}"}` |
| "Release to Prod" | same with `production` |
| "Go Live" | `{"request":"DEPLOYED", ...}` (works only after approval in FluxGate) |
| "Rollback" | `ROLLBACK_REQUESTED`, then `ROLLBACKED` after approval |

The feature key comes from a Jira custom field (for example "FluxGate feature") or the issue's linked feature.
4. Read back status: scheduled rule → `GET /teams/{teamId}/features/by-key/{key}` and compare stage status.
5. Network: Jira Cloud calls come from Atlassian's published IP ranges; restrict ingress to them. Jira Data Center usually runs in the same network; restrict to its hosts.

## 4. Testing

- Backend unit tests per task (handlers with mocks, validation, `ensure_user_can_vote`).
- DB integration tests for the new repository (`tests/database/`), against the seeded `feture_toggle_test` DB.
- `api-tests/`: JI-30 adds the end-to-end flows as Jira would run them: integration with rules, link, inbound "Ready for Release" event approves in a Jira-approved environment, "Done" deploys, `deploy` before approval is refused, `approve` in a non-opted-in environment is refused and a `request` rule leaves a pending request for a human, wrong secret 401, repeated delivery is a no-op.
