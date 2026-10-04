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
| J21 | Results are captured from two sources: the inbound event (stored with its jobs in one transaction), and an **`activity_log` cursor** for every other change. *Revised 2026-10-03 after a code check:* approval decisions (vote, auto-approval, capped reconciliation, cancel) and approval-gated requests write **no** activity row today, and several stage paths write theirs best effort after the change. JI-40 adds the missing rows. A best-effort row that fails to write means a missed comment; the next change refreshes the remote link. This is accepted. |
| J22 | On 401 or 403 from Jira, write-back for that integration **pauses** until an admin saves a new credential or resumes. FluxGate does not retry a bad token. |
| J23 | The public inbound route gets an **in-process rate limit** (single-server backend, see project memory). Events over the limit are not stored. |
| J24 | Native Jira webhooks are accepted when signed with **HMAC-SHA256** (`X-Hub-Signature: sha256=<hex>`). A secret in the URL is not supported, because URLs end up in proxy and access logs. |
| J25 | *Added 2026-10-04 (§3.8).* When Jira approves a request (`approve_stage_change_externally`), its approval-risk assessment is **skipped** if it has not finished: the judgment row becomes `skipped` and no Jev call is made. If a call is already running, its result is dropped: no `done` row and no `approval_risk_assessed` activity row. A finished assessment is kept as history. A human vote, a cancel and auto-approval do not skip, so nothing changes for them. |
| J26 | *Added 2026-10-04 (§3.8).* The approval-risk input sent to Jev carries the request's `external_ref` and `reason` (JI-11), so Jev sees the Jira issue key and why the change was asked for. The four questions and the derivation do not change. |
| J27 | *Added 2026-10-04 (§3.8).* A stage change reason that a **person** typed is recorded for the justification check after the change, with kind `stage_change` (JI-47), on the activity row that holds the reason. A reason that FluxGate generates for a Jira status rule (`Jira status '<status>'`) is not recorded, because it is not a person's justification. |

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

UI base URL for links: new optional config `[jira] ui_base_url`. When it is not set, use `allowed_origin` if that is one absolute `http(s)` URL (`public_base_url` is the backend URL, not the UI). If neither gives a URL, enabling the remote link returns 400 `ui base URL is not configured`.

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
   - Remote link: `POST {base}/rest/api/{v}/issue/{key}/remotelink` with `globalId = fluxgate:feature:<featureId>`, `object.url` = the FluxGate UI feature page `<ui base>/features/<featureId>` (see §3.1 for the UI base), `object.title` = `FluxGate: <featureKey> · <env> <STATUS> · ...` in pipeline order. Same `globalId` means Jira updates the existing link.
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
| `GET /api/v1/jira-integrations/{id}/outbound-jobs?status=&offset=&limit=` | Paged list, newest first. Uses `offset` and `limit` (default 50, at most 200), like the JI-15 event log. |
| `POST /api/v1/jira-integrations/{id}/outbound-jobs/{jobId}/retry` | `dead` to `pending`, `attempts = 0`, `next_attempt_at = now()`. 409 when the job is not `dead`. |

### 3.3 Capture (JI-43)

**Source 1: inbound events.** In `receive_jira_event` (`rest/jira_events.rs`), in the same transaction that stores the `jira_integration_events` row, enqueue one `comment` job for the event's issue when write-back and comments are on. No job for duplicates (`duplicate: true`), ignored events, or events with no per-environment result. The comment lists each result: `"qa: approve applied"`, `"prod: deploy refused: not approved"`, plus unknown environments and features. The issue that sent the event gets the comment even without a link row. Source 1 enqueues only this comment. The remote link refresh for applied results comes from Source 2, because every applied Jira action writes an activity row (`stage_deployed`, `stage_rollbacked`, `stage_change_requested` or `approval_request_approved_externally`). To store the event and its job in one transaction, the handler generates the event id before the insert (`NewJiraEvent.id`), and `dedupe_key = event:<eventId>`.

**Source 2: the activity cursor** (`scheduler/jira_writeback_capture.rs`, every 5 s):

- A one-row table `jira_writeback_cursor(id BOOL PRIMARY KEY DEFAULT TRUE CHECK (id), last_created_at TIMESTAMPTZ NOT NULL)`. The first run inserts `now()` (no backfill).
- Each tick reads `activity_log` rows with `created_at` in `[last_created_at - 60 s, now() - 2 s]`, oldest first, of the types in the mapping table below. The overlap catches transactions that committed late. `dedupe_key` makes re-reads harmless.
- Every row of these types carries `metadata.feature_id` (stage rows use `entity_type = 'stage'`, so read the feature from metadata, not from `entity_id`). For each row, resolve the feature's `jira` links, then the enabled integrations of the feature's team with write-back on.
- Mapping:

| Activity | Jobs |
|---|---|
| `stage_approved`, `stage_rejected` (added by JI-40), `stage_deployed`, `stage_rollbacked`, `approval_request_cancelled` (added by JI-40), `kill_switch_activated`, `kill_switch_deactivated` | `comment` (if comments are on) + `remote_link` (if the remote link is on) |
| `stage_change_requested`, `approval_request_approved_externally`, `feature_updated` with `metadata.target_version_id` (version rollback restores stage statuses) | `remote_link` only |
| `external_link_added` | `remote_link` only |
| `external_link_removed` | `remote_link_delete` only |

- **Jira-made rows get no comment.** A row whose `actor_id` is the `actor_user_id` of any Jira integration came from an inbound event, and Source 1 already commented. These rows produce only the `remote_link` refresh. (Only `approval_request_approved_externally` carries `approval_source`; the Jira-made `stage_deployed` and `stage_rollbacked` rows do not, so the actor is the reliable marker.)
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

JI-47: attach `components/ai/ReasonQualityHint.tsx` to the stage change reason field from JI-21, the way `FeatureEmergencyActionModal.tsx` uses it. Advisory only; it never blocks submit. The justification check has no stage-change kind today, so the backend gets `ReasonKind::StageChange` (`stage_change`) first (contract update), then the UI `ReasonKind` type.

### 3.7 Setup guide and end-to-end test (JI-50)

- `setup-guide.md`:
  - A new section on write-back: how to create a Cloud API token or DC PAT for a dedicated Jira user, the permissions it needs (browse, add comments, link issues), test connection, the paused state.
  - Section 2 gets option C, "Native webhook" (Cloud, and DC if JI-45 confirmed it).
  - The network section: FluxGate needs outbound access to Jira; the built-in rate limit; the proxy limit becomes optional.
- `api-tests/src/tests/advanced/jira-writeback.test.ts` with a small Node HTTP server as a fake Jira that records requests.

### 3.8 AI judgments and Jira (JI-51, JI-52, JI-53)

*Added 2026-10-04, after a check of how phase 1 and phase 2 affect the AI judgments ([`../../ai-judgments/design.md`](../../ai-judgments/design.md)).* Jira changes go through the normal stage path, so they get the same approval-risk assessment as a person's change. Three gaps were found:

1. When Jira approves a request, the request closes before the queued assessment runs. AI-11 cannot act on a closed request (phase 1 §3.8 records `ai_risk_mode_skipped`), but the Jev call was still made, and a late `approval_risk_assessed` row ("AI risk assessment: high") appeared after Jira had approved. For the `approve` action from `NOT_DEPLOYED`, the request and the approval happen in the same event, so the call was always wasted.
2. The assessment input had no `external_ref` or `reason`, so Jev never saw the Jira issue or why the change was asked for.
3. The JI-47 hint checks a stage change reason before submit, but the reason was never recorded for the justification check after submit, as emergency, cleanup and freeze override reasons are.

**JI-51: skip the assessment of a request Jira approved (J25).**

- Migration `20261004110000_ai_judgments_skipped_status.sql`: `ai_judgments.status` allows `skipped`. A skipped row is final: `skipped` + `error` (the reason text) + `completed_at`.
- `AiJudgmentRepository::skip_unfinished(subject_type, subject_id, kind, reason) -> bool` sets `status = 'skipped'` only where `status IN ('pending','failed')`.
- The guards of `start_attempt`, `mark_done` and `mark_failed` change from `status <> 'done'` to `status IN ('pending','failed')`. So a skipped row is never started, finished or failed. `claim_retryable` already reads only `pending` and `failed`. `upsert_pending` still reopens a row on a new submission; a closed request gets none.
- `JudgmentService::skip(subject_type, subject_id, kind, reason)`.
- `ApprovalLogicImpl::approve_stage_change_externally`, after the commit of the path with a pending request: skip the `approval_risk` judgment of that request with `skipped: approval request approved by <source> before the assessment ran`. A failure is logged and does not fail the approval. The activity row also gets `"ai_risk_assessment": "skipped"` when the policy's mode is not `off`, written before the commit like `ai_risk_mode_skipped`. The skip runs after the commit, so it can find nothing to skip if the assessment has already finished.
- `rest/approval.rs` `map_ai_risk`: `skipped` gives `aiRisk: null`, the same as no assessment, so there is no contract change. The approvals UI already shows "Approved by Jira (...)" for such a request.
- A run already in flight when the skip lands: `mark_done` matches no row, the run returns `Stale`, and `apply` does not run. The API call was made; nothing is shown.

**JI-52: Jira context in the approval-risk input (J26).**

- `approval_risk::build_input` takes the request's `external_ref` and `request_reason`. `change` gets two keys, always present: `"external_ref": "<key>" | null` and `"reason": "<text>" | null`. `reason` is cut to 500 characters (`MAX_TEXT_CHARS`).
- The questions and the derivation do not change. `overall_risk` already says "Use `change` and `impact`".
- Privacy: `reason` is free text from the requester. The justification check already sends the same kind of text to Jev. Owner, user ids and emails are still never included.
- `input_hash` changes for new requests only. Stored judgments are not re-run.

**JI-53: record stage change reasons for the justification check (J27).**

- `StageChangeMeta` gets `check_reason: bool` (default `false`). The REST stage route and the by-key route (`perform_stage_change`, `rest/feature.rs`) set it to `true` when a reason is present. `logic/external_change.rs`, used for Jira status rules, leaves it `false`. So does the scheduled-change scheduler, because its reason was already checked as `scheduled_change` when the change was created.
- `FeatureLogicImpl` gets an optional `Arc<JudgmentService>`, passed in `lib.rs`. This is the same service that `ApprovalLogicImpl` already holds.
- In `request_stage_change`, after the change and its activity row (`stage_change_requested` for a request with an approval, or the direct row), when `check_reason` is on and a reason is present: `justification::record_justification(.., SubjectType::Activity, <activity id>, ReasonKind::StageChange, reason, Some(feature key))`. The verdict is merged into that row's `metadata.ai_justification`, as for the other kinds. The team toggle, the rule check and "never fails the caller" apply as before.
- If the activity row cannot be written (the write is best effort), nothing is recorded.

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
| JI-40 | Activity rows for approval decisions (vote, auto-approval, capped reconciliation), cancel and gated requests | backend | — |
| JI-41 | Write-back configuration, encrypted credential, test connection | backend | — |
| JI-42 | Outbound jobs table, sender, retry and pause, job endpoints | backend | JI-41 |
| JI-43 | Capture from inbound events and the activity cursor | backend | JI-40, JI-42 |
| JI-44 | Inbound rate limit | backend | — |
| JI-45 | Native webhook HMAC auth | backend | JI-41 |
| JI-46 | UI: write-back settings, native secret, Outbound tab, paused banner | UI | JI-42, JI-45 |
| JI-47 | `ReasonQualityHint` on the stage change reason field (backend `stage_change` kind, then UI) | backend + UI | — |
| JI-50 | Setup guide update and end-to-end write-back test | docs + api-tests | JI-43, JI-44, JI-45 |
| JI-51 | Skip the AI risk assessment of a request Jira approved | backend | JI-14 |
| JI-52 | Jira reference and reason in the AI risk input | backend | JI-11 |
| JI-53 | Record stage change reasons for the justification check | backend | JI-47 |

Order: JI-40, JI-41, JI-42, JI-43, JI-44, JI-45, JI-46, JI-47, JI-50, then the follow-ups JI-51, JI-52, JI-53 (added 2026-10-04).

## 6. Risks and open points

- Data Center webhook signing is not confirmed (§3.5).
- Cloud comment ADF and the remote link payload were taken from Atlassian docs, not run against a real site. JI-50 notes this for the first team.
- Jira Cloud API rate limits: the sender's 50 jobs per 5 s tick and 429 handling should be enough for one team. Revisit if many teams share one Jira site.
- The capture cursor relies on `activity_log.created_at`. The 60 s overlap covers transactions shorter than 60 s. A longer transaction that writes a stage activity could be missed.
- Best-effort activity rows (`let _ = log_activity(...)` in `request_stage_change`, the kill-switch scheduler, canary) can fail silently; that change then gets no comment.
- Deleting a feature cascades its links without an `external_link_removed` row, so the remote link stays on the issue, pointing to a missing page.
