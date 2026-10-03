# Jira integration, phase 1: design

Date: 2026-10-03. Decisions are in [`README.md`](README.md#decisions-2026-10-03). Code references are against backend `6c9b332` and UI `dc16a2f`.

## 1. Goal

A team works in Jira. When an issue moves through its workflow, Jira asks FluxGate to deploy or roll back the linked feature in an environment. Humans approve in FluxGate, as today. Approvers see which Jira issue asked for the change and why. Jira can read back the stage status.

Out of scope: pushing events to Jira (webhooks), approving from Jira, kill switch from Jira, a Forge app, a bridge service.

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

### 3.5 UI (JI-20, JI-21)

- Feature page: a "Jira" card listing linked keys (link when `url` is set), add (key + optional URL) and remove, for users who can edit the feature.
- Stage request dialog in `FeatureCreate.tsx`: optional "External reference" and "Reason" inputs, sent with `requestStageChange`.
- Approvals page: show `externalRef` and `requestReason` on the list item and detail.
- Activity feed: show `external_ref` and `reason` from metadata.

### 3.6 Jira Automation recipe (JI-30)

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
- `api-tests/`: JI-30 adds one end-to-end flow as Jira would run it: system client, link, request by key with `externalRef`, approval by a human user, execute, read back by key, and a 403 when the system client tries to approve.
