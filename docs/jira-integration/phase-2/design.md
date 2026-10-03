# Jira integration, phase 2: design

Date: 2026-10-03. Phase 1 is in [`../design.md`](../design.md) and [`../README.md`](../README.md). Code references are against backend `135d918` and UI `131060d`.

## 1. Goal

In phase 1, Jira drives FluxGate, but Jira users cannot see what happened unless they open FluxGate. Phase 2 closes that loop: **FluxGate reports results on the Jira issue**. It also hardens the public inbound route and adds the reason hint that phase 1 left as a follow-up.

What a Jira user sees on an issue linked to a feature:

1. A **comment** with the outcome of each Jira event FluxGate received for that issue (applied, refused and why, unknown environment).
2. A **comment** when a person approves or rejects an approval request in FluxGate.
3. A **comment** when a stage changes in FluxGate for another reason: deploy or rollback from the UI, the scheduler, canary governance, or the kill switch.
4. A **remote link** ("FluxGate: flag-x · qa DEPLOYED · prod DEPLOYMENT_REQUESTED") that FluxGate keeps current.

Out of scope: moving the Jira issue to another status (transitions), generic outbound webhooks, a bridge service, a Forge app, mapping Jira users to FluxGate users, kill switch from Jira.

## 2. Decisions (2026-10-03)

| # | Decision |
|---|---|
| J16 | Write-back is **report only**: comments and one remote link per feature and issue. FluxGate never transitions a Jira issue. This avoids workflow coupling and loops, because a transition could fire a FluxGate status rule again. |
| J17 | FluxGate calls the **Jira REST API directly** with a credential stored per integration. Cloud: account email + API token (Basic). Data Center: personal access token (Bearer). There is no generic webhook layer, but the outbox (§3.2) is designed so that one could be added as another consumer. |
| J18 | The credential is encrypted with `secret_box` (`FLUXGATE_ENCRYPTION_KEY`), with the integration id as AAD. It is write-only in the API and never appears in a response, a log line, `last_error`, a test fixture or a commit. |
| J19 | Write-back is **opt-in per integration** and off by default. |
| J20 | Jobs are **only** created for issues linked to the feature (`feature_external_links`, system `jira`), plus the issue that sent an inbound event. |
| J21 | Results are captured from two sources: the inbound event transaction, and an **`activity_log` cursor** for every other change. Every write path already writes an activity row in its own transaction, so the capture does not touch each path. |
| J22 | On 401 or 403 from Jira, write-back for that integration **pauses** until an admin saves a new credential or resumes. FluxGate does not retry a bad token. |
| J23 | The public inbound route gets an **in-process rate limit** (single-server backend, see project memory). Events over the limit are not stored. |
| J24 | Native Jira webhooks are accepted when signed with **HMAC-SHA256** (`X-Hub-Signature: sha256=<hex>`). A secret in the URL is not supported, because URLs end up in proxy and access logs. |

## 3. Changes

### 3.1 Write-back configuration (JI-41)

New columns on `jira_integrations` (new migration; never edit an applied one):

| Column | Type | Notes |
|---|---|---|
| `writeback_enabled` | `BOOLEAN NOT NULL DEFAULT FALSE` | Master switch. |
| `writeback_comments` | `BOOLEAN NOT NULL DEFAULT TRUE` | Post comments. |
| `writeback_remote_link` | `BOOLEAN NOT NULL DEFAULT TRUE` | Keep the remote link current. |
| `jira_auth_kind` | `VARCHAR(20) NULL CHECK (IN ('cloud_basic','dc_pat'))` | Also selects the API version and comment format: Cloud v3 with ADF, DC v2 with plain text. |
| `jira_account_email` | `TEXT NULL` | Required for `cloud_basic`. |
| `jira_credential_enc` | `TEXT NULL` | `secret_box::encrypt_with_aad(token, integration_id)`. |
| `writeback_paused_reason` | `TEXT NULL` | Set by the sender on 401/403 (§3.2). |
| `native_webhook_secret_enc` | `TEXT NULL` | Used by §3.5. Encrypted, not hashed, because HMAC needs the plaintext. |

`jira_base_url` already exists. It is required when `writeback_enabled` is true.

Validation when write-back is enabled:
- `jira_base_url` must be an absolute `https` URL. `http` is allowed only when the backend config has `[jira] allow_insecure_http = true`.
- `jira_auth_kind` and a credential must be set; `cloud_basic` also needs `jira_account_email`.
- If `FLUXGATE_ENCRYPTION_KEY` is missing, saving a credential returns the existing `encryption_key_missing` error.

Endpoints (team admin, same policy as the phase 1 integration endpoints; register DTOs in `ApiDoc` and update the contract baseline):

| Method and path | Purpose |
|---|---|
| `PUT /api/v1/jira-integrations/{id}/writeback` | Set `enabled`, `comments`, `remoteLink`, `authKind`, `accountEmail`, `credential` (optional; omitted keeps the stored one). Saving a credential clears `writeback_paused_reason`. |
| `POST /api/v1/jira-integrations/{id}/writeback/test` | Calls `GET /rest/api/{v}/myself` with the stored credential. Returns `{ok, status, message}`. The message never contains the credential. |
| `POST /api/v1/jira-integrations/{id}/writeback/resume` | Clears `writeback_paused_reason`. |

The integration response gains `writeback: {enabled, comments, remoteLink, authKind, accountEmail, hasCredential, pausedReason}` and `hasNativeWebhookSecret`. It never returns the credential or a secret. Changes write activity rows (`jira_integration_updated`) without the credential.

Turning write-back off sets the integration's `pending` jobs to `dead` with `last_error = 'write-back disabled'`.

### 3.2 Outbound jobs and the sender (JI-42)

New table:

```sql
CREATE TABLE jira_outbound_jobs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  integration_id UUID NOT NULL REFERENCES jira_integrations(id) ON DELETE CASCADE,
  issue_key TEXT NOT NULL,
  feature_id UUID NULL REFERENCES features(id) ON DELETE SET NULL,
  kind VARCHAR(20) NOT NULL CHECK (kind IN ('comment', 'remote_link', 'remote_link_delete')),
  payload JSONB NOT NULL DEFAULT '{}',
  dedupe_key TEXT NOT NULL UNIQUE,
  status VARCHAR(10) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'dead')),
  attempts INT NOT NULL DEFAULT 0,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error TEXT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  sent_at TIMESTAMPTZ NULL
);
-- due-job scan
CREATE INDEX ON jira_outbound_jobs (next_attempt_at) WHERE status = 'pending';
-- job list in the UI
CREATE INDEX ON jira_outbound_jobs (integration_id, created_at DESC);
-- at most one pending remote link refresh per integration, issue and feature
CREATE UNIQUE INDEX ON jira_outbound_jobs (integration_id, issue_key, feature_id)
  WHERE status = 'pending' AND kind = 'remote_link';
```

`dedupe_key` formats: `event:<eventId>`, `activity:<activityId>:<integrationId>:<issueKey>`, `link:<integrationId>:<issueKey>:<featureId>:<source>` where `<source>` is `event:<eventId>` or `activity:<activityId>`. Inserts use `ON CONFLICT DO NOTHING` with no conflict target, so both the `dedupe_key` constraint and the partial unique index below make a duplicate a no-op. A remote link insert that hits the partial unique index is also a no-op: the pending job already covers it.

`comment` payload: `{lines: [string], featureKey, environment?}`. The sender builds ADF (Cloud) or plain text (DC) from it. `remote_link` has no payload: the title is built at send time from current stage status, so it is never stale.

**Sender** (`scheduler/jira_writeback_sender.rs`, every 5 s):

1. Claim up to 50 due `pending` jobs (`next_attempt_at <= now()`) with `FOR UPDATE SKIP LOCKED`, skipping integrations that are disabled, have write-back off, or have `writeback_paused_reason` set. Within one issue, process jobs in `created_at` order, so comments arrive in order.
2. Decrypt the credential once per integration per tick. Do not cache it across ticks.
3. Send with `reqwest`: timeout 10 s, **redirects disabled** (the credential must not follow a redirect to another host).
   - Comment: `POST {base}/rest/api/3/issue/{key}/comment` (Cloud, ADF body) or `POST {base}/rest/api/2/issue/{key}/comment` (DC, `{"body": text}`).
   - Remote link: `POST {base}/rest/api/{v}/issue/{key}/remotelink` with `globalId = fluxgate:feature:<featureId>`, `object.url` = the FluxGate feature page (from `public_base_url`), `object.title` = `FluxGate: <featureKey> · <env> <STATUS> · ...` in pipeline order. Same `globalId` means Jira updates the existing link.
   - Remote link delete: `DELETE {base}/rest/api/{v}/issue/{key}/remotelink?globalId=...`.
4. Classify the result:

| Result | Action |
|---|---|
| 2xx (and 404 on a remote link delete) | `status = 'sent'`, `sent_at = now()`. |
| 429, 5xx, timeout, connect error | `attempts += 1`. Next attempt after 30 s, 2 min, 10 min, 1 h, 6 h. On 429, use `Retry-After` when it is larger. After attempt 6, `dead`. |
| 401, 403 | Job `dead`. Set `writeback_paused_reason = 'Jira returned <status> at <time>'`. Other pending jobs stay pending. |
| 404 (issue gone), other 4xx | `dead`. |

`last_error` stores the status code and the first 300 characters of the response body only, never request headers or the URL query. Tracing spans carry the job id, integration id and issue key, never the credential.

Retention: the existing token-cleanup scheduler pattern deletes `sent` jobs older than 30 days and `dead` jobs older than 90 days.

Endpoints (team admin):

| Method and path | Purpose |
|---|---|
| `GET /api/v1/jira-integrations/{id}/outbound-jobs?status=&page=&pageSize=` | Paged list, newest first. |
| `POST /api/v1/jira-integrations/{id}/outbound-jobs/{jobId}/retry` | `dead` to `pending`, `attempts = 0`, `next_attempt_at = now()`. 409 when the job is not `dead`. |

### 3.3 Capture (JI-43)

**Source 1: inbound events.** In `receive_jira_event` (`rest/jira_events.rs`), in the same transaction that stores the `jira_integration_events` row, enqueue one `comment` job for the event's issue when write-back and comments are on. No job for duplicates (`duplicate: true`), ignored events, or events with no per-environment result. The comment lists each result: `"qa: approve applied"`, `"prod: deploy refused: not approved"`, plus unknown environments and features. The issue that sent the event gets the comment even without a link row. Also enqueue a `remote_link` job for each linked feature that has at least one applied result.

**Source 2: the activity cursor** (`scheduler/jira_writeback_capture.rs`, every 5 s):

- A one-row table `jira_writeback_cursor(id BOOL PRIMARY KEY DEFAULT TRUE CHECK (id), last_created_at TIMESTAMPTZ NOT NULL)`. The first run inserts `now()` (no backfill).
- Each tick reads `activity_log` rows with `created_at` in `[last_created_at - 60 s, now() - 2 s]` and these types: `stage_approved`, `stage_rejected`, `stage_deployed`, `stage_rollbacked`, `kill_switch_activated`, `kill_switch_deactivated`, `external_link_added`, `external_link_removed`. The overlap catches transactions that committed late. `dedupe_key` makes re-reads harmless.
- For each row, resolve the feature, then its `jira` links, then the enabled integrations of the feature's team with write-back on.
- Mapping:

| Activity | Jobs |
|---|---|
| approve, reject, deploy, rollback, kill switch | `comment` (if comments are on) + `remote_link` (if the remote link is on) |
| `external_link_added` | `remote_link` only |
| `external_link_removed` | `remote_link_delete` only |

- Rows whose metadata has `approval_source = jira` came from an inbound event. Source 1 already commented, so these produce no `comment`, only the `remote_link` refresh.
- Comment text uses the activity actor name and environment, for example `"Approved for prod by Jane Doe in FluxGate"`, `"Deployed to qa (scheduled change)"`. It never includes metadata fields other than environment, status, actor name, `externalRef` and `reason`.
- After the batch commits, set `last_created_at` to the newest row's `created_at` read in this tick (or leave it when there were none).

### 3.4 Inbound rate limit (JI-44)

On `POST /api/v1/integrations/jira/{id}/events`, before the secret check and before parsing the body:

- Keyed limiter (`governor` crate) per integration id: 120 per minute, burst 60.
- One shared bucket for ids that do not exist: 30 per minute. The limiter must not grow with random ids: check existence with the cheap lookup the handler already does, or key unknown ids to the shared bucket.
- Over the limit: `429` with `Retry-After` (seconds). Nothing is stored in the event log. One `warn` line per integration per minute with the dropped count.
- Config: `[jira] inbound_per_minute`, `[jira] inbound_burst` (defaults above).

### 3.5 Native webhook HMAC (JI-45)

- `POST /api/v1/jira-integrations/{id}/native-webhook-secret` (team admin) generates a random secret, stores it in `native_webhook_secret_enc`, returns it once. Calling it again rotates. `DELETE` on the same path removes it.
- `receive_jira_event` accepts the request when the phase 1 secret (`Authorization: Bearer` or `X-FluxGate-Jira-Secret`) is valid, **or** when `X-Hub-Signature` is `sha256=<hex>` and equals HMAC-SHA256 of the **raw** body with the native secret. Constant-time compare. Only `sha256` is accepted. A missing or wrong signature with no valid secret is 401, as today.
- Native `jira:issue_updated` bodies already match the phase 1 parser. Replay protection stays on `delivery_hash`.
- First step of the task: confirm in Atlassian's docs which editions sign webhooks with `X-Hub-Signature` (Cloud admin webhooks with a secret; Data Center depends on version). Record the result in the task's handoff log. If Data Center does not sign, the code is unchanged and the guide says "Data Center: use Automation".

### 3.6 UI (JI-46, JI-47)

JI-46, in the Jira integration settings page from JI-22:

- "Write-back to Jira" section: base URL, edition (Cloud or Data Center), account email (Cloud only), credential as a write-only password input with a "saved" badge when `hasCredential`, toggles for comments and remote link, "Test connection" button showing the result.
- Paused banner when `pausedReason` is set, with "Resume".
- "Native webhook" block: "Generate secret" shows the secret once with a copy button; "Rotate" and "Remove".
- New "Outbound" tab next to the event log: status filter, issue key, kind, attempts, `last_error`, next attempt, "Retry" on `dead` rows.
- Design tokens only (`__tests__/designTokenGuard.test.ts`).

JI-47: attach `components/ai/ReasonQualityHint.tsx` to the stage change reason field from JI-21, the way `FeatureEmergencyActionModal.tsx` uses it. Advisory only; it never blocks submit.

### 3.7 Setup guide and end-to-end test (JI-50)

- `setup-guide.md`:
  - A new section on write-back: how to create a Cloud API token or DC PAT for a dedicated Jira user, the permissions it needs (browse, add comments, link issues), test connection, the paused state.
  - Section 2 gets option C, "Native webhook" (Cloud, and DC if JI-45 confirmed it).
  - The network section: FluxGate needs outbound access to Jira; the built-in rate limit; the proxy limit becomes optional.
- `api-tests/src/tests/advanced/jira-writeback.test.ts` with a small Node HTTP server as a fake Jira that records requests.

## 4. Testing

| Level | What |
|---|---|
| Backend unit | Comment text and ADF builder; remote link title builder; backoff schedule; result classification; HMAC check (valid, wrong signature, wrong prefix, missing header, body changed); rate limiter burst then 429. |
| Backend integration (test DB) | Capture dedupe across the overlap window; `approval_source = jira` rows produce no comment; remote link coalescing; write-back off cancels pending jobs; credential never in a response. |
| Sender with a mock Jira | Add `wiremock` as a dev dependency. 2xx to `sent`; 429 with `Retry-After`; 401 pauses the integration; 404 to `dead`; no redirect followed; `last_error` has no `Authorization` value. |
| api-tests | Fake Jira server: an inbound event produces a comment; a human approval produces a comment and a remote link; a 401 pauses write-back; a native webhook with a valid signature is applied and with a bad one is 401; burst over the limit gets 429. |
| UI | Vitest for the new settings section, the Outbound tab and the reason hint. |

Test credentials and secrets are generated at runtime. None are committed.

## 5. Tasks

Same rules as phase 1 ([`../README.md`](../README.md#rules-for-agents)): one task at a time, in order, commit on `main`, failing test first.

| ID | Title | Repo | Depends on |
|---|---|---|---|
| JI-40 | Audit: every stage-status write path writes an activity row; add missing rows and a test | backend | — |
| JI-41 | Write-back configuration, encrypted credential, test connection | backend | — |
| JI-42 | Outbound jobs table, sender, retry and pause, job endpoints | backend | JI-41 |
| JI-43 | Capture from inbound events and the activity cursor | backend | JI-40, JI-42 |
| JI-44 | Inbound rate limit | backend | — |
| JI-45 | Native webhook HMAC auth | backend | JI-41 |
| JI-46 | UI: write-back settings, native secret, Outbound tab, paused banner | UI | JI-42, JI-45 |
| JI-47 | UI: `ReasonQualityHint` on the reason field | UI | — |
| JI-50 | Setup guide update and end-to-end write-back test | docs + api-tests | JI-43, JI-44, JI-45 |

Order: JI-40, JI-41, JI-42, JI-43, JI-44, JI-45, JI-46, JI-47, JI-50.

## 6. Risks and open points

- Data Center webhook signing is not confirmed (§3.5).
- Cloud comment ADF and the remote link payload were taken from Atlassian docs, not run against a real site. JI-50 notes this for the first team.
- Jira Cloud API rate limits: the sender's 50 jobs per 5 s tick and 429 handling should be enough for one team. Revisit if many teams share one Jira site.
- The capture cursor relies on `activity_log.created_at`. The 60 s overlap covers transactions shorter than 60 s. A longer transaction that writes a stage activity could be missed; JI-40 should confirm none exists.
