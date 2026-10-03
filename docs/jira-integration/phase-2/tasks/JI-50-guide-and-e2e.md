# JI-50: Setup guide update and end-to-end write-back test

| Field | Value |
|---|---|
| Type | Docs + test |
| Status | Done in b75c9a9 (test), e3321e3 (guide) |
| Repo | backend repo (`feature-toggle/`): `docs/` and `api-tests/` |
| Depends on | JI-43, JI-44, JI-45 (JI-46 for the UI wording in the guide) |
| Behavior change | None |
| Design | [design.md §3.7, §4](../design.md#37-setup-guide-and-end-to-end-test-ji-50) |

## Goal

An admin can set up write-back, native webhooks and the rate limit from the guide alone. One api-test proves the whole loop against a running backend with a fake Jira.

## Current code (verify first)

| Piece | Where |
|---|---|
| Guide | `docs/jira-integration/setup-guide.md` (sections 1-8 from JI-30: FluxGate side, Jira side with Automation options A and B, field id, network, checking results, status tables, not supported, by-key) |
| Phase 1 e2e test | `api-tests/src/tests/advanced/jira-flow.test.ts`: helpers `webhookBody`, `sendJiraEvent`, `getStageStatus`, `resultFor`; setup of team, environments, policy, approver, feature with relationship qa to prod |
| Docker runner | `api-tests` `test:docker` script and its compose file (`grep -n test:docker api-tests/package.json`) |
| Backend config for tests | the config the api-test backend starts with (`FEATURE_TOGGLE_CONFIG` in the docker script) |
| Step 0 result of JI-45 | JI-45 handoff log (Data Center signing yes or no) |

## Changes

1. **Guide** (`setup-guide.md`):
   - New section **"Write-back: show results in Jira"**, placed after section 1:
     - Create a dedicated Jira user. Cloud: an API token at id.atlassian.com. Data Center: a personal access token.
     - Permissions it needs: Browse projects, Add comments, Link issues.
     - In FluxGate: set the base URL, edition, email, token and toggles, then Test connection.
     - What appears on the issue: example comments and the remote link title.
     - The paused state and Resume. The Outbound tab and Retry.
     - Comments are never sent for changes Jira made itself; the event comment covers them.
     - Security: the token is stored encrypted (`FLUXGATE_ENCRYPTION_KEY` is required) and is never shown again.
   - Section 2 gets **option C, "Native webhook"**: create the webhook, add a JQL filter, select the `jira:issue_updated` event, and paste the FluxGate native secret into Secret.
     - Mark it Cloud only, or Cloud and Data Center <version>, per the JI-45 Step 0 result.
     - Update the line that says native webhooks are not supported.
   - Network section:
     - FluxGate needs outbound HTTPS to the Jira site.
     - Built-in inbound limit (120/min, burst 60, `[jira] inbound_per_minute` / `inbound_burst`), with 429 and `Retry-After`. The proxy limit becomes optional.
     - `[jira] ui_base_url` for the remote link URL, and `allow_insecure_http` for http Jira in labs only.
   - Status tables: add 429. Add the outbound job statuses.
   - "Not supported": remove write-back. Keep transitions, Forge and the bridge.
2. **api-test** `api-tests/src/tests/advanced/jira-writeback.test.ts`:
   - **Fake Jira:** an `http.createServer` on port 0 in `beforeAll`. It records `{method, url, headers.authorization, body}` and answers 201 for comments, 200 or 201 for remote links, and 200 for `/myself`. A switch makes it answer 401 for the pause test.
   - **Fake Jira host:** `process.env.FAKE_JIRA_HOST ?? '127.0.0.1'`. The docker runner sets `host.docker.internal`, and adds `extra_hosts: ["host.docker.internal:host-gateway"]` to the backend service if it is not there yet.
   - **Backend config:** the backend under test needs `[jira] allow_insecure_http = true` and a `ui_base_url`. Add both to the api-test backend config only.
   - **Token:** generated with `crypto.randomUUID()` at runtime.
   - **Polling:** the capture and sender run every 5 s, so `waitFor(predicate, 30_000)` polls the fake Jira's request log every 500 ms.
   - **Setup:** copy the setup of `jira-flow.test.ts`, then:
     - set the base URL to the fake Jira;
     - `PUT .../writeback` with comments and remote link on;
     - link the feature to `PROJ-<random>`.
   - **Tests:**
     1. `test connection reaches the fake Jira with the token` (Basic auth header decodes to `email:token`).
     2. `a Jira approve event posts one comment with the outcome`: the comment body text contains `qa: approve applied`.
     3. `a human approval in FluxGate posts a comment and updates the remote link`: a prod request by Jira (request rule), the approver votes, then a comment containing `Approved for prod by` and a remote link POST whose title contains `prod DEPLOYMENT_APPROVED`.
     4. `a Jira-made deploy gives no second comment`: count comments for the issue before and after; +1 from the event, not +2.
     5. `a 401 from Jira pauses write-back`: the fake Jira switches to 401, then trigger a change. `GET integration` shows `writeback.pausedReason`, and `outbound-jobs?status=pending` still has the later jobs.
     6. `a signed native webhook is accepted and a bad signature is 401`: compute the HMAC with `crypto.createHmac('sha256', secret)`.
     7. `bursts over the limit get 429`: send 65 events quickly with a valid secret (`Promise.all`), then expect at least one 429 with `Retry-After`. Run this test **last** in the file: it fills the bucket for that integration.
   - **Cleanup** in `afterAll`: close the server and delete the integration.
3. **README and HANDOFF:**
   - Tick JI-50 in the phase 2 README.
   - `../HANDOFF.md`: phase 2 complete, plus what is left open (real Jira site not tested; DC signing per JI-45).

## Steps

- [ ] **Step 1:** write `jira-writeback.test.ts`. Run it against the local backend on `feture_toggle_test` (`pnpm --dir api-tests exec jest jira-writeback.test.ts`). Each test must pass; investigate any failure, because it is a backend bug or a test bug, never a reason to skip.
- [ ] **Step 2: mutation check.** Temporarily remove the Jira-actor filter in `plan_jobs`. Test 4 must fail. Restore the filter, and record this in the handoff log.
- [ ] **Step 3:** `pnpm --dir api-tests exec tsc --noEmit` is clean. If Docker is available, also run `pnpm --dir api-tests run test:docker`; otherwise say so in the handoff log.
- [ ] **Step 4:** update the guide. Then follow the guide by hand with curl against the fake Jira, or with a real Jira test site if the user provides one. Note which steps you ran.
- [ ] **Step 5:** commit `test(jira): end-to-end write-back test; docs: write-back and native webhook setup (JI-50)`, then commit the docs update (`docs(jira): ...`).

## Done when

- The guide covers write-back, native webhooks and the rate limit, matching the code.
- The 7 api-tests pass locally, and the mutation check fails as expected.
- Phase 2 is marked complete in `HANDOFF.md`.

## Handoff log

2026-10-04, backend `b75c9a9` (test and infrastructure), `e3321e3` (guide):
- Tests: `api-tests/src/tests/advanced/jira-writeback.test.ts`, 7 tests, all pass against the local backend on `feture_toggle_test` (two full runs, about 45 s each). Test 4 waits for the event comment and the remote link refresh, then 8 s more, and asserts exactly +1 comment. Tests 3 to 5 have a 60 s timeout (test 5 took up to 22 s because it also resumes and drains the queue). Test 5 goes beyond the brief: it resumes write-back and waits until no job is pending.
- Environment names in the test are exactly `qa` and `prod` (the comment text uses the environment name).
- Backend config for the test stack: new `api-tests/backend-config.toml` (`[jira] allow_insecure_http = true`, `ui_base_url`), mounted by `docker-compose.api-tests.yml` at `/app/config/config.toml`. The compose backend also gets `extra_hosts: host.docker.internal:host-gateway` and `FLUXGATE_ENCRYPTION_KEY`; `scripts/run-api-tests-docker.sh` generates the key per run (`openssl rand -base64 32`, exported, never stored) and sets `FAKE_JIRA_HOST` (default `host.docker.internal`).
- Local run: binary `feature-toggle-backend` (not `fluxgate`, which is the CLI), `FEATURE_TOGGLE_CONFIG` pointing at a scratch copy of the api-test config with `127.0.0.1:18080` and gRPC `127.0.0.1:50061`, a runtime key, test DB. The backend was stopped afterwards.
- Mutation check: removed `&& !jira_made` in `plan_jobs` (`jira_capture.rs`), rebuilt, restarted. Test 4 failed (`Expected: 4, Received: 5`); the other 6 passed. File restored with `git checkout`; `git diff` shows no backend change. A final full run with the restored code passed 7 of 7.
- `pnpm --dir api-tests exec tsc --noEmit` is clean. `test:docker` was not run: Docker is not installed here.
- Guide: new section "Write-back: show results in Jira" (after section 1), option C "Native webhook" (2.4, with Cloud confirmed and Data Center worded per JI-45), network section with the built-in rate limit (429, `Retry-After`, the keep-the-URL-private and per-IP proxy advice), `[jira]` key table, 429, native signature and outbound job status tables, "Not supported" updated.
- Manual steps from the guide: the guide's API calls for write-back (PUT writeback, test connection, resume, outbound jobs list), native secret generation and the signed webhook request, and the event and rate limit behavior were all exercised by the api-test against the fake Jira. The `openssl` signing snippet and the Jira UI steps (Automation, webhook form, Cloud token page) were not run: no Jira site was available.
- Deferred manual checks now covered by this run: JI-43 step 10 (fake Jira end to end), JI-44 step 9 (200 then 429), JI-45 step 9 (signed webhook and bad signature), JI-46 step 6 only for the API behind it (the UI itself was not clicked).
- Not covered: real Jira Cloud or Data Center; the UI paused banner in a browser.
