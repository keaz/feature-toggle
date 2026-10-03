# Jira setup guide (Jira Cloud and Jira Data Center)

This guide connects a Jira project to FluxGate, so that moving an issue to a status (for example "Ready for Release" or "Done") requests, approves, deploys or rolls back a feature in the environment the issue names. You need to be a FluxGate admin or a Team Admin of the team, and a Jira admin (or project admin) who can create Automation rules.

FluxGate can also write results back to the Jira issue (comments and a link to the feature), and Jira can call FluxGate through Automation or through a signed native webhook. Both are optional extras on top of the basic setup.

How it works, in one paragraph: every time an issue changes status, Jira sends the issue to one FluxGate URL. FluxGate reads the issue key, the new status and an environment field from the issue. It finds the features linked to the issue, finds the status rules that match the status, and runs each rule for each feature and environment. Rules are configured in FluxGate, not in Jira, so the Jira side is one static rule that you set up once.

Contents:

1. [FluxGate side](#1-fluxgate-side)
   - [Write-back: show results in Jira](#write-back-show-results-in-jira)
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

## Write-back: show results in Jira

Write-back is optional and off by default. When it is on, FluxGate writes to the Jira issues that are linked to a feature:

- a **comment** when something happens that a Jira reader cares about (approved, rejected, deployed, rolled back, approval cancelled, kill switch), and when FluxGate processes a Jira event (the outcome of each rule);
- one **remote link** per feature on the issue ("Web links"), titled with the feature key and the status of each stage. FluxGate updates the same link when a stage changes, and removes it when the issue is unlinked from the feature.

Set it up in this order.

1. **Create a dedicated Jira user** for FluxGate (a service account, not a person), so comments show a clear author.
   - **Jira Cloud**: sign in as that user and create an **API token** at <https://id.atlassian.com/manage-profile/security/api-tokens>. You need the user's email and the token.
   - **Jira Data Center**: create a **personal access token** for that user (profile → Personal Access Tokens). Data Center needs no email.
2. **Give the user these permissions** on the projects involved: **Browse projects**, **Add comments**, **Link issues**.
3. **In FluxGate**, open **Settings → Jira**, pick the integration, and open **Write-back**:
   - make sure the integration has a **Jira base URL** (section 1.1; `https` unless you allow `http`, see [section 4](#4-network)). The base URL must not contain credentials (`https://user:password@...`), a query (`?...`) or a fragment (`#...`); FluxGate refuses such a URL while write-back is on or being switched on ("Jira base URL must not contain credentials, a query or a fragment");
   - choose the **edition** (Cloud or Data Center), enter the **account email** (Cloud only) and the **token**;
   - switch on **Enable write-back**, and choose **Comments** and **Remote link**;
   - save, then press **Test connection**. FluxGate calls `GET /rest/api/<v>/myself` on your Jira and shows the Jira display name of the user, or the error.

   API: `PUT /api/v1/jira-integrations/{id}/writeback` with `{"enabled": true, "comments": true, "remoteLink": true, "authKind": "cloud_basic", "accountEmail": "bot@example.com", "credential": "<token>"}` (`authKind` is `cloud_basic` for Cloud and `dc_pat` for Data Center), then `POST /api/v1/jira-integrations/{id}/writeback/test`.
4. **Link issues to features** (section 1.3). Write-back only writes to linked issues.

> **Changing the Jira base URL host clears the token.** If you change the host (scheme, host or port) of the base URL, FluxGate deletes the stored token and turns write-back off, so a token for one Jira is never sent to another. Jobs still queued for the old host are marked `dead`, even when write-back was already off. Enter the token again and switch write-back on again.

### What appears on the issue

A comment for a human action (Cloud shows it as paragraphs):

```
FluxGate: new-checkout
Approved for prod by Jane Doe
Reason: ready for release
Ref: REL-9
```

A comment for a Jira event (the outcome of each rule):

```
FluxGate: Jira status 'Ready for Release'
new-checkout · qa: approve applied
new-checkout · prod: request applied
```

The remote link has the title `FluxGate: new-checkout · qa DEPLOYED · prod DEPLOYMENT_APPROVED` and opens the feature in the FluxGate UI. Its URL is built from `[jira] ui_base_url` in the backend config, or from `allowed_origin` when `ui_base_url` is not set. If neither gives a usable URL, the remote link job is skipped (it ends as `sent` with a note that starts with `skipped:`), and comments still work.

**Comments are never posted for changes that Jira itself made.** When a Jira event approves, deploys or rolls back a stage, the event comment above already reports it; a second comment for the same change would be noise. The remote link is still refreshed.

### Paused state, Resume and the Outbound tab

- If Jira answers **401 or 403** (the token is wrong, expired or lacks permission), FluxGate **pauses** write-back for that integration. The Write-back section shows the reason. Jobs that are already queued wait; nothing more is sent.
- **Resume** (or saving a new token) clears the pause, and the waiting jobs go out. API: `POST /api/v1/jira-integrations/{id}/writeback/resume`.
- Other failures (timeouts, 5xx, 429 from Jira) are retried with a growing delay and end as `dead` after the last attempt.
- **Settings → Jira → Outbound tab** (next to Events) lists the jobs: `pending`, `sent` or `dead`, with the last error. Press **Retry** on a `dead` job to send it again. API: `GET /api/v1/jira-integrations/{id}/outbound-jobs?status=dead` and `POST /api/v1/jira-integrations/{id}/outbound-jobs/{jobId}/retry`. Turning write-back off marks the pending jobs `dead` with the note `write-back disabled`; disabling the whole integration (the **Enabled** switch) does the same with `integration disabled`. **Retry** answers 409 `write-back is off` while the integration or its write-back is off.

### Security

- The token is stored **encrypted** in the database. The backend needs the environment variable `FLUXGATE_ENCRYPTION_KEY` (base64 of 32 random bytes, for example `openssl rand -base64 32`); without it FluxGate refuses to save a token (`encryption_key_missing`). Keep the key outside the repository and the config file, and back it up: a lost key means the token must be entered again.
- FluxGate **never shows the token again**, not in the UI, the API or the activity log. Replace it by entering a new one.
- Use a dedicated user with only the three permissions above, so a leaked token can do little.
- **Who controls the target host.** A team admin sets the Jira base URL, and the backend then sends HTTPS requests to that host with the token. So a team admin can point write-back at **any https host the backend can reach**, including internal services (server-side request forgery). Give team admin rights only to trusted people, and if the backend runs inside a private network, restrict its outbound traffic (egress firewall or proxy) to your Jira hosts.
- On **Data Center**, comments are plain text that Jira renders as wiki markup. FluxGate escapes the markup characters (`[ ] { } | ! * _ ^ ~ + - ? #`, a leading `h1.` or `bq.`, and `\`) in every comment, so a reason or feature key cannot add links, images or macros. Cloud comments are sent as ADF and need no escaping.

## 2. Jira side

There are two ways for Jira to call FluxGate:

- **Automation** (options A and B in 2.1 and 2.2) sends FluxGate's secret in an `Authorization` header. It works on every Cloud and Data Center version that has Automation.
- **Native webhook** (option C, section 2.4) uses Jira's built-in webhook and its Secret field. Jira signs the body, and FluxGate checks the signature. Use it when you prefer not to maintain an Automation rule.

Both reach the same Events URL and the same status rules. A plain webhook without a Secret does not work: FluxGate rejects every request that has neither its secret nor a valid signature.

### 2.1 Jira Cloud (Automation)

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

### 2.2 Jira Data Center (Automation)

Automation for Jira is part of Jira Data Center (it was a Marketplace app in older versions; install it if your version does not have it). Create the same rule: **Issue transitioned** → **Send web request**, `POST`, header `Authorization: Bearer <secret>`, body "Issue data (Jira format)" or a custom body.

For a custom body on Data Center, identify the user by key or name instead of an account id:

```json
"user": {"key": "{{initiator.key}}", "displayName": "{{initiator.displayName}}"}
```

If your Automation version has no `asJsonStringArray`, use body option A or the single-value form `"{{issue.customfield_10042.value}}"`.

### 2.3 Restrict who can release

FluxGate trusts whatever the rule sends for Jira-approved environments. Protect the approving status in the Jira workflow (transition conditions or validators), and limit who can edit the Automation rule and the environment field.

### 2.4 Option C: native webhook

A Jira webhook can replace the Automation rule. Jira signs the request body with a secret that you share with FluxGate, and FluxGate accepts a request when the signature is valid.

**Which versions.** Jira **Cloud** signs webhook bodies (Secret field in the webhook form), and this is confirmed. **Recent Data Center versions** that offer a webhook **Secret** field sign the same way (Atlassian's current Data Center documentation, the Managing webhooks page of version 11.3, describes it; that is simply the version the page belongs to, not the first version with the field, which Atlassian does not name). If your Data Center webhook form has no Secret field, use Automation (2.2).

1. **Create the FluxGate native secret.** In FluxGate: **Settings → Jira**, the integration, **Native webhook secret → Generate**. Copy the secret at once; FluxGate shows it once and stores it encrypted (it needs `FLUXGATE_ENCRYPTION_KEY`). **Rotate** gives a new one and the old one stops working. **Remove** turns native webhooks off for the integration. API: `POST /api/v1/jira-integrations/{id}/native-webhook-secret` (returns `{"secret": "..."}`) and `DELETE` on the same path. This secret is different from the integration secret of section 1.1.
2. **Create the webhook in Jira.**
   - **Jira Cloud**: Settings → System → **WebHooks** → **Create a WebHook**.
   - **Jira Data Center**: Administration → System → **WebHooks** → **Create a WebHook**.
3. Fill the form:
   - **URL**: the Events URL from section 1.1.
   - **Secret**: the FluxGate native secret.
   - **Events**: only **Issue → updated** (`jira:issue_updated`). FluxGate acts on status changes; the other events add load and nothing else.
   - **JQL filter**: limit it to the project (and issue types) you use, for example `project = PROJ`. Without a filter Jira sends every update of every issue.
4. Save, move a linked issue to a status, and check the **Event log** (section 5).

How FluxGate checks it: Jira sends the header `X-Hub-Signature: sha256=<hex>`, where `<hex>` is the HMAC-SHA256 of the **raw request body** with the secret as the key. FluxGate compares in constant time. A request that has the correct signature needs no `Authorization` header. A wrong or missing signature answers **401**, with the same message as a wrong secret. The `Authorization` / `X-FluxGate-Jira-Secret` secret keeps working next to the native secret.

You can test the signature by hand:

```bash
BODY='{"webhookEvent":"jira:issue_updated", ...}'
SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$NATIVE_SECRET" | sed 's/^.* //')
curl -X POST "$EVENTS_URL" -H 'Content-Type: application/json' \
  -H "X-Hub-Signature: sha256=$SIG" --data-binary "$BODY"
```

A native webhook body has no custom data, so the issue must carry the environment field (or labels) FluxGate reads, as in option A. Approvals show the Jira user from the webhook's `user` field.

## 3. Find the environment field id

FluxGate needs the field **id** (`customfield_10042`), not its name.

- **Jira Cloud**: Settings → Issues → **Custom fields** → the field's **⋯** → Edit details (or Contexts). The URL ends in `id=10042`; the field id is `customfield_10042`. Or call `GET https://<site>/rest/api/3/field` and search for the field name.
- **Jira Data Center**: Administration → Issues → **Custom fields** → the field's cog → Configure. The URL contains `customFieldId=10042`. Or call `GET <base url>/rest/api/2/field`.
- **Any version**: open `<base url>/rest/api/2/issue/PROJ-123?expand=names` in a browser while logged in. The `names` object maps each `customfield_…` id to its display name, and `fields` shows the value format.

To use labels instead, set the environment field to `labels` and add a label per environment (`qa`, `prod`) to the issue.

## 4. Network

- The inbound endpoint is public (no FluxGate login): it is protected only by the integration secret or the native webhook signature. Expose only `POST /api/v1/integrations/jira/*/events` to Jira, not the whole API.
- **Jira Cloud** calls come from Atlassian's published egress IP ranges (<https://ip-ranges.atlassian.com/>; Atlassian's "IP addresses and domains for Atlassian cloud products" page lists the ones Automation uses). Allow only those ranges on that path at your proxy or firewall.
- **Jira Data Center** usually runs inside your network. Allow only the Jira nodes.
- **Outbound (write-back)**: FluxGate needs outbound **HTTPS** to the Jira site (Cloud: `https://<site>.atlassian.net`). Allow it in the backend's egress firewall or proxy rules.
- Bodies over 1 MiB are refused (413).

### Built-in inbound rate limit

The events endpoint has a built-in limit, so a loop or a flood cannot overload FluxGate:

- **120 events per minute per integration**, with a **burst of 60**. Over the limit, FluxGate answers **429** with a `Retry-After` header (seconds). Jira Automation and webhooks retry later.
- Requests for **unknown integration ids** share one bucket of **30 per minute** (burst 10), so guessing ids does not get through.
- Change the numbers in the backend config:

  ```toml
  [jira]
  inbound_per_minute = 120
  inbound_burst = 60
  ```

  A value of 0 means the default.

> **Important.** The limit runs **before** the secret check, because checking the secret costs work too. So anyone who knows an integration id can use up that integration's budget and make real Jira events get 429 for a while. Keep the Events URL private (do not paste it in tickets or chat). For an internet-facing setup, also add a **limit per source IP** at your reverse proxy, and allow only Jira's source addresses on the path.

A proxy limit is optional for setups inside a private network.

### Backend settings for write-back

These keys are in the `[jira]` section of the backend config file (see `$FEATURE_TOGGLE_CONFIG`); restart the backend after a change.

| Key | Default | Meaning |
|---|---|---|
| `ui_base_url` | none | The FluxGate UI origin used for the remote link URL, for example `https://fluxgate.example.com`. When unset, `allowed_origin` is used. |
| `allow_insecure_http` | `false` | Allow an `http://` Jira base URL. For labs only: the token travels unencrypted. Without it, **Test connection** answers "Jira base URL must use https" and the sender pauses write-back with the same message. |
| `inbound_per_minute` | `120` | Sustained events per minute for one integration. |
| `inbound_burst` | `60` | Events one integration may send at once. |

## 5. Check the result

- **FluxGate**: Settings → Jira → the integration → **Event log**. Each event lists the issue, the status, the Jira user, and one line per feature and environment with the rule, action, outcome, the stage status before and after, and the reason when refused. Unknown environment values and unknown feature keys are listed too. Events are kept 30 days. API: `GET /api/v1/jira-integrations/{id}/events?offset=0&limit=20`.
- **Outbound**: Settings → Jira → the integration → **Outbound** tab shows each comment and remote link job and its status (see Write-back above). On the issue, look for the FluxGate comments and the "FluxGate: ..." web link.
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
| 401 | Wrong or missing secret, wrong native webhook signature, unknown integration, or integration disabled. Same message for all; not stored. | Check the URL, the header (or the webhook Secret) and the Enabled switch; rotate the secret if unsure. |
| 413 | Body over 1 MiB. | Use a custom body. |
| 429 | Rate limit of the integration (or of unknown ids) reached. `Retry-After` gives the seconds to wait. Not stored. | Wait and retry; Jira retries by itself. Raise `[jira] inbound_per_minute` / `inbound_burst` if real traffic hits it. |
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

Status of an outbound write-back job (Outbound tab):

| Status | Meaning | What to do |
|---|---|---|
| `pending` | Waiting to be sent, or waiting for a retry (the capture and the sender run every 5 seconds). While write-back is paused, jobs stay `pending`. | Nothing; check the paused reason if it does not move. |
| `sent` | Jira accepted it. A `sent` job whose last error starts with `skipped:` was not sent on purpose (for example no usable UI URL for the remote link). | For `skipped:`, set `[jira] ui_base_url`. |
| `dead` | Gave up after the retries, or was cancelled (`write-back disabled`). The last error holds the Jira status and the start of its answer. | Fix the cause (permission, issue key), then **Retry**. |

Values that match nothing are not errors:

- `unknownEnvironments`: environment field values with no alias and no active environment of that name (or a name shared by several environments). Add an alias.
- `unknownFeatures`: feature keys from the feature key field that do not exist in the team.
- No linked feature: `results` is empty. Link the issue (section 1.3).

## 7. Not supported yet

- Changing the Jira issue itself (transitions, status or custom fields). Write-back adds comments and a remote link only. Read the rest in the event log, or poll FluxGate (section 8).
- Kill switch from Jira. Use FluxGate.
- Mapping Jira users to FluxGate users. A Jira approval records the Jira account id and display name only; it never counts as a FluxGate user's vote.
- Forge apps and a bridge between FluxGate and Jira. Use Automation or a native webhook (section 2).

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
