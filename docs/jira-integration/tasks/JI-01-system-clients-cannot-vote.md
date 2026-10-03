# JI-01: System clients cannot approve or reject approval requests

| Field | Value |
|---|---|
| Type | Hardening |
| Status | Done in e1bebfe |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — |
| Behavior change | A system-client token gets 403 on `POST /approval-requests/{id}/approve` and `/reject`. System clients are no longer eligible approvers (user decision 2026-10-03). Users are not affected. `cancel` is unchanged. |
| Design | [design.md §3.1](../design.md#31-system-clients-never-vote-ji-01) |

## Goal

Decision J2: approvals stay with humans in FluxGate. A Jira automation holds a system-client token with `flag:write`, and `flag:write` allows POST on any `approval-requests` path. Make it impossible for any system client to vote, whatever roles or scopes it has, so the audit trail always shows a human approver.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Scope to route mapping | `middleware/jwt_guard.rs`: `system_client_scope_allowed`. `flag:write` allows writes on paths with an `approval-requests` segment. |
| System-client JWT | `middleware/jwt_guard.rs`: `create_system_client_jwt_token` (roles `Requester`, `Approver`; `is_admin: false`). Check how `JwtGuard` marks the request as a system client (claims `token_type`, `JwtUser.team_id`, or a `PolicyActor` with `ActorKind::SystemClient`). |
| `JwtUser` | `lib.rs`: `struct JwtUser { id, username, is_admin, roles, team_id, token_hash }` |
| Vote handlers | `rest/approval.rs`: `approve_request` (`#[post("/approval-requests/{id}/approve")]`), `reject_request` |
| Vote rules | `logic/approval.rs`: `ensure_user_can_vote` (self-approval check, admin override, DB `Approver` role via `role_repository.user_has_role`, `is_eligible_voter`) |
| System clients table | `database/system_client.rs`; the shadow `users` row shares the system client's id |

Today a system client probably fails the vote anyway, because `ensure_user_can_vote` reads roles from `user_roles` and the shadow user should have none. Confirm this (look at system-client creation in `database/system_client.rs` / `logic/system_client.rs`) and record the result in the handoff log. The task adds an explicit rule either way.

## Changes

1. **REST layer.** In `approve_request` and `reject_request`, before calling the logic: when the caller is a system client, return `RestError::forbidden("system_client_vote_not_permitted")`. Use the same system-client detection `JwtGuard` uses; add a small helper (for example `fn is_system_client(req: &HttpRequest) -> bool` or a `JwtUser` method) if none exists. Do not add a database query when the JWT already says it.
2. **Logic layer (defense in depth).** At the top of `ensure_user_can_vote`, return a new `Error::SystemClientVoteNotPermitted` (or reuse an existing forbidden variant) when `approver_id` is a system client. Read it with one query (`SELECT EXISTS(SELECT 1 FROM system_clients WHERE id = $1)`) through the approval or system-client repository. Map the error to HTTP 403 in the `RestError` conversion.
3. Do not change `system_client_scope_allowed`: `cancel` must stay reachable with `flag:write`.
4. Update the OpenAPI `responses` of both handlers with the 403 case if not listed.

## Tests (write first)

- `rest/approval.rs` unit tests: a request carrying a system-client `JwtUser` / claims gets 403 with `system_client_vote_not_permitted` on approve and on reject; the logic mock is not called (`expect_approve_request().times(0)`).
- `logic/approval.rs` unit test: `ensure_user_can_vote` returns the new error for a system-client id, before the self-approval and role checks.
- A user with the `Approver` role can still vote (existing tests must pass unchanged).
- `api-tests`: optional here; JI-30 covers it end to end.

## Done when

- Both layers deny system-client votes; users vote as before.
- `cargo fmt`, `cargo clippy --all-targets`, `cargo test -p feature-toggle-backend` pass (seeded test DB).
- Contracts: if the OpenAPI responses changed, export and update the baseline.
- Handoff log entry written, `HANDOFF.md` and README table updated.

## Handoff log

### 2026-10-03: done in `e1bebfe`

**Finding.** The task assumed the shadow user has no roles. It has: `create_system_client` (`database/system_client.rs`) inserts `user_roles` rows for `Approver` and `Requester`. Commit `7c54500` also made active system clients **eligible approvers** of their team in `approver_qualifies_sql`. So a system-client token could vote whenever a policy routed by role. Blocking votes alone would have left bots on eligible lists and in the AI-11 "can still vote" cap. The user decided (2026-10-03): remove system clients from eligibility too, and keep the role rows.

**What changed**

- `middleware/jwt_guard.rs`: `is_approval_vote_route` (POST `/api/v1/approval-requests/{any}/approve|reject`, on the routed path) is checked in the system-client branch **before** the scope check. Answer: 403 `{"error":"forbidden","code":"system_client_vote_not_permitted"}`. Not audited (scope violations are not audited either). This is the REST-layer check; the handlers in `rest/approval.rs` are unchanged.
- `logic/approval.rs`: `ensure_user_can_vote` first calls `ApprovalRepository::is_system_client(approver_id)` and returns `Error::SystemClientVoteNotPermitted`, before the self-approval, admin override and role checks.
- `database/approval.rs`: new `is_system_client` (runtime query on `system_clients`). `approver_qualifies_sql` drops the system-client team branch and adds `AND NOT EXISTS (SELECT 1 FROM system_clients sc WHERE sc.id = u.id)`. Routing, `count_reachable_approvals` and `is_eligible_voter` all use it.
- `lib.rs`: `Error::SystemClientVoteNotPermitted`. `rest/error.rs`: constants `SYSTEM_CLIENT_VOTE_CODE` / `SYSTEM_CLIENT_VOTE_MESSAGE`, `RestError::system_client_vote_not_permitted()` (a `Forbidden` with that code), and the `From<crate::Error>` arm.
- Tests: guard route matcher and an end-to-end guard test (approve, reject, and percent-encoded `%61pprove`); logic test (admin override on, bot named on the policy: still denied, no vote, no role lookups); error mapping test; `tests/database/system_client_approval_test.rs` rewritten to "a system client is never an eligible approver; a human approver of the team still counts"; five existing mock tests got `expect_is_system_client().returning(|_| Ok(false))`.
- `api-tests/src/tests/system-client.test.ts`: the system client's approve **and** reject now expect 403 `system_client_vote_not_permitted` (was `self_approval_not_allowed`).

**Behavior changes**

- A system client gets 403 on approve and reject.
- A stage change request created by a system client, in a team whose policy routes by role, now needs a human approver in the team. Without one, the request fails the "no eligible approvers" check, as for any user.
- No contract change (no DTO or OpenAPI change); `check-contract-compat.sh` passes.

**Verified**

- `cargo fmt`; `cargo clippy --all-targets`: no warnings in the changed files.
- `cargo test -p feature-toggle-backend` on `feture_toggle_test`: all pass (753 lib, 285 integration, 0 failed).
- `./scripts/check-contract-compat.sh`: pass.
- Docker is not installed on this machine, so `test:docker` could not run. Instead: backend built and run on `127.0.0.1:18180` against `feture_toggle_test` (`env -u TYPESAFE_API_KEY`), then `API_BASE_URL=http://127.0.0.1:18180/api/v1 pnpm --dir api-tests exec jest --runInBand system-client approval`: 4 suites, 62 tests, all pass.
- The new tests were written before the implementation, but the red run was a compile failure, not a failing assertion. Ten existing mock tests then failed until their mocks were updated.

**For later tasks**

- JI-30's end-to-end test should assert `code == "system_client_vote_not_permitted"` (the `error` field is `forbidden`).
- `cancel` is still allowed for system clients with `flag:write`, so Jira can withdraw a request it made.
