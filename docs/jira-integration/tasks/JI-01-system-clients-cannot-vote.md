# JI-01: System clients cannot approve or reject approval requests

| Field | Value |
|---|---|
| Type | Hardening |
| Status | Open |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — |
| Behavior change | A system-client token gets 403 on `POST /approval-requests/{id}/approve` and `/reject`. Users are not affected. `cancel` is unchanged. |
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

(empty)
