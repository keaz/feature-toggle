# JI-30: Jira Automation recipe (Cloud and Data Center) and end-to-end API test

| Field | Value |
|---|---|
| Type | Docs + test |
| Status | Open |
| Repo | backend (`feature-toggle/`): `docs/jira-integration/`, `api-tests/` |
| Depends on | JI-01, JI-10, JI-11, JI-12 |
| Behavior change | None |
| Design | [design.md §3.6](../design.md#36-jira-automation-recipe-ji-30), [§4](../design.md#4-testing) |

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

1. **`api-tests/src/tests/advanced/jira-flow.test.ts`**:
   1. Admin sets up team, environment, feature with a stage, approval policy (reuse helpers from the approval tests), and a human approver user.
   2. Create a system client for the team; use only its token for the "Jira" steps.
   3. Jira links `PROJ-1` to the feature; `GET /teams/{teamId}/features?externalKey=PROJ-1` returns it.
   4. Jira requests `DEPLOYMENT_REQUESTED` by key with `externalRef: "PROJ-1"` and a reason.
   5. Jira tries to approve the request → 403 `system_client_vote_not_permitted`.
   6. The human approver approves; the request shows `externalRef` and `requestReason`.
   7. Jira requests `DEPLOYED` by key; `GET` by key shows the stage `DEPLOYED`.
   8. Activity log of the feature has the `external_ref`.
   9. A system client of another team gets 403 on the by-key routes.
2. **`docs/jira-integration/automation-recipe.md`**: the setup from design §3.6, written for an admin who has never seen FluxGate's API:
   - prerequisites, creating and rotating the system client, which scopes it uses;
   - one rule per transition with the exact URL, headers and JSON body, using Jira smart values (`{{issue.key}}`, `{{issue.summary}}`, a custom field for the feature key);
   - read-back rule (scheduled, `GET` by key) and how to branch on stage status;
   - Cloud vs Data Center: network reachability, Atlassian IP allow-list for Cloud, the "Send web request" action exists in both;
   - error table: 400, 403 (scope, team, vote), 404 (`feature_not_found`, `environment_not_found`, `stage_not_found`), 409 (state machine) and what the rule should do;
   - what is not supported yet (push to Jira, approving from Jira, kill switch).
3. Link the recipe from the repo `ReadME.md` or `FluxGate-System-Guide.md` only if that file already has an integrations section; otherwise link it from this folder's README.

## Done when

- `pnpm --dir api-tests run test:docker` passes, including `jira-flow.test.ts`.
- The recipe has been followed by hand once against a local stack with `curl` standing in for Jira (record the commands in the handoff log, without tokens).
- Handoff log entry written, `HANDOFF.md` and README table updated; phase 1 marked complete.

## Handoff log

(empty)
