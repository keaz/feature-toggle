# Changelog

All notable user-visible changes to FluxGate are recorded here.

## Unreleased

### Added

- **Single sign-on with OpenID Connect (backend).** Admins can register OIDC identity providers (`/api/v1/sso/*`, system admin only) and users sign in through `GET /api/v1/auth/sso/{slug}/authorize`, with PKCE, a browser-bound state cookie (`fluxgate_sso_state`) and a one-time code exchange (`POST /api/v1/auth/sso/exchange`). Features: just-in-time user creation, linking by verified email (opt-in, never for admins), allowed email domains, group to role, team and admin mappings with `authoritative`, `additive` or `off` sync, and an enforce-SSO setting that makes `POST /api/v1/auth/login` return `403 sso_required` for everyone except break-glass admins (system admins not granted admin by SSO). Turning enforcement on returns `409 enforce_sso_requires_local_admin` unless an enabled break-glass admin with a password exists. Email linking is refused (`sso_linking_not_allowed`) when the user already has an identity at another provider and either provider syncs roles; link a user to at most one provider with role sync on. Changing a provider's `issuerUrl` deletes its user identities (users stay and are linked or created again at their next login). Enforcing SSO and deprovisioning at the IdP do not end existing sessions; disable the user in FluxGate for that. With a public or multi-tenant issuer (Google, Entra `/common`), JIT lets any account at that IdP create a user, so set allowed domains or turn JIT off. The admin UI page is Settings → Single Sign-On (`/settings/sso`). Client secrets are stored encrypted (AES-256-GCM) with `FLUXGATE_ENCRYPTION_KEY` (standard base64 of 32 bytes), or come from `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`. New optional `public_base_url` config key; set it in production (the backend warns at startup when it is unset). New migrations make `users.password_hash` nullable and add `users.auth_source`, `users.admin_source` and a `source` column on `user_roles` and `user_teams`; the users API gains `authSource`, `ssoManagedRoleIds`, `ssoManagedTeamIds` and `identities`. Removing an SSO-managed role by hand returns `409 sso_managed`. See `docs/sso.md`.
- **`adminSource` on the users API (backend).** `UserResponse` (list, get, update, and the `user` in the login and SSO exchange responses) has a new `adminSource` field: `manual` or `sso` for admins, `null` for non-admins. The UI uses it to lock the Admin checkbox with an `SSO` chip for SSO-granted admins. The OpenAPI contract hash in the baseline was updated.
- **SSO docs (docs).** `docs/sso.md` has an admin UI walkthrough, Keycloak client-role recipes (ID token and userinfo mappers, never the access token), a troubleshooting row for roles that appear only in the access token, and the new way to take ownership of an SSO-granted admin flag.
- **Edges drop flags renamed or removed while they were disconnected (backend + edge, B15).** The gRPC stream protocol has a new `FeatureUpdate.Action` value, `SNAPSHOT_COMPLETE = 5`. The backend sends it once after the initial snapshot of each `StreamUpdates` stream, and not when the snapshot fails. On that marker the edge removes cached features of its team that the snapshot did not contain and drops their cached and pending assignments, so evaluating such a key falls back to a backend fetch and then to not-found. Before, these entries kept evaluating until LRU eviction. The change is additive and backward compatible: older edges ignore the new action, and new edges connected to an older backend never sweep. The proto contract hashes changed, so the contract baseline was updated.
- **Configurable edge client-info cache size (edge).** New `[cache] client_max_capacity` setting (env `EDGE_CACHE__CLIENT_MAX_CAPACITY`) sets how many client credentials' info the edge caches. The default stays 1000, so existing deployments are unchanged.

### Changed

- **Dependencies on non-boolean flags are rejected (backend).** A dependency passes only when it evaluates to `true`, so a dependency on a contextual flag with string, number or object variant values always blocked its dependents. Creating or updating a feature to add such a dependency, changing a depended-on flag to non-boolean values (or from `SIMPLE` to `CONTEXTUAL` with non-boolean stored variants), and version rollbacks that would do either now return `400 invalid_input`. Evaluation results do not change. Existing configurations are not migrated and stay editable; their non-boolean dependencies keep blocking until removed.

- **Dependencies are now bucketed with their own key (evaluation engine).** When flag `F` depends on flag `D`, the engine used to evaluate `D` inside `F` with `F`'s key in the bucketing hash (`SHA256("F:" + targetingKey)`). It now uses `D`'s own key (`SHA256("D:" + targetingKey)`), so the `D` check inside `F` gives the same result as evaluating `D` directly for the same user. `F`'s own weighted split is no longer correlated with `D`'s split, so all of `F`'s variants are served again.

  **Who is affected:** only flags that depend on a flag whose matching criterion is a weighted split with variants of different truthiness (for example a percentage rollout `{off: false, on: true}`). Flags evaluated directly, flags without dependencies, and dependencies without weighted splits keep exactly the same results.

  **How many users change:** let `q` be the share of `D`'s bucket range that is truthy. Among users whose evaluation of `F` reaches `D`'s weighted check, `2q(1−q)` see a different `D` result: half of them (`q(1−q)`) lose `F` and half gain it. For example, a 50% rollout flips 50% of those users and a 10% rollout flips 18%.

  **When it takes effect:** backend REST and gRPC evaluations change immediately after the backend is upgraded. Edge servers change once they run the new engine; users who already have a cached truthy result for `F` on an edge keep it until `F` is re-synced (the next upsert or snapshot of `F`, in practice the next edge restart or reconnect).
- **Edges keep recorded sticky assignments when an enabled flag is updated (edge).** An Upsert or Snapshot of a flag dropped that flag's assignments that were queued but not yet sent to the backend (up to one flush interval, 10 s by default), including on every reconnect snapshot. They are now kept and sent while the flag stays enabled. Cached results are still cleared on every update, so users are evaluated against the new config exactly as before; only the backend's assignment records are more complete. A kill-switched flag still drops both.

- **OFREP requires an SDK key (edge, breaking).** `POST /ofrep/v1/evaluate/flags` and `POST /ofrep/v1/evaluate/flags/{key}` now authenticate with `Authorization: Bearer <clientId>.<apiKey>` or `X-API-Key: <clientId>.<apiKey>`. A bare client ID is no longer accepted and gets `401`. The admin UI shows the SDK key in this form. `/evaluate` is unchanged. `Web` clients get CORS headers for their allowed origins on all three endpoints; see `docs/edge-server-api.md`. Deploy the edge and switch the SDK configuration together.
- **Access tokens last 30 minutes and renew with rotating refresh tokens (backend).** `POST /api/v1/auth/login` returns a refresh token, and `POST /api/v1/auth/refresh` exchanges it for a new access token and refresh token. Lifetimes are set in the new `[auth]` config section (`access_token_ttl_minutes = 30`, `refresh_token_ttl_days = 7`). A refresh token that is reused after a 10 s grace window revokes its whole family.
- **System-client management is restricted (backend).** Only admins and Team Admins of the client's team can manage system clients. System clients no longer carry `is_admin`: an M2M approval that relied on the admin override now gets `403`. Existing system-client tokens keep working.
- **Approval requests cannot be approved by their requester (backend).** A request whose requester is the only eligible approver can no longer reach quorum. Cancel such pending requests.
- **Disabling or demoting the last enabled admin is rejected (backend).** `PATCH /api/v1/users/{id}` returns `409 last_admin_required` when the update would leave no enabled admin. System-client shadow users never count as admins.

### Removed

- **Edge no longer loads persisted sticky assignments at startup (edge).** On startup the edge called `ListUserAssignments` and filled its assignment cache, but the first stream snapshot cleared every flag's cached assignments right after, so the load cost one backend call per start and never changed a served result. The call and its cache-filling code are gone. Served results, the assignment flush and the backend RPC are unchanged.

### Security

- **Backend container no longer logs the database password.** `scripts/backend-entrypoint.sh` printed the full `DATABASE_URL` and an "Extracted password" line at startup, so the PostgreSQL password ended up in container logs. It now prints the URL with the credentials replaced by `***`. Rotate the database password if these logs were shipped anywhere others can read.

- **Authentication hardening (backend, edge).**
  - Disabled users cannot log in or refresh, and disabling a user revokes all their access and refresh tokens in the same transaction.
  - JWTs carry `iss`, `aud` and `kid` claims and are validated against them. After a signing-secret rotation, tokens signed by the previous secret stay valid for `access_token_ttl_minutes`.
  - Route policies are matched on the router-decoded path, and system-client shadow users are never admins.
  - Scheduled changes are authorized against their creator when they run. Variant allocation changes are transactional.
  - Approvers cannot approve their own requests.
  - The edge authenticates OFREP requests with SDK keys and caches auth failures.
- **Known follow-up:** open WebSocket streams are authenticated only at upgrade, so they outlive a later disable, token revocation or token expiry until the client reconnects.

### Fixed

- **Removing a user's last manual role works (backend).** `POST /api/v1/users/{id}/roles` with an empty `roleIds` list now removes all manual role assignments (SSO-managed ones stay). Before, an empty list was ignored.
- **Live updates no longer send variants for Simple flags (backend).** Live `FeatureUpdate` upserts from REST feature and criteria changes and from approvals included the stored variants of `SIMPLE` features, and edge servers applied them. The stream snapshot, `GetFeatureByKey` and REST evaluation omit variants for `SIMPLE` features. So a Simple flag with non-boolean stored variants could act non-boolean on an edge after a live update until the next snapshot. Live updates now use the snapshot mapping: only `CONTEXTUAL` features carry variants.

### Upgrade notes

Read these before deploying.

- **All user sessions end at deploy.** Tokens issued by the old version lack `iss` and `aud`, so users log in again. Access tokens are now 30 minutes and renew with rotating refresh tokens (`[auth]` config).
- **System-client (M2M) tokens:** existing tokens keep working. A token signed by a rotated secret stops working `access_token_ttl_minutes` after the rotation, and a legacy token without `kid` stops at the first rotation. Re-issue M2M tokens after every rotation.
- **Pending approval requests** where the requester is the only eligible approver can no longer reach quorum, and self-approval is blocked. Cancel those requests.
- **Pending scheduled ENABLE, DISABLE and ARCHIVE changes** created by system clients, or by Team Admins of another team, are BLOCKED when they run (`creator_not_authorized`). Recreate them with an authorized creator.
- **Edge OFREP breaking change:** SDK key `<clientId>.<apiKey>` is required, and a bare client ID gets `401`. Deploy the edge and switch the SDK configuration together. `/evaluate` is unchanged.
- **Deploy order:** deploy the backend first with the `Recreate` strategy, not a rolling update, because old and new pods reject each other's tokens. Deploy the admin UI right after. Deploy the edge last, together with the SDK configuration change.
- **Before deploy**, check that an enabled human admin exists. The result must be at least 1:
  ```sql
  SELECT count(*) FROM users WHERE is_admin AND enabled AND id NOT IN (SELECT id FROM system_clients);
  ```
- **After deploy**, remove admin rights from system-client shadow users again:
  ```sql
  UPDATE users SET is_admin = FALSE WHERE id IN (SELECT id FROM system_clients);
  ```
- **Runbook after `deactivate-all`:** restart all backend pods, which creates a new signing secret, and re-issue every system-client token. Remove any stale `jwt_secret =` line from deployed configs.
