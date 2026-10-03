# Jira integration: work packages

This folder holds the design and the task breakdown for the first Jira integration phase: linking features to Jira issues and letting Jira issue status drive request, approval, deployment and rollback per environment, through rules configured in FluxGate. Read [`design.md`](design.md) once, then [`HANDOFF.md`](HANDOFF.md) for the current state. Then take the **next open task** from the table below. Each task file says what to change, where, how to test it, and when it is done. One agent can finish one task without reading the other task files.

Source investigation: [`../investigations/2026-10-sso-jira-sdks.md`](../investigations/2026-10-sso-jira-sdks.md), section 2.

## Rules for agents

1. **One task at a time, in table order.** Do not start a task until every task in its "Depends on" column is done and committed. Do not work on two tasks in one session.
2. **Commit directly on `main`** in each repo (project practice; no feature branches). Stage files by explicit path: both repos have unrelated local changes.
   - Backend tasks go in this repo (`feature-toggle/`).
   - UI tasks go in `../feature-toggle-ui/`, a separate git repo.
3. **Re-check the code first.** Line numbers come from backend `6c9b332` and UI `dc16a2f` and will drift. Find code by symbol name (`grep -n "fn <name>"`). If the code no longer matches the task, stop and write that in the task's handoff log.
4. **Stay in scope.** Change only what the task lists. If you need a visible behavior change that the task does not list, stop and ask.
5. **Write the failing test first**, then the change. Each task lists its tests.
6. **Run before you finish:**
   - Backend:
     ```bash
     cargo fmt
     cargo clippy --all-targets
     export DATABASE_URL="${DATABASE_URL%/*}/feture_toggle_test"   # seeded test DB
     cargo test -p feature-toggle-backend
     ./scripts/check-contract-compat.sh        # when REST DTOs or endpoints changed
     ```
   - UI (in `../feature-toggle-ui/`):
     ```bash
     pnpm lint
     pnpm build           # also the type check
     pnpm test:run        # use pnpm, never npm
     ```
7. **Repo gotchas:**
   - Never edit an applied migration. Add a new timestamped one: `migrations/YYYYMMDDHHMMSS_name.sql`, later than every existing file (latest today: `20261004010000_approval_request_external_ref.sql`).
   - Prefer runtime `sqlx::query(...)`. If you add a `query!`/`query_as!` macro, run `cargo sqlx prepare -- --all-targets` in `feature-toggle-backend/` and commit `.sqlx/`.
   - New REST endpoints and DTO fields: register them in `ApiDoc` (`rest/mod.rs`). Then run `./scripts/export-contracts.sh` and copy `feature-toggle-backend/contracts/generated/contract-hashes.json` to `contracts/baseline/`. Say so in the commit message.
   - Any write that changes evaluable feature state must send a `FeatureUpdate` on the broadcast channel. External links and request metadata do not change evaluation, so they need no broadcast.
   - The UI fails tests on raw Tailwind palette classes or hex colors (`__tests__/designTokenGuard.test.ts`). Use token classes such as `bg-warning/10` or `text-destructive`.
   - After changing code, run `graphify update .` in the repo you changed (`graphify-out/` is not committed).
8. **Never** put a system-client token, a Jira API token or a database password in a file, a log line, a test fixture or a commit.
9. **When done:**
   - Set the task's `Status` row to `Done in <commit>`.
   - Append a dated entry to the task's **Handoff log**: what changed, what you verified (commands and results), open questions, and anything the next task must know.
   - Update [`HANDOFF.md`](HANDOFF.md): status table, "next task", and any new fact a later task needs.
   - Tick the task in the table below.
   - Commit the doc changes (`docs(jira): ...`).
10. **If you stop before done**, append a handoff log entry with exactly where you stopped (last commit, failing test, next step), and update `HANDOFF.md`. The next agent continues from that entry.

## Decisions (2026-10-03)

| # | Decision |
|---|---|
| J1 | Support Jira Cloud and Jira Data Center. Jira calls into FluxGate (a Jira webhook or an Automation "Send web request" rule). No Forge app, no bridge service, no outbound webhooks in this phase. |
| J2 | *Revised 2026-10-03, see J10.* Humans approve in FluxGate unless an environment is opted in to Jira approval. A system-client token can never vote through the vote endpoint (JI-01). |
| J3 | Feature ↔ issue links live in a new `feature_external_links` table. The `jira:PROJ-123` tag convention is not used. |
| J4 | `externalRef` and `reason` are optional on stage change requests. They are stored on the approval request and in the activity log metadata. |
| J5 | Jira addresses features by team, feature key and environment name, not by UUID (JI-12, optional). |
| J6 | Jira learns the result by reading FluxGate (`GET` by key) or from the integration's event log in the UI. Push to Jira (webhooks, comments) is a later phase. |
| J7 | Kill switch from Jira is out of scope. System clients stay denied on emergency endpoints. |
| J8 | Security prerequisites P1-P8 from the investigation are already fixed (verified 2026-10-03 against `6c9b332`). |
| J9 | System clients are never eligible approvers; their shadow users keep the `Approver` role row (2026-10-03, JI-01). |
| J10 | **Jira status drives rollout (2026-10-03).** A team configures a Jira integration in FluxGate with **status rules**: Jira status → action. Rules are data, edited in the UI, never hard-coded. |
| J11 | Rule actions: `request` (pending approval request, humans approve in FluxGate), `approve`, `deploy`, `rollback`. |
| J12 | **Jira is a trusted approver** for environments an admin opts in. An `approve` rule closes the approval request as approved by Jira; the audit stores the Jira user (account id, display name), issue key and status. FluxGate does not map the Jira user to a FluxGate user. In environments not opted in, `approve` is refused and logged. |
| J13 | The environment comes from a **Jira field** on the issue (custom field or labels), mapped to FluxGate environments by name or by an alias map in the integration. A multi-value field targets each environment it lists. |
| J14 | `deploy` on a stage that is not approved is **refused**: no change, a logged event result and an activity row. It never approves implicitly. |
| J15 | Every inbound Jira event and its per-feature, per-environment result is stored and shown in the UI, so rule behavior can be checked and debugged. |

## Tasks

| Done | ID | Title | Repo | Behavior change | Depends on |
|---|---|---|---|---|---|
| [x] | [JI-01](tasks/JI-01-system-clients-cannot-vote.md) | System clients cannot approve or reject approval requests, and are not eligible approvers | backend | **Yes** (403 for M2M votes; bots not eligible; user decision 2026-10-03) | — |
| [x] | [JI-10](tasks/JI-10-feature-external-links-backend.md) | `feature_external_links` table, CRUD API, `externalKey` list filter | backend | Additive | — |
| [x] | [JI-11](tasks/JI-11-stage-change-external-ref-backend.md) | `externalRef` + `reason` on stage change, approval requests and activity | backend | Additive | — |
| [x] | [JI-13](tasks/JI-13-jira-integration-config-backend.md) | Jira integration config: integrations, environment field and map, Jira-approved environments, status rules, inbound secret | backend | Additive | JI-10 |
| [x] | [JI-14](tasks/JI-14-external-approval-path-backend.md) | Approval by an external system (Jira): approve, deploy, rollback paths with source and actor in the audit | backend | Additive (only reachable through JI-15) | JI-11 |
| [ ] | [JI-15](tasks/JI-15-jira-inbound-events-backend.md) | Inbound Jira events endpoint and rule engine, event log | backend | Additive | JI-13, JI-14 |
| [x] | [JI-12](tasks/JI-12-by-key-endpoints-backend.md) | By-key read and request-change endpoints (optional, for status read-back and manual rules) | backend | Additive | JI-11 |
| [ ] | [JI-20](tasks/JI-20-ui-feature-jira-links.md) | UI: Jira links panel on the feature page | UI | New UI | JI-10 |
| [ ] | [JI-21](tasks/JI-21-ui-external-ref-on-stage-changes.md) | UI: `externalRef`/`reason` inputs and display on approvals and activity, "approved by Jira" display | UI | New UI | JI-11, JI-14 |
| [ ] | [JI-22](tasks/JI-22-ui-jira-integration-settings.md) | UI: Jira integration settings, rules editor, event log | UI | New page | JI-13, JI-15 |
| [ ] | [JI-30](tasks/JI-30-jira-automation-recipe-and-e2e.md) | Jira setup guide (Cloud + DC) and end-to-end API test | docs + api-tests | None | JI-15 (JI-12 if built) |

## Order

```
JI-01 ─► JI-10 ─► JI-11 ─► JI-13 ─► JI-14 ─► JI-15 ─► JI-12 ─► JI-20 ─► JI-21 ─► JI-22 ─► JI-30
```

The work runs one task at a time, in this order. JI-12 is optional: skip it if the inbound rules cover every flow, and say so in `HANDOFF.md`.

## Later phases (not planned here)

From the investigation, in order: outbound webhooks (`webhook_endpoints`, HMAC signing, retry scheduler) so Jira gets status pushed; a bridge service for two-way sync; a Forge issue panel (Cloud only). Plan them as new task files in this folder when they are picked up.
