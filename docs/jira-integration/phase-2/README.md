# Jira integration, phase 2: work packages

Phase 2 makes FluxGate report results on the Jira issue (comments and a live remote link), hardens the public inbound route (rate limit, signed native webhooks) and adds the reason hint to the stage change form. Read [`design.md`](design.md) once, then the phase 2 section of [`../HANDOFF.md`](../HANDOFF.md). Then take the **next open task** from the table below.

> **For agentic workers:** each task file is self-contained: goal, current code, interfaces, steps (failing test first), done-when, handoff log. Use superpowers:subagent-driven-development or superpowers:executing-plans to run them one at a time. Steps use checkbox (`- [ ]`) syntax.

**Goal:** a Jira user sees, on the issue, what FluxGate did with each Jira event, every human approval decision and every stage change of linked features, without opening FluxGate.

**Architecture:** results become rows in an outbox table `jira_outbound_jobs`. Two capture paths fill it: the inbound event handler (same transaction as the event row) and a scheduler that reads `activity_log` with a cursor. A second scheduler sends due jobs to the Jira REST API with the integration's encrypted credential, retries with backoff and pauses the integration on 401/403.

**Tech stack:** Rust (Actix, sqlx, tokio, `reqwest`), PostgreSQL, React + TypeScript (pnpm, Vitest), Jest api-tests. New crates: `hmac`, `governor`, dev `wiremock`.

**Spec:** [`design.md`](design.md) (decisions J16-J24).

## Rules

Phase 1 rules apply unchanged: [`../README.md` § Rules for agents](../README.md#rules-for-agents). In short:
- One task at a time, in table order.
- Commit on `main`; stage by explicit path.
- Write the failing test first.
- Run fmt, clippy, tests and the contract check.
- Never edit an applied migration.
- Run `graphify update .` after code changes.
- Update the handoff log and `HANDOFF.md`.

Line numbers in the task files come from backend `b276078` and UI `131060d`. Find code by symbol name.

## Global constraints

- Credential, native webhook secret and inbound secret never appear in a response body, a log line, `last_error`, an activity row, a test fixture or a commit. Tests generate them at runtime.
- Credential and native webhook secret are sealed with `secret_box::encrypt_with_aad(value, integration_id.as_bytes())`. Without `FLUXGATE_ENCRYPTION_KEY`, return the existing `RestError::encryption_key_missing()`.
- Jira HTTP calls: timeout 10 s, `reqwest::redirect::Policy::none()`.
- Jira API versions: Cloud `rest/api/3` (ADF comment body), Data Center `rest/api/2` (plain text body).
- Remote link `globalId` = `fluxgate:feature:<featureId>`.
- Backoff after attempts 1-5: 30 s, 2 min, 10 min, 1 h, 6 h; job `dead` after attempt 6.
- Inbound rate limit defaults: 120 per minute, burst 60 per integration; 30 per minute shared for unknown ids.
- New migrations: later than `20261004040000_jira_integration_events.sql` and any migration added since (check `ls migrations | tail`).
- New REST endpoints and DTO fields: register in `ApiDoc` (`rest/mod.rs`), run `./scripts/export-contracts.sh`, copy the hashes to `contracts/baseline/`, say so in the commit message.
- Writes that only touch jobs, integration config or activity rows need no `FeatureUpdate` broadcast.
- UI: pnpm only; design tokens only (no raw palette classes or hex).

## Review focus

The five inputs most likely to break write-back in real use, each pinned by a test in the owning task:

1. **One activity row, several links and integrations.** A feature linked to two issues, in a team with two write-back integrations, must give one comment per issue per integration, never one per re-read. Test in JI-43 (`capture_fans_out_once_per_issue_and_integration`).
2. **Jira echo.** A change made by Jira must not produce a second comment from the activity cursor. The marker is the actor being an integration's `actor_user_id`. Test in JI-43 (`jira_made_rows_refresh_the_link_without_a_comment`).
3. **A credential typo.** A 401 must pause the integration after one job, not burn every pending job to `dead`. Test in JI-42 (`unauthorized_pauses_the_integration_and_keeps_other_jobs_pending`).
4. **Jira redirects or echoes the request.** A 3xx must not be followed. `last_error` must not contain the credential even when Jira echoes headers in its body. Tests in JI-42 (`redirect_is_not_followed`, `last_error_never_contains_the_credential`).
5. **A feature unlinked or deleted while jobs are pending.** A `remote_link` job for a feature with no row (`feature_id` NULL after delete) or no link any more must end `sent` as a delete or be dropped, not retry for 6 hours. Test in JI-42 (`remote_link_for_missing_feature_is_dropped`).

## Tasks

| Done | ID | Title | Repo | Depends on |
|---|---|---|---|---|
| [ ] | [JI-40](tasks/JI-40-activity-rows-for-approval-decisions.md) | Activity rows for approval decisions, cancel and gated requests | backend | — |
| [ ] | [JI-41](tasks/JI-41-writeback-config.md) | Write-back configuration, encrypted credential, test connection | backend | — |
| [ ] | [JI-42](tasks/JI-42-outbound-jobs-and-sender.md) | Outbound jobs table, sender, retry and pause, job endpoints | backend | JI-41 |
| [ ] | [JI-43](tasks/JI-43-capture.md) | Capture from inbound events and the activity cursor | backend | JI-40, JI-42 |
| [ ] | [JI-44](tasks/JI-44-inbound-rate-limit.md) | Inbound rate limit | backend | — |
| [ ] | [JI-45](tasks/JI-45-native-webhook-hmac.md) | Native webhook HMAC auth | backend | JI-41 |
| [ ] | [JI-46](tasks/JI-46-ui-writeback-settings.md) | UI: write-back settings, native secret, Outbound tab, paused banner | UI | JI-42, JI-45 |
| [ ] | [JI-47](tasks/JI-47-reason-hint-on-stage-change.md) | `ReasonQualityHint` on the stage change reason field | backend + UI | — |
| [ ] | [JI-50](tasks/JI-50-guide-and-e2e.md) | Setup guide update and end-to-end write-back test | docs + api-tests | JI-43, JI-44, JI-45 |

```
JI-40 ─► JI-41 ─► JI-42 ─► JI-43 ─► JI-44 ─► JI-45 ─► JI-46 ─► JI-47 ─► JI-50
```
