# JI-46: UI for write-back settings, native secret, Outbound tab and paused banner

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
| Repo | UI (`../feature-toggle-ui/`, separate git repo) |
| Depends on | JI-42, JI-45 |
| Behavior change | New UI on the Jira settings page |
| Design | [design.md §3.6](../design.md#36-ui-ji-46-ji-47) |

## Goal

A team admin can configure write-back, test the connection, resume a paused integration, manage the native webhook secret and see outbound job delivery, all from `/settings/jira`.

## Current code (verify first, paths under `feature-toggle-ui/src`)

| Piece | Where |
|---|---|
| API client | `api/jiraIntegrations.ts` (JI-22): types and calls for integration CRUD, rules, events |
| Page | `pages/JiraSettingsPage.tsx`, route `/settings/jira`, gate `canAccessTeamsManagement` |
| Components | `components/jira/JiraIntegrationSettingsForm.tsx`, `JiraRulesEditor.tsx`, `JiraEventLog.tsx` (20 per page, refresh, warning token for refused) |
| Secret shown once | page state pattern for the inbound secret (copy button, "I stored the secret" dismiss) in `JiraSettingsPage.tsx` |
| Tests | `components/jira/__tests__/*`, `pages/__tests__/JiraSettingsPage.test.tsx`, `utils/jiraIntegrations.test.ts`; `__tests__/designTokenGuard.test.ts` |
| Backend contract | `GET /api/v1/openapi.json` on a running backend; DTO names in JI-41, JI-42, JI-45 interfaces |

## Interfaces

- Consumes these backend endpoints and DTOs:
  - From JI-41: `PUT .../writeback`, `POST .../writeback/test`, `POST .../writeback/resume`, `JiraIntegrationResponse.writeback`, `hasNativeWebhookSecret`.
  - From JI-42: `GET .../outbound-jobs?status=&offset=&limit=` and `POST .../outbound-jobs/{jobId}/retry`.
  - From JI-45: `POST` and `DELETE .../native-webhook-secret`.
- Produces in `api/jiraIntegrations.ts`:
  ```ts
  export type JiraAuthKind = 'cloud_basic' | 'dc_pat';
  export interface JiraWriteback { enabled: boolean; comments: boolean; remoteLink: boolean; authKind: JiraAuthKind | null;
    accountEmail: string | null; hasCredential: boolean; pausedReason: string | null }
  export interface UpdateJiraWritebackInput { enabled: boolean; comments: boolean; remoteLink: boolean;
    authKind?: JiraAuthKind; accountEmail?: string; credential?: string }   // credential omitted = keep stored
  export interface JiraOutboundJob { id: string; issueKey: string; featureId: string | null;
    kind: 'comment' | 'remote_link' | 'remote_link_delete'; status: 'pending' | 'sent' | 'dead';
    attempts: number; nextAttemptAt: string; lastError: string | null; createdAt: string; sentAt: string | null }
  export function updateJiraWriteback(id: string, input: UpdateJiraWritebackInput): Promise<JiraIntegration>;
  export function testJiraWriteback(id: string): Promise<{ ok: boolean; status: number | null; message: string }>;
  export function resumeJiraWriteback(id: string): Promise<JiraIntegration>;
  export function listJiraOutboundJobs(id: string, params: { status?: string; offset: number; limit: number }): Promise<{ items: JiraOutboundJob[]; total: number }>;
  export function retryJiraOutboundJob(id: string, jobId: string): Promise<JiraOutboundJob>;
  export function createNativeWebhookSecret(id: string): Promise<{ secret: string }>;
  export function deleteNativeWebhookSecret(id: string): Promise<void>;
  ```
- New components in `components/jira/`: `JiraWritebackSettings.tsx`, `JiraNativeWebhookSecret.tsx` and `JiraOutboundLog.tsx`.

## Behavior

- **`JiraWritebackSettings`:**
  - Fields:
    - "Enable write-back" toggle.
    - Edition select: Jira Cloud (`cloud_basic`) or Jira Data Center (`dc_pat`).
    - Account email, shown only for Cloud.
    - API token or PAT: `type="password"`, `autoComplete="off"`, empty by default. When `hasCredential` is true, show a "Saved" badge and the placeholder "Leave empty to keep the saved token".
    - Toggles: "Post comments", "Keep a FluxGate link on the issue".
  - Save sends `credential` only when the field is non-empty. After saving, clear the field. The token is never kept in state after save.
  - Show server 400 messages as they are.
  - "Test connection" shows the `message` and uses `ok` for the style (success or destructive token).
  - Paused banner (warning token) with `pausedReason` and a "Resume" button, shown when `pausedReason` is set.
  - Note under the base URL when write-back is on and `jiraBaseUrl` is empty: "Set the Jira base URL above first."
- **`JiraNativeWebhookSecret`:**
  - Shows "No native webhook secret" or "Native webhook secret set".
  - Buttons: "Generate" (or "Rotate"), and "Remove" with confirmation.
  - Shows the secret once, using the same copy and dismiss pattern as the inbound secret.
  - Help text: "Paste this secret into the Secret field of a Jira Cloud webhook. FluxGate checks the X-Hub-Signature header."
- **`JiraOutboundLog`:** a new tab "Outbound" next to the event log.
  - Status filter: all, pending, sent, dead. 20 per page, plus a refresh button.
  - Columns: created, issue key, kind, status, attempts, next attempt (pending only), last error.
  - "Retry" on `dead` rows; refresh after a retry.
  - `dead` uses the destructive token, `pending` the warning token.
- Design tokens only, and every text input has a label.

## Steps

- [ ] **Step 1: failing API tests** in `api/jiraIntegrations.test.ts`, or the existing util test file if that is where the API is tested:
  - Each new function hits the right method and path.
  - `updateJiraWriteback` leaves out `credential` when it is empty.
- [ ] **Step 2: failing component tests:**
  - `components/jira/__tests__/JiraWritebackSettings.test.tsx`:
    - The email field is hidden for Data Center.
    - Saving with an empty token sends no `credential`.
    - Saving with a token sends it, then clears the field.
    - The "Saved" badge shows when `hasCredential` is true.
    - The paused banner and Resume call `resumeJiraWriteback`.
    - Test connection shows its message.
  - `JiraNativeWebhookSecret.test.tsx`:
    - Generate shows the secret once.
    - After dismiss, the secret is gone from the DOM.
    - Remove asks for confirmation.
  - `JiraOutboundLog.test.tsx`:
    - The filter changes the query.
    - Retry is only on dead rows, and it calls the API, then refreshes.
- [ ] **Step 3:** run `pnpm test:run -- jira`. Expect failures.
- [ ] **Step 4:** implement the API functions and the components, then wire them into `JiraSettingsPage.tsx`:
  - The write-back section and the native secret block go under the existing settings form.
  - The Outbound tab goes next to the event log.
- [ ] **Step 5:** run `pnpm lint`, `pnpm build` and `pnpm test:run`. All pass. Then run `graphify update .` in `feature-toggle-ui/`.
- [ ] **Step 6: manual check.**
  - Backend on the test DB, `pnpm dev --port 8090`, and the Chrome DevTools MCP.
  - Configure write-back against a local fake Jira, then test the connection.
  - Generate a native secret.
  - Watch an outbound job go from pending to sent.
  - Record the results in the handoff log. Never put the token or the secret in the log.
- [ ] **Step 7:** commit in `feature-toggle-ui`: `feat(jira): write-back settings, native webhook secret and outbound log (JI-46)`. Stage the files by explicit path.

## Done when

- Everything in Behavior works against the real backend. Tokens and secrets never stay in the UI after save or dismiss.
- Lint, build and tests pass. Handoff log, `../HANDOFF.md` and the README table updated (`docs(jira): ...` commit in the backend repo).

## Handoff log

(empty)
