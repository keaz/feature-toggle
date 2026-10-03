# JI-30: Jira setup guide (Cloud and Data Center) and end-to-end API test

| Field | Value |
|---|---|
| Type | Docs + test |
| Status | Done in `1ba9d09` |
| Repo | backend (`feature-toggle/`): `docs/jira-integration/`, `api-tests/` |
| Depends on | JI-15 (and JI-12 if it was built) |
| Behavior change | None |
| Design | [design.md §3.7-3.10](../design.md#37-jira-integration-and-status-rules-ji-13), [§4](../design.md#4-testing) |

## Goal

Give teams a copy-paste setup for Jira Automation, and prove the whole flow with one API test that runs as Jira would.

## Current code (verify first)

| Piece | Where |
|---|---|
| API test harness | `api-tests/` (Jest + axios, pnpm): `pnpm --dir api-tests run test:docker` |
| System-client helpers | `api-tests/src/tests/system-client.test.ts`: `POST /teams/{teamId}/system-clients` returns `{systemClient, token}`; `createTokenClient(token)` |
| Approval flow tests | `api-tests/src/tests/advanced/stage-transition-approvals.test.ts`, `approval-workflow.test.ts`, `stage-deployment.test.ts` |
| Wiki | `../fluxgate.wiki/` documents webhooks that do not exist; do not copy from it |

## Changes

1. **`api-tests/src/tests/advanced/jira-flow.test.ts`** (Jira is simulated with axios posts of Jira-shaped bodies to the inbound endpoint):
   1. Admin sets up a team, environments `qa` and `prod`, a feature with stages in both, an approval policy for both, and a human approver user.
   2. Create a Jira integration (JI-13): `environment_field = customfield_10042`, alias `QA → qa`, Jira approves in `qa` only. Rules: `Ready for Release → approve`, `Done → deploy`, `Ready for Release → request` limited to `prod`.
   3. Link `PROJ-1` to the feature (JI-10).
   4. Event "Ready for Release", field `QA`: `qa` stage is `DEPLOYMENT_APPROVED`; the approval request has `approvalSource = 'jira'` and the Jira display name.
   5. Event "Done", field `QA`: `qa` stage is `DEPLOYED`.
   6. Event "Done", field `Prod` before any approval: result `refused` ("not approved"), stage unchanged.
   7. Event "Ready for Release", field `Prod`: the `approve` rule is refused (not trusted) and the `request` rule leaves a pending request; the human approves it in FluxGate; event "Done", field `Prod` deploys.
   8. Wrong secret → 401. Same delivery twice → second response repeats the stored result, no new activity.
   9. The integration's event log lists all events with results.
2. **`docs/jira-integration/setup-guide.md`**, for an admin who has never seen FluxGate's API:
   - FluxGate side: create the integration, copy the secret and inbound URL, choose the environment field and aliases, choose Jira-approved environments (with the security note: anyone who can move the issue to the status can then release there; restrict the transition in the Jira workflow), write the rules, link issues to features.
   - Jira side, one static setup:
     - Jira Cloud: Automation rule "When issue transitioned" → "Send web request" POST to the inbound URL, header `Authorization: Bearer <secret>`, body "Issue data (Jira format)". Or a system webhook (admin) for issue updates; check whether it can send the header, else use Automation.
     - Jira Data Center: Automation for Jira with the same rule, or a webhook if your version can send custom headers.
   - Finding the field id (`customfield_…`) in Jira.
   - Network: Cloud calls come from Atlassian's published IP ranges; restrict ingress to the inbound path. Data Center usually runs inside the network.
   - Reading results: the event log in Settings → Jira; the Jira Automation audit log shows the response body.
   - Error and outcome table: 400, 401, `refused` reasons (not approved, not trusted, freeze window, dependency), `no-op`, unknown environment values.
   - Not supported yet: comments or transitions back to Jira, kill switch, mapping Jira users to FluxGate users.
   - If JI-12 was built: the per-transition by-key alternative (design §3.10).
3. Link the guide from this folder's README.

## Done when

- `jira-flow.test.ts` passes (`test:docker`, or against a local backend as `HANDOFF.md` §3 describes when Docker is not available).
- The recipe has been followed by hand once against a local stack with `curl` standing in for Jira (record the commands in the handoff log, without tokens).
- Handoff log entry written, `HANDOFF.md` and README table updated; phase 1 marked complete.

## Handoff log

### 2026-10-03: done in `1ba9d09`

**What changed**

- `api-tests/src/tests/advanced/jira-flow.test.ts` (8 tests). Setup: team, environments `qa` (Staging) and `prod` (Production), one policy (`specific_environments`, both), a human approver, a feature with stages qa → prod (a two-stage feature needs a relationship), integration with `environmentField = customfield_10042`, aliases `QA`/`Prod`, Jira approves in qa only, rules `Ready for Release → approve`, `Done → deploy`, `Ready for Release → request` (prod only), link `PROJ-<random>`. Jira is simulated with axios posts of a Cloud webhook body (status changelog, option-object custom field) and an Automation `{issue, user}` body with the `X-FluxGate-Jira-Secret` header.
- Assertions follow the task list 1-9: qa approved as Jira (`approvalSource = 'jira'`, `externalApprover` snake_case keys, `externalRef`), qa deployed, prod `deploy` refused `not approved` with the stage unchanged, prod `approve` refused `environment not approved by Jira` and `request` leaves a pending `fluxgate` request, human approves, prod deployed, wrong and missing secret 401, a repeated delivery returns `duplicate: true` with the same `eventId` and results and no new feature activity (`GET /activity/recent?entityType=feature&entityId=`), event log total 6 (the replay and the 401s are not stored), newest first.
- `docs/jira-integration/setup-guide.md`: FluxGate side (integration, secret, Events URL, aliases, Jira-approved environments with the security note, rules with the policy / no-policy note, links), Jira side (Automation "Send web request" on Cloud and Data Center; body option A "Issue data (Jira format)", option B custom body with `initiator`), finding the field id, network, reading results, HTTP status and outcome tables, not supported, by-key alternative (JI-12). Linked from the README.

**Decisions taken in this task**

- The guide recommends Automation only. Jira's native webhooks cannot add an `Authorization` header (they only sign the body with their own secret), and the inbound endpoint has no other way to authenticate. A header-adding proxy is mentioned as possible, not documented.
- "Issue data (Jira format)" does not include the user who moved the issue, so the guide offers a custom body with `{{initiator.accountId}}` / `{{initiator.displayName}}` (Data Center: `{{initiator.key}}`). The Jira smart values in the guide were written from Atlassian's documentation, not run against a real Jira site; the backend accepts the shapes they render to (checked by the parser tests and the manual run).
- `design.md` §3.10 named the page `automation-recipe.md`; the task file names it `setup-guide.md`, which was used.

**Verified**

- Backend `cf2213e` built, run on `127.0.0.1:18180` against `feture_toggle_test` (HANDOFF §3). `API_BASE_URL=http://127.0.0.1:18180/api/v1 pnpm --dir api-tests exec jest --runInBand jira-flow`: 8 passed. `pnpm --dir api-tests exec tsc --noEmit -p .`: clean. Docker is still not installed, so `test:docker` was not run.
- Mutation check: adding prod to `jiraApprovedEnvironmentIds` fails 3 tests (untrusted approve, human approval step, event log).
- Manual run of the guide with curl standing in for Jira (no tokens recorded; the secret stayed in a shell variable):
  ```bash
  # as api-test-admin: team, environment "qa" (no policy), feature with a qa stage, then
  POST /teams/$TEAM/jira-integrations  {"name":"Jira PROJ","environmentField":"customfield_10042","environmentAliases":{"QA":"$QA"},"jiraApprovedEnvironmentIds":["$QA"]}
  PUT  /jira-integrations/$ID/rules     {"rules":[{"jiraStatus":"Ready for Release","action":"approve"},{"jiraStatus":"Done","action":"deploy"}]}
  POST /features/$FEATURE/external-links {"system":"jira","externalKey":"PROJ-30","url":"https://acme.atlassian.net/browse/PROJ-30"}
  # Jira: POST /integrations/jira/$ID/events with an Automation body {issue:{key,fields:{status,customfield_10042:{value}}},user}
  ```
  Results: wrong secret 401; "Done" first → `deploy refused "not approved"`; "Ready for Release" → `approve applied NOT_DEPLOYED → DEPLOYMENT_APPROVED` (no policy: trusted approve moves the stage directly); "Done" with `X-FluxGate-Jira-Secret` → `DEPLOYED`; value `Staging-EU` → `results: []`, `unknownEnvironments: ["Staging-EU"]`; body `not json` → 400, stored with `error`; event log total 5.

**Open points (phase 2)**

- No rate limit on the public inbound route (from JI-15); the guide tells admins to add one at the proxy.
- No write-back to Jira; `ReasonQualityHint` on the stage change reason field (from JI-21).
