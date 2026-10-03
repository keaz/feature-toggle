# JI-30: Jira setup guide (Cloud and Data Center) and end-to-end API test

| Field | Value |
|---|---|
| Type | Docs + test |
| Status | Open |
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

(empty)
