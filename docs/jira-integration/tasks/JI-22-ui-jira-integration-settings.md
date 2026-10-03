# JI-22: UI, Jira integration settings, rules editor and event log

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done |
| Repo | UI (`../feature-toggle-ui/`) |
| Depends on | JI-13, JI-15 |
| Behavior change | New settings page. |
| Design | [design.md §3.5](../design.md#35-ui-ji-20-ji-21-ji-22), [§3.7](../design.md#37-jira-integration-and-status-rules-ji-13), [§3.9](../design.md#39-inbound-events-and-the-rule-engine-ji-15) |

## Goal

Let a team admin set up Jira integrations and change the status rules at any time (decision J10), and see what each Jira event did (decision J15).

## Current code (verify first, paths under `feature-toggle-ui/src`)

| Piece | Where |
|---|---|
| Settings page pattern | `pages/AiSettingsPage.tsx` (team-scoped admin page); SSO settings page (`/settings/sso`) for list + detail + secret handling |
| Routes | `routes/AppShellRoutes.tsx` (`/settings/ai` near line 92) and the settings navigation |
| API pattern | `api/client.ts`, one file per resource in `api/` |
| Environments list | existing environments API in `api/` |
| Tests | Vitest + Testing Library; `__tests__/designTokenGuard.test.ts` |

## Changes

1. `api/jiraIntegrations.ts`: types and calls for every JI-13 and JI-15 endpoint.
2. Page `pages/JiraSettingsPage.tsx`, route `/settings/jira`, team admins and admins only (same gate as the AI settings page). Sections:
   - **Integrations** list for the selected team; create (name, Jira base URL).
   - **Secret**: shown once after create or rotate, with copy button and a clear "you will not see this again" note; rotate button with confirmation.
   - **Inbound URL**: the full events URL (`<backend public URL>/api/v1/integrations/jira/<id>/events`) with copy button, and the header to use.
   - **Environment field**: field id (`labels` or `customfield_…`), alias table (Jira value → FluxGate environment select).
   - **Jira approves in**: multi-select of the team's environments, with a warning text that Jira status changes approve without a FluxGate approver there.
   - **Feature key field** (optional).
   - **Rules** editor: rows of Jira status (text), action (request/approve/deploy/rollback), environments filter (optional multi-select), enabled; reorder up/down; save sends the whole list (`PUT`). Show a warning on an `approve` rule when no environment trusts Jira, and on a `deploy` rule with no `approve` rule (it will only work after a human approval).
   - **Event log**: newest first, paged: time, issue key, Jira status, Jira user, and per result feature, environment, action, outcome (applied/no-op/refused) with reason. Refused rows use the warning token style.
3. Navigation entry under Settings.

## Tests (write first)

- Rules editor: add, reorder, remove, save payload order; duplicate status+action shows the server's 400 message.
- Secret is displayed after create and rotate and not after reload.
- Event log renders applied, no-op and refused results.
- Non-admins do not see the page.
- Design token guard passes.

## Done when

- `pnpm lint`, `pnpm build`, `pnpm test:run` pass.
- Checked by hand against a backend with JI-15: create integration, set rules, send a curl event, see it in the log.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

### 2026-10-03, UI `131060d`

- `api/jiraIntegrations.ts`: types and calls for every JI-13 and JI-15 endpoint (`listJiraIntegrations`, `createJiraIntegration`, `updateJiraIntegration`, `deleteJiraIntegration`, `rotateJiraIntegrationSecret`, `listJiraStatusRules`, `replaceJiraStatusRules`, `listJiraEvents`). `JIRA_RULE_ACTIONS` lists the actions.
- `utils/jiraIntegrations.ts`: `jiraInboundEventsUrl(id)` (REST base URL from `config.js`, a relative base is resolved against the UI origin), `JIRA_SECRET_HEADER`, rule draft helpers (`toRuleDrafts`, `toRuleInputs`, `moveItem`) and `ruleWarnings`.
- Page `pages/JiraSettingsPage.tsx`, route `/settings/jira`, nav item "Jira" under Settings with no extra gate (the group gate is `teamsManagement`, so admins and Team Admins see it). The page gate is `canAccessTeamsManagement`, like the backend policy; the AI page is admin only, this one is not.
- Components in `components/jira/`: `JiraIntegrationSettingsForm` (name, base URL, environment field, aliases, "Jira approves in" with the trust warning, feature key field, enabled; saves the whole form with `PATCH`, empty base URL / feature key field clear them), `JiraRulesEditor` (loads and `PUT`s the whole list; add, reorder, remove, enable; blank status blocked in the UI; server 400 shown as is), `JiraEventLog` (20 per page, refresh button, refused uses the warning token style).
- Warnings: approve rule whose environments (or, without a filter, the integration) have no Jira-approved environment; deploy rule when there is no enabled approve rule. The approve warning follows the **saved** "Jira approves in" list, not unsaved form edits.
- Secret: kept in page state only, shown on create and rotate with a copy button and an "I stored the secret" dismiss; gone after reload or team switch. Delete is a hard delete on the backend (rules and events cascade), so the page asks for confirmation.
- Tests: `utils/jiraIntegrations.test.ts`, `components/jira/__tests__/JiraRulesEditor.test.tsx`, `components/jira/__tests__/JiraEventLog.test.tsx`, `pages/__tests__/JiraSettingsPage.test.tsx`, nav test cases. `pnpm lint`, `pnpm build`, `pnpm test:run` (90 files, 709 tests) pass.
- Manual check (backend on `:8080` against `feture_toggle_test`, `pnpm dev --port 8090`, Chrome DevTools MCP, `api-test-admin`): created integration `JI-22 Jira` in team `JI-21 check 1791039213`, set "Jira approves in" = production, rules "Ready for Release" → approve and "Done" → deploy. Curl webhooks for `PROJ-12` (linked to `ji21-checkout-1791039213`): approve applied, deploy applied (with unknown environment `staging`), repeat deploy no-op; after clearing "Jira approves in", approve refused. Wrong secret → 401. After reload the event log showed all four with the right badges, the secret was gone and the approve rule showed its warning. The integration is left in the test DB with no Jira-approved environment; the feature's production stage is now `DEPLOYED`.
