# Jira setup guide (Jira Cloud and Jira Data Center)

This guide connects a Jira project to FluxGate, so that moving an issue to a status (for example "Ready for Release" or "Done") requests, approves, deploys or rolls back a feature in the environment the issue names. You need to be a FluxGate admin or a Team Admin of the team, and a Jira admin (or project admin) who can create Automation rules.

How it works, in one paragraph: every time an issue changes status, Jira sends the issue to one FluxGate URL. FluxGate reads the issue key, the new status and an environment field from the issue. It finds the features linked to the issue, finds the status rules that match the status, and runs each rule for each feature and environment. Rules are configured in FluxGate, not in Jira, so the Jira side is one static rule that you set up once.

Contents:

1. [FluxGate side](#1-fluxgate-side)
2. [Jira side](#2-jira-side)
3. [Find the environment field id](#3-find-the-environment-field-id)
4. [Network](#4-network)
5. [Check the result](#5-check-the-result)
6. [Responses and outcomes](#6-responses-and-outcomes)
7. [Not supported yet](#7-not-supported-yet)
8. [Alternative: one Automation rule per transition](#8-alternative-one-automation-rule-per-transition)

## 1. FluxGate side

All of this is in the FluxGate UI under **Settings → Jira** (`/settings/jira`). The API calls are given too, for scripted setups; they need a FluxGate user token (`Authorization: Bearer <user token>`) of an admin or Team Admin.

### 1.1 Create the integration

Create one integration per Jira project (or per group of projects that share the same environment field and rules). Choose:

- **Name**: unique in the team, for example `Jira PROJ`.
- **Jira base URL** (optional): `https://acme.atlassian.net`. The UI uses it for links.
- **Environment field**: the Jira field that names the target environment. Either `labels` or a custom field id such as `customfield_10042` (see [section 3](#3-find-the-environment-field-id)). A select list, a multi-select list, a text field or labels all work. A multi-value field targets every environment it lists.
- **Environment aliases** (optional): Jira value → FluxGate environment. Values are compared ignoring case and surrounding spaces. A value that has no alias is matched to an active environment of the team with the same name (ignoring case). Add an alias when the Jira value differs from the environment name (`QA env` → `qa`), or when two environments share a name.
- **Jira-approved environments**: environments where Jira is a **trusted approver**. In these environments an `approve` rule approves the stage on behalf of Jira; elsewhere `approve` is refused and logged. Leave this empty if humans must approve in FluxGate.

> **Security note.** In a Jira-approved environment, anyone who can move the issue to the approving status can release the feature there. Restrict that transition in the Jira workflow (a transition condition such as "user is in group release-managers"), and keep production out of the list unless the Jira workflow is your approval process.

```bash
curl -X POST "$FLUXGATE/api/v1/teams/$TEAM_ID/jira-integrations" \
  -H "Authorization: Bearer $USER_TOKEN" -H 'Content-Type: application/json' \
  -d '{
        "name": "Jira PROJ",
        "jiraBaseUrl": "https://acme.atlassian.net",
        "environmentField": "customfield_10042",
        "environmentAliases": {"QA": "<qa environment id>", "Prod": "<prod environment id>"},
        "jiraApprovedEnvironmentIds": ["<qa environment id>"]
      }'
```

The response holds `integration.id` and `secret`. **Copy the secret now**: FluxGate stores only its hash and shows it once. If you lose it, use **Rotate secret** (`POST /api/v1/jira-integrations/{id}/rotate-secret`) and update the Jira rule. Rotating makes the old secret stop working at once.

The **inbound URL** is shown on the integration page as **Events URL**:

```
https://<your FluxGate backend>/api/v1/integrations/jira/<integration id>/events
```

Use the public URL of the backend (the host Jira can reach), not the UI's URL if they differ.

### 1.2 Write the status rules

A rule is: Jira status → action, optionally limited to some environments. Statuses are compared ignoring case and surrounding spaces. Several rules can match one status; they run in list order.

| Action | What it does |
|---|---|
| `request` | Requests deployment. If an approval policy applies, a pending approval request is created and humans approve it in FluxGate. |
| `approve` | Jira-approved environments only. Approves the stage as Jira: closes the pending request (or requests and approves in one step). The approval records the Jira user, issue key and status. |
| `deploy` | Deploys a stage that is already approved. On a stage that is not approved it is **refused**; it never approves. |
| `rollback` | Requests a rollback of a deployed stage. In a Jira-approved environment it also approves and completes the rollback. |

Example: Jira approves and deploys QA; production needs a human approval in FluxGate.

| Jira status | Action | Environments |
|---|---|---|
| Ready for Release | `approve` | all (only QA is Jira-approved, so it is refused for Prod) |
| Ready for Release | `request` | Prod |
| Done | `deploy` | all |

```bash
curl -X PUT "$FLUXGATE/api/v1/jira-integrations/$INTEGRATION_ID/rules" \
  -H "Authorization: Bearer $USER_TOKEN" -H 'Content-Type: application/json' \
  -d '{"rules": [
        {"jiraStatus": "Ready for Release", "action": "approve"},
        {"jiraStatus": "Ready for Release", "action": "request", "environmentIds": ["<prod environment id>"]},
        {"jiraStatus": "Done", "action": "deploy"}
      ]}'
```

The `PUT` replaces the whole list.

Approval policies matter:

- With a policy on the environment, `request` leaves a pending request for the policy's approvers. A trusted `approve` closes it.
- Without a policy, `request` moves the stage to `DEPLOYMENT_REQUESTED` and it stays there: only a trusted Jira `approve` (or a reject in FluxGate) moves it on. In an environment without a policy, use `approve` (Jira-approved) rather than `request`, or add a policy.

### 1.3 Link issues to features

FluxGate acts only on features linked to the issue. Open the feature in FluxGate and add the issue key in the **Jira** card, or:

```bash
curl -X POST "$FLUXGATE/api/v1/features/$FEATURE_ID/external-links" \
  -H "Authorization: Bearer $USER_TOKEN" -H 'Content-Type: application/json' \
  -d '{"system": "jira", "externalKey": "PROJ-123", "url": "https://acme.atlassian.net/browse/PROJ-123"}'
```

One issue can be linked to several features, and one feature to several issues. As an alternative, set **Feature key field** on the integration to a Jira field that holds FluxGate feature keys; FluxGate then also acts on the features named there.

## 2. Jira side

Use a Jira **Automation** rule on both Cloud and Data Center. It is the only Jira feature that can send FluxGate's secret in a header.

> Jira's built-in webhooks (System → WebHooks) cannot add an `Authorization` header; at most they sign the body with their own secret, which FluxGate does not check. FluxGate rejects every request without its secret, so plain webhooks do not work. FluxGate does accept the webhook body format, so a proxy that adds the header in front of FluxGate is possible, but Automation is simpler.

### 2.1 Jira Cloud

Project settings → **Automation** → **Create rule**:

1. **Trigger**: "Issue transitioned" (called "Work item transitioned" in newer Jira Cloud). Leave "From status" and "To status" empty, so every status change is sent; FluxGate's rules decide what to do. Restrict it to statuses if you want fewer events.
2. **Action**: "Send web request".
   - **Web request URL**: the inbound URL from section 1.1.
   - **HTTP method**: `POST`.
   - **Headers**: `Authorization` = `Bearer <secret>`. Tick **Hidden** so the secret is not shown in the rule or the audit log. (`X-FluxGate-Jira-Secret: <secret>` works too.) Add `Content-Type` = `application/json` if you use a custom body.
   - **Web request body**: one of the two options below.
   - Tick **Delay execution of subsequent rule actions until we've received a response** so the audit log shows FluxGate's response.
3. Name the rule (for example "FluxGate release"), and turn it on.

**Body option A, simple: "Issue data (Jira format)".** Jira sends the whole issue. FluxGate reads the key, `fields.status.name` and the environment field from it. This body does not say who moved the issue, so approvals show "Approved by Jira" without a person.

**Body option B, with the Jira user: "Custom data".** Sends only what FluxGate needs, plus the user who triggered the rule:

```json
{
  "issue": {
    "key": "{{issue.key}}",
    "fields": {
      "status": {"name": "{{issue.status.name}}"},
      "customfield_10042": {{issue.customfield_10042.value.asJsonStringArray}},
      "updated": "{{issue.updated}}"
    }
  },
  "user": {"accountId": "{{initiator.accountId}}", "displayName": "{{initiator.displayName}}"}
}
```

Replace `customfield_10042` with your field id (twice). For a single-select field you can also use `"customfield_10042": "{{issue.customfield_10042.value}}"`; for a text field `"{{issue.customfield_10042}}"`; for labels `"labels": {{issue.labels.asJsonStringArray}}` with `environmentField = labels`. Use **Validate** in the rule editor to check the rendered JSON against a real issue.

### 2.2 Jira Data Center

Automation for Jira is part of Jira Data Center (it was a Marketplace app in older versions; install it if your version does not have it). Create the same rule: **Issue transitioned** → **Send web request**, `POST`, header `Authorization: Bearer <secret>`, body "Issue data (Jira format)" or a custom body.

For a custom body on Data Center, identify the user by key or name instead of an account id:

```json
"user": {"key": "{{initiator.key}}", "displayName": "{{initiator.displayName}}"}
```

If your Automation version has no `asJsonStringArray`, use body option A or the single-value form `"{{issue.customfield_10042.value}}"`.

### 2.3 Restrict who can release

FluxGate trusts whatever the rule sends for Jira-approved environments. Protect the approving status in the Jira workflow (transition conditions or validators), and limit who can edit the Automation rule and the environment field.

## 3. Find the environment field id

FluxGate needs the field **id** (`customfield_10042`), not its name.

- **Jira Cloud**: Settings → Issues → **Custom fields** → the field's **⋯** → Edit details (or Contexts). The URL ends in `id=10042`; the field id is `customfield_10042`. Or call `GET https://<site>/rest/api/3/field` and search for the field name.
- **Jira Data Center**: Administration → Issues → **Custom fields** → the field's cog → Configure. The URL contains `customFieldId=10042`. Or call `GET <base url>/rest/api/2/field`.
- **Any version**: open `<base url>/rest/api/2/issue/PROJ-123?expand=names` in a browser while logged in. The `names` object maps each `customfield_…` id to its display name, and `fields` shows the value format.

To use labels instead, set the environment field to `labels` and add a label per environment (`qa`, `prod`) to the issue.

## 4. Network

- The inbound endpoint is public (no FluxGate login): it is protected only by the integration secret. Expose only `POST /api/v1/integrations/jira/*/events` to Jira, not the whole API.
- **Jira Cloud** calls come from Atlassian's published egress IP ranges (<https://ip-ranges.atlassian.com/>; Atlassian's "IP addresses and domains for Atlassian cloud products" page lists the ones Automation uses). Allow only those ranges on that path at your proxy or firewall.
- **Jira Data Center** usually runs inside your network. Allow only the Jira nodes.
- FluxGate has no rate limit on this endpoint. Add one at the reverse proxy before exposing it to the internet.
- Bodies over 1 MiB are refused (413).

## 5. Check the result

- **FluxGate**: Settings → Jira → the integration → **Event log**. Each event lists the issue, the status, the Jira user, and one line per feature and environment with the rule, action, outcome, the stage status before and after, and the reason when refused. Unknown environment values and unknown feature keys are listed too. Events are kept 30 days. API: `GET /api/v1/jira-integrations/{id}/events?offset=0&limit=20`.
- **Jira**: the Automation rule's **Audit log** shows FluxGate's response body when "Delay execution … until we've received a response" is ticked.
- **Approvals**: an approval made by Jira shows "Approved by Jira (<display name>, <issue key>)" in FluxGate's approvals list. The activity log has an `approval_request_approved_externally` entry.

A response looks like this:

```json
{
  "eventId": "5f0c…",
  "results": [
    {"featureKey": "new-checkout", "environment": "qa", "ruleStatus": "Ready for Release",
     "action": "approve", "outcome": "applied", "from": "NOT_DEPLOYED", "to": "DEPLOYMENT_APPROVED",
     "reason": null, "approvalRequestId": "9a1e…"}
  ],
  "unknownEnvironments": [],
  "unknownFeatures": [],
  "duplicate": false
}
```

## 6. Responses and outcomes

HTTP status of the inbound call:

| Status | Meaning | What to do |
|---|---|---|
| 200 | Event processed (also when no rule matched or nothing changed). Read `results`. | — |
| 200, `duplicate: true` | Same delivery (issue, status, environments, change id) within 10 minutes. The stored result is returned; nothing runs again. | Nothing; Jira retried. |
| 200, `ignored: "no status change"` | A Jira webhook body without a status change. | Nothing. Automation bodies always count as a status change. |
| 400 | Body is not JSON, has no issue, a bad issue key, or (Automation body) no `fields.status.name`. Stored in the event log with the error. | Fix the body; check the rendered JSON with Validate. |
| 401 | Wrong or missing secret, unknown integration, or integration disabled. Same message for all; not stored. | Check the URL, the header and the Enabled switch; rotate the secret if unsure. |
| 413 | Body over 1 MiB. | Use a custom body. |
| 500 | Database error. Stored with the error. | Retry later; check the backend logs. |

Per-target `outcome` in `results`:

| Outcome | Reason (examples) | Meaning |
|---|---|---|
| `applied` | — | The stage moved from `from` to `to`. |
| `no_op` | — | Nothing to do: already requested, approved or deployed (or not deployed, for rollback). |
| `refused` | `not approved` | `deploy` on a stage that is not approved. Approve it first (a human in FluxGate, or a trusted `approve`). |
| `refused` | `environment not approved by Jira` | `approve` in an environment that is not Jira-approved. Use `request` and approve in FluxGate, or add the environment to the Jira-approved list. |
| `refused` | `freeze window <name>` | A freeze window is active. Jira cannot override a freeze; wait or override in FluxGate. |
| `refused` | `Rollout blocked: '<feature>' depends on '<dependency>' which is not deployed …` | A dependency is not deployed in this environment. Deploy it first. |
| `refused` | `feature has no stage in this environment` | The feature has no stage for the environment the issue names. |
| `refused` | `approval request already resolved` | A person approved or rejected the request at the same time. |
| `refused` | `<reason> (stage now <STATUS>)` | One step applied and a later one was refused, for example the request was created but the approval was not. |
| `error` | — | Infrastructure error for this target only; the other targets still ran. |

Values that match nothing are not errors:

- `unknownEnvironments`: environment field values with no alias and no active environment of that name (or a name shared by several environments). Add an alias.
- `unknownFeatures`: feature keys from the feature key field that do not exist in the team.
- No linked feature: `results` is empty. Link the issue (section 1.3).

## 7. Not supported yet

- Writing back to Jira (comments, transitions, status fields). Read the result in the event log or the Automation audit log, or poll FluxGate (section 8).
- Kill switch from Jira. Use FluxGate.
- Mapping Jira users to FluxGate users. A Jira approval records the Jira account id and display name only; it never counts as a FluxGate user's vote.
- Jira native webhooks without a header-adding proxy (section 2).

## 8. Alternative: one Automation rule per transition

Teams that prefer to keep the logic in Jira can call FluxGate's by-key endpoints from per-transition Automation rules, instead of the inbound endpoint and status rules. These calls use a **system client token** (FluxGate → Teams → System clients) with the `flag:write` and `admin:read` scopes, not the integration secret. Keep its expiry short and rotate it.

Request a stage change by feature key and environment name:

```
POST /api/v1/teams/{teamId}/features/by-key/{featureKey}/environments/{environmentName}/request-change
Authorization: Bearer <system client token>
{"request": "DEPLOYMENT_REQUESTED", "externalRef": "{{issue.key}}", "reason": "{{issue.summary}}"}
```

| Jira transition | `request` |
|---|---|
| "Ready for Staging" | `DEPLOYMENT_REQUESTED` (environment `staging`) |
| "Go Live" | `DEPLOYED` (works only after approval in FluxGate) |
| "Rollback" | `ROLLBACK_REQUESTED`, then `ROLLBACKED` after approval |

Read the status back with `GET /api/v1/teams/{teamId}/features/by-key/{featureKey}` (for example from a scheduled rule). The feature key comes from a Jira custom field. On this path a system client can request but never approve: approvals are always made by people in FluxGate.
