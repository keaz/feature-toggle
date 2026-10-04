# Single sign-on (OIDC)

FluxGate supports OpenID Connect login through the Authorization Code flow with PKCE (S256). You can register several identity providers. Each provider can create users on first login, link to existing users, and keep roles, teams and the admin flag in sync with IdP groups.

Out of scope: SAML, SCIM and RP-initiated (single) logout. Signing out of FluxGate does not sign the user out of the IdP.

## How a login works

1. The browser opens `GET /api/v1/auth/sso/{slug}/authorize?redirect=<path>`. The backend creates a login state (state, nonce and PKCE verifier, valid 10 minutes, single use) and redirects to the IdP. It also sets the `fluxgate_sso_state` cookie.
2. The IdP redirects the browser to `GET /api/v1/auth/sso/{slug}/callback?code=&state=`. The backend checks the state cookie, consumes the state, exchanges the code for tokens on the back channel, validates the id_token, finds or creates the user and syncs roles.
3. The backend redirects to `<ui>/auth/sso/complete?code=<one-time code>[&redirect=<path>]`. On failure it redirects to `<ui>/login?ssoError=<code>`. `<ui>` is `allowed_origin`.
4. The UI posts the one-time code to `POST /api/v1/auth/sso/exchange` and receives the same body as `POST /api/v1/auth/login` (`isTemporary` is always `false`).

Design points:

- The IdP tokens never reach the browser. The one-time code is 32 random bytes, only its SHA-256 hash is stored, it is bound to the user and the provider, it is valid 60 seconds and single use. FluxGate session tokens are issued at exchange and are not stored in the one-time code.
- id_token checks: signature through the provider JWKS (RS256, ES256 or PS256 only; `none` and HS* are rejected), `iss` equal to the discovery issuer, `aud` contains the client ID (a multi-value `aud` needs `azp` equal to the client ID), `exp` and `iat` with 60 s leeway, and `nonce`. Discovery and JWKS are cached for 10 minutes. An unknown `kid` triggers at most one JWKS refetch per issuer per 60 seconds.
- Users are matched by `(provider, sub)` only.
- Discovery, JWKS, token and userinfo responses are limited to 1 MiB, with a 10 s timeout and no redirects. The provider connection test (`/test`) uses a 5 s timeout.
- If the same new user finishes two first logins in parallel (for example two tabs), one creates the user and the other fails with `sso_provider_error`. Retrying that login works and signs in the created user.

### State cookie

`/authorize` sets the cookie `fluxgate_sso_state`: HttpOnly, SameSite=Lax, `Path=/api/v1/auth/sso/`, 10 minutes. It carries the hash of the state, which binds the login to the browser that started it (login CSRF protection). The cookie is `Secure` when the browser-facing base URL is https. The callback clears it on success and on failure.

- A reverse proxy must forward cookies on `/api/v1/auth/sso/`. A proxy or CDN that strips them makes every login fail with `sso_state_invalid`.
- The cookie name is fixed. If a user starts SSO in two tabs, the later tab overwrites the cookie and the earlier tab fails with `sso_state_invalid`. The user starts again.
- Any request to the callback clears the cookie, also a forged cross-site callback GET. Such a request aborts a login that is in flight in that browser (the user starts again) but never issues a session.

## Command line login

The `fluxgate` CLI logs in through the same providers.

- `fluxgate login --sso <slug>` calls `GET /api/v1/auth/sso/{slug}/authorize?cli_redirect=http://127.0.0.1:<port>/callback&cli_challenge=<S256>`. `cli_redirect` must be exactly `http://127.0.0.1:<port>/callback` or `http://[::1]:<port>/callback`, and comes with a 43-character base64url PKCE challenge; anything else redirects to `<ui>/login?ssoError=sso_invalid_cli_redirect`. The state cookie works as for the UI. Once the state is valid, the callback sends `?code=<one-time>` (or `?error=<code>`) to the loopback address instead of the UI. The code is bound to the challenge: `POST /api/v1/auth/sso/exchange` needs `{"code", "codeVerifier"}` for it, and a wrong or missing verifier burns the code. Nothing extra is registered at the IdP: the IdP still redirects to the backend callback.
- `fluxgate login --use-device-code` (or `--no-browser` with an SSO session) works without a local browser. `POST /api/v1/auth/device/authorize` returns a device code, a user code like `BCDF-GHJK` and `<allowed_origin>/device?code=...`. The person opens that page, signs in by any method and approves (`POST /api/v1/auth/device/approve`, a signed-in user, never a system client). The CLI polls `POST /api/v1/auth/device/token`, which answers `authorization_pending`, `slow_down`, `access_denied` or `expired_token` until one session is released. Codes live 10 minutes and are stored hashed; both public device routes are rate limited per backend instance. Approvals and denials are in the activity log (`cli_login_approved`, `cli_login_denied`).

## Configuration

### Backend settings

| Setting | Purpose |
| --- | --- |
| `allowed_origin` (`config.toml`) | The UI origin. Callbacks redirect to `<allowed_origin>/auth/sso/complete` and `/login`. |
| `public_base_url` (`config.toml`, optional) | Public URL of the backend as the browser and the IdP reach it, for example `https://fluxgate.example.com`. |
| `FLUXGATE_ENCRYPTION_KEY` (env) | Key that encrypts stored client secrets. |
| `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET` (env, optional) | Client secret override per provider. |

**`public_base_url`.** Set it in production. The redirect URI sent to the IdP is `<public_base_url>/api/v1/auth/sso/<slug>/callback`. When it is unset, the backend logs a warning at startup and derives the URL from each request (scheme and host, honouring `Forwarded` and `X-Forwarded-*` headers). Those headers then decide both the `redirect_uri` and whether the state cookie is `Secure`, so the proxy in front of the backend must set them and strip any values sent by clients. If a proxy changes the host between `/authorize` and the callback, the IdP rejects the flow. A trailing slash is removed.

**`FLUXGATE_ENCRYPTION_KEY`.** It must be the standard base64 encoding of exactly 32 bytes (not base64url, no other length). Generate it with:

```sh
openssl rand -base64 32
```

Client secrets are encrypted with AES-256-GCM, with a random 96-bit nonce per value, and stored as `base64(nonce || ciphertext)`. The provider ID is bound to the ciphertext as associated data, so a secret copied between provider rows does not decrypt. Keep the key stable: if you change it, stored secrets stop decrypting and you must save them again. If the variable is unset or invalid, saving a non-empty `clientSecret` returns `400 {"error":"encryption_key_missing"}`. An invalid key also logs a startup warning.

**`FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`.** Take the provider slug, uppercase it and replace every character that is not a letter or digit (for example `-`) with `_`. The slug `my-idp` gives `FLUXGATE_SSO_MY_IDP_CLIENT_SECRET`. When this variable is set and not empty, it wins over the stored secret and the provider reports `clientSecretFromEnv: true`. This needs no encryption key, which suits secret managers and Kubernetes secrets.

### Managing providers

Admins manage providers in the admin UI under **Settings → Single Sign-On** (`/settings/sso`) or with the API under `/api/v1/sso/*`. All of these routes are system-admin only. The OpenAPI document lists them under the `SSO` tag.

| Field | Default | Notes |
| --- | --- | --- |
| `slug` | none | `^[a-z0-9][a-z0-9-]{0,48}$`, unique. Part of the redirect URI. |
| `displayName` | none | Shown on the login button. |
| `issuerUrl` | none | The OIDC issuer. FluxGate fetches `<issuer>/.well-known/openid-configuration`. |
| `clientId`, `clientSecret` | none | `clientSecret` is write-only. Omit it on PATCH to keep it, send `""` to clear it. |
| `scopes` | `openid email profile` | Must include `openid`. |
| `groupsClaim` | `groups` | Claim that carries groups or roles. Dot paths such as `realm_access.roles` work. |
| `allowedEmailDomains` | empty (any) | Case-insensitive. |
| `jitProvisioning` | `true` | Create users on first login. |
| `allowEmailLinking` | `false` | Link to an existing user by email. |
| `roleSyncMode` | `authoritative` | `authoritative`, `additive` or `off`. |
| `enabled` | `false` | Only enabled providers appear on the login page and accept logins. |

`POST /api/v1/sso/providers/{id}/test` fetches the discovery document and the JWKS (5 s timeout) and returns `{"ok", "issuer", "authorizationEndpoint", "error"}`. Use it before you enable a provider. The test is admin-only and requests the issuer URL from the backend host, so it can reach internal hosts. We accept this server-side request forgery risk because only administrators, who are trusted, can call it. Deleting a provider deletes its identities and mappings. Users remain.

Changing `issuerUrl` deletes the provider's identities in the same transaction, because a `sub` is only unique per issuer. Users keep their accounts. On their next login they are linked again (email linking) or created (JIT), subject to the usual rules. The `sso_provider_updated` activity records `issuer_changed` and `identities_cleared`.

Client authentication is `client_secret_basic`. FluxGate uses `client_secret_post` only when the provider does not advertise basic.

### Redirect URI to register at the IdP

```
<public_base_url>/api/v1/auth/sso/<slug>/callback
```

For example `https://fluxgate.example.com/api/v1/auth/sso/okta/callback`. The match is exact.

## Admin UI walkthrough

This walkthrough sets up Keycloak in the admin UI. The steps are the same for other IdPs. Setup recipes at the end of this page list the IdP-side settings.

### Prerequisites

- Sign in as a system admin. Other users see "Access Denied".
- Set `FLUXGATE_ENCRYPTION_KEY` and restart the backend. The key must be standard base64 of 32 bytes: `openssl rand -base64 32`. Alternatively, provide the secret per provider with `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`. A missing or invalid key shows this message when you save a secret: `Server has no FLUXGATE_ENCRYPTION_KEY; set it or provide the secret via FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`.
- Set `public_base_url` (see "Backend settings"). The redirect URI shown in the form depends on it.
- Create the roles and teams you want to map under **Settings → Roles** and **Settings → Teams**.

### Step 1: Add a provider

1. Open **Settings → Single Sign-On** (`/settings/sso`) and click **Add provider**.
2. Fill in the form:

| Field | Keycloak example |
| --- | --- |
| Display name | `Keycloak` (shown on the login button) |
| Slug | `keycloak` (lowercase letters, digits and dashes) |
| Issuer URL | `https://keycloak.example.com/realms/acme` |
| Client ID | `fluxgate` |
| Client secret | The secret from the Keycloak Credentials tab |
| Scopes | Keep `openid`, `email`, `profile` |
| Groups claim | `groups` |

3. Copy the **Redirect URI** with the **Copy** button. Register it at the IdP exactly as shown (see "Redirect URI to register at the IdP").
4. Leave **Enabled** off.
5. Click **Create provider**.

A badge next to **Client secret** shows its state: `Set`, `Not set`, or `Provided by environment variable (read-only)`. When you edit a provider, leave the field empty to keep the secret, or click **Clear secret** to remove it. Other fields (**Allowed email domains**, **Create accounts on first sign-in**, **Link to existing accounts by email**) are described in "Managing providers" and "Who may sign in".

### Step 2: Test the connection

1. In the provider list, click the **Edit provider** (pencil) button.
2. Click **Test connection**.
3. Success shows `Connection OK`, with the issuer and authorization endpoint. Failure shows `Connection failed` and an `Error:` line.

The test uses the saved configuration, not unsaved edits. The button is not shown while you add a provider, so save first.

### Step 3: Map groups

1. In the provider list, click the **Group mappings** button.
2. Click **Add mapping** for each row. Enter the group value, choose a **Target type** (`Role`, `Team` or `Admin`), then choose the target.
3. Click **Save mappings**.

Example:

| Group value | Target type | Target |
| --- | --- | --- |
| `fluxgate-devs` | Role | Requester |
| `fluxgate-devs` | Team | Payments |
| `fluxgate-approvers` | Role | Approver |
| `fluxgate-admins` | Admin | System administrator |

- One group can map to several targets. Use one row per target.
- The group value must match the IdP value exactly, including case. See "Group mapping and role sync".
- Click the **Remove mapping** (trash) button to delete a row.
- Saving replaces all mappings of the provider.
- The editor shows these messages: `Group value is required.`, `Select a role.` or `Select a team.`, `Selected role no longer exists. Choose another.` (same for team), and `Duplicate mapping.`

### Step 4: Choose the role sync mode

Set **Role sync mode** in the provider form. See "Group mapping and role sync" for the full rules.

- `Authoritative (IdP groups are the source of truth)`: use it when the IdP owns access. Sync also removes SSO-managed roles and teams. This is the default.
- `Additive (only add roles and teams)`: use it when admins also manage access in FluxGate. Sync never removes anything.
- `Off (do not sync roles)`: use it when SSO only authenticates users. Mappings are ignored.

### Step 5: Enable and sign in

1. Open the provider with **Edit provider**, turn **Enabled** on, and click **Save changes**. The list shows an `Enabled` badge.
2. Open `/login`. The page shows a **Sign in with Keycloak** button above an `or` divider. The pattern is `Sign in with <Display name>`. The button is hidden when no provider is enabled.
3. Click the button. The user signs in at the IdP and returns to FluxGate signed in.

If a login fails, the user returns to `/login` with an error message above the form, for example `Your email domain is not allowed for this sign-in method.` See "Who may sign in" for the error codes.

### Step 6: Verify

- **Settings → Users** shows an `SSO` badge on users who sign in with SSO.
- Open the user (**Edit User**). The **Basic Details** tab lists **Linked identities**: provider slug, email and last login.
- On the **Assign Teams** and **Assign Roles** tabs, roles and teams granted by sync are checked, disabled, and marked with an `SSO` chip. The chip hint reads `Managed by single sign-on`.
- If you change a locked assignment in another way (for example through the API), the request fails with `409 sso_managed` and the UI shows `This assignment is managed by single sign-on.` Remove the group at the IdP or remove the mapping. See the "Manual wins" rule.
- On the user edit page, the **Admin** checkbox of a user whose admin flag came from an SSO `admin` mapping (`adminSource: "sso"`) is checked, locked, and marked with an `SSO` chip. The chip hint reads `Managed by single sign-on`. To change the flag, remove the user from the admin group at the IdP (or remove the admin mapping) and let the next SSO login revoke it. Then grant admin manually if needed.
- A login that changes roles or teams writes a `sso_role_sync` activity.

### Step 7 (optional): Enforce SSO

Turn on **Enforce single sign-on** at the top of the **Single sign-on** page. The page warns: `Only break-glass admins (made admin manually, not through single sign-on) can still sign in with a password.` Everyone else sees `Password sign-in is disabled. Use single sign-on.`

The toggle fails with `409 enforce_sso_requires_local_admin` when no enabled break-glass admin with a password exists. Keep at least one such admin with a strong password. See "Enforce SSO and break-glass admin". Test SSO login before you turn this on.

### Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Save fails with `Server has no FLUXGATE_ENCRYPTION_KEY...` | The key is missing or invalid (not 32 bytes of standard base64). | Set a valid key and restart the backend, or use `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`. |
| The IdP shows `redirect_uri` mismatch | The registered URI differs from the one FluxGate sends. `public_base_url` is unset or wrong. | Set `public_base_url`. Register the exact URI from the form. |
| No login button | The provider is disabled, or the list failed to load. | Turn **Enabled** on. |
| `Your email domain is not allowed for this sign-in method.` (`sso_email_domain_not_allowed`) | The domain is not in **Allowed email domains**, or the IdP does not send `email_verified: true`. | Add the domain, mark emails verified at the IdP, or empty the list. |
| Roles are not synced | Wrong **Groups claim**; Keycloak **Full group path** is on, so groups arrive as `/fluxgate-devs`; sync mode is `Off`; or the mapping value differs in case. | Fix the claim name. Turn Full group path off or map `/fluxgate-devs`. Change the sync mode. Match the value exactly. |
| `Sign-in session expired. Try again.` (`sso_state_invalid`) | A proxy strips the `fluxgate_sso_state` cookie, a second tab overwrote it, or the login took over 10 minutes. | Forward cookies on `/api/v1/auth/sso/`. Use one tab. Start again. |
| Roles show in the Keycloak access token but are not synced | FluxGate reads the ID token (userinfo as fallback), never the access token. Keycloak client roles go only into the access token by default. | Add a mapper that sets Add to ID token and Add to userinfo. See the Keycloak recipe. |
| `409 sso_managed` when you change a role or team | SSO sync owns the assignment. | Remove the group at the IdP or remove the mapping. |

## Who may sign in

Checks run in this order after the id_token is valid:

1. **Email.** The IdP must send an `email` claim (from the id_token or userinfo). Otherwise the login fails with `sso_email_missing`.
2. **Allowed domains.** If `allowedEmailDomains` is not empty, the email domain must be in the list and the IdP must send `email_verified: true`. An unverified or missing `email_verified` never satisfies a non-empty list (`sso_email_domain_not_allowed`). With an empty list, unverified emails are accepted.
3. **Identity.** A known `(provider, sub)` signs in the linked user.
4. **Linking.** For an unknown identity with an email that matches an existing user (case-insensitive), FluxGate links the identity only if `allowEmailLinking` is on and `email_verified` is `true`. It never links to a system admin account or to a system-client user. It also refuses to link a user who already has an identity at another provider when either provider has `roleSyncMode` other than `off` (see "One provider with role sync per user"). Otherwise the login fails with `sso_linking_not_allowed`.
5. **JIT.** Otherwise, if `jitProvisioning` is on, FluxGate creates a user without a password, with `authSource: "sso"`, not admin. If it is off, the login fails with `sso_user_not_provisioned`.

   With a public or multi-tenant issuer (for example Google, or Entra `https://login.microsoftonline.com/common/v2.0`), anyone with an account at that IdP can pass the id_token checks, so JIT lets any such account create a FluxGate user. With these issuers, set `allowedEmailDomains` or turn `jitProvisioning` off.
6. A disabled user fails with `sso_account_disabled`.

Error codes sent as `ssoError`: `sso_state_invalid`, `sso_provider_error`, `sso_token_invalid`, `sso_email_missing`, `sso_email_domain_not_allowed`, `sso_user_not_provisioned`, `sso_linking_not_allowed`, `sso_account_disabled`. Details go to the backend log without secrets, codes or tokens.

SSO users have no local password. A password login for them returns the normal 401, and admins cannot issue temporary passwords for them (`400 sso_user_no_local_password`).

## Group mapping and role sync

Mappings turn IdP group values into FluxGate assignments. Set them with `PUT /api/v1/sso/providers/{id}/mappings`, which replaces all mappings of the provider. Each mapping has a `groupValue` (exact, case-sensitive match) and a target:

- `role` with a role ID
- `team` with a team ID
- `admin` with no target: members become system admins

Where groups come from:

- The claim named `groupsClaim`. FluxGate first tries the whole name as a literal key (useful for URL-style claim names), then a dot path. A string is one group, an array contributes its string items, other types give no groups.
- If the id_token has no such claim and the sync mode is not `off`, FluxGate calls the userinfo endpoint and reads the claim there. The userinfo `sub` must equal the id_token `sub`.
- Microsoft Entra overage: when the id_token has `_claim_names.groups`, the user is in too many groups for the token. FluxGate treats this as no groups, does not call userinfo, and logs a warning. Use app roles instead (see the Entra recipe).

Sync modes (applied at every SSO login):

| Mode | Behaviour |
| --- | --- |
| `off` | No sync. |
| `additive` | Adds the roles and teams of matching mappings and grants admin. Never removes anything. |
| `authoritative` | Also removes SSO-managed roles and teams that the user no longer qualifies for, and revokes an SSO-granted admin flag. |

Rules:

- **Manual wins.** Sync only removes assignments that it created (source `sso`). Assigning a role or team manually that a user holds through SSO converts the row to manual, and sync no longer removes it. Manually removing an SSO-managed role returns `409 sso_managed`. Remove the group from the user in the IdP, or remove the mapping.
- **Admin.** Sync grants admin only to users who were not admins and marks the flag as SSO-managed. It revokes only an SSO-granted flag. Admins made manually, and admins that existed before SSO, are never revoked. If an admin saves a user who is already an SSO-granted admin in the UI with admin still on, the flag stays SSO-managed. The users API reports the origin as `adminSource` (`manual` or `sso`). To take ownership of an SSO-granted flag, remove the user from the admin group at the IdP (or remove the admin mapping) and let the next SSO login revoke it, then grant admin manually. Through the API, revoking admin and then granting it again also works.
- **Last admin.** Sync never removes the last enabled admin. It keeps the flag, logs a warning and writes an activity `sso_role_sync_warning`, at most once per 24 hours per user.
- **Deleted targets.** Deleting a role or team deletes its mappings. Mappings whose target no longer exists are ignored.
- **Audit.** A login that changes anything writes an activity `sso_role_sync` with the added and removed role and team IDs and the admin change. Other activities: `sso_user_provisioned`, `sso_identity_linked`, `sso_login`, `sso_provider_created|updated|deleted`, `sso_mappings_updated`, `sso_settings_updated`.
- The users API shows `authSource`, `adminSource`, `ssoManagedRoleIds`, `ssoManagedTeamIds` and `identities`. `adminSource` is `manual` or `sso` for admins and `null` for everyone else.
- If sync is `authoritative` and the groups cannot be read (for example userinfo fails), the user logs in with SSO-managed roles removed. This fails closed.
- **Stuck SSO rows.** Sync runs only at login through a provider with sync on. After a provider is deleted, or its sync is set to `off`, the SSO-managed roles and teams it granted stay and cannot be removed by hand (`409 sso_managed`). Workaround: assign the role or team manually (this converts the row to manual), then remove it.

### One provider with role sync per user

Role sync is per user, not per provider: two providers that sync the same user would revoke each other's roles at every login. Link a user to at most one provider with role sync on. FluxGate enforces this for email linking (`sso_linking_not_allowed`). Users linked to several providers before this check existed are not changed.

## Enforce SSO and break-glass admin

`PUT /api/v1/sso/settings` with `{"enforceSso": true}` blocks password login for everyone except break-glass admins: `POST /api/v1/auth/login` returns `403 {"error":"sso_required"}` after the password check.

- **Break-glass admins** are system admins whose admin flag was not granted by SSO group sync (made manually, or admins that existed before SSO). Only they can still use a password. This is the way in when the IdP is down or misconfigured.
- An admin granted by an SSO `admin` mapping must sign in through SSO, even if the account has a local password (for example a local user linked by email). Otherwise the password would bypass the IdP and an authoritative revoke.
- `PUT /api/v1/sso/settings` with `{"enforceSso": true}` returns `409 {"error":"enforce_sso_requires_local_admin"}` unless at least one enabled break-glass admin with a password exists (system-client shadow users do not count). Keep at least one such admin with a strong password.
- Enforcing SSO does not end existing sessions, and neither does deprovisioning a user at the IdP: FluxGate sessions stay valid until they expire. To cut a user off at once, disable the user in FluxGate, which revokes their tokens.

## Security notes

- Never log or return secrets: client secrets, codes, tokens, PKCE verifiers and raw id_tokens stay out of logs, activity metadata and responses.
- Login states and one-time codes are single use. A background job deletes expired ones.
- Post-login `redirect` is accepted only if it starts with `/` and not `//` or `/\`.
- Run behind HTTPS in production, set `public_base_url` to the https URL, and forward cookies on `/api/v1/auth/sso/`.
- The backend and the IdP must reach each other over the network: the backend calls the IdP for discovery, JWKS, token and userinfo.

## Setup recipes

In each recipe, replace `<slug>` with the slug you choose and register `<public_base_url>/api/v1/auth/sso/<slug>/callback`. Test with `POST /api/v1/sso/providers/{id}/test`, then set `enabled` to `true`. The Keycloak recipe was tested against a Keycloak server (login, linking by verified email, group sync with the added and removed groups). The Okta and Entra recipes follow the vendor documentation and were not run against live tenants.

### Keycloak

1. Create or pick a realm.
2. Create a client: client type OpenID Connect, client authentication **on** (confidential), standard flow on. Valid redirect URI: `<public_base_url>/api/v1/auth/sso/<slug>/callback`.
3. Copy the client secret from the Credentials tab.
4. Groups, roles or client roles. FluxGate reads claims from the ID token and falls back to userinfo. It never reads the access token. Pick one option below.
   - Groups: add a client scope mapper of type **Group Membership**, token claim name `groups`, **Full group path off**, add to ID token and userinfo. Group names then arrive as `fluxgate-devs`, not `/fluxgate-devs`. Or use realm roles: set `groupsClaim` to `realm_access.roles` and make sure the realm roles mapper has Add to ID token or Add to userinfo on (FluxGate reads userinfo when the id_token lacks the claim).
   - Client roles (third option): go to Clients → `<client>` → Client scopes → `<client>-dedicated` → Add mapper → By configuration → **User Client Role**. Set Client ID to the client, Token claim name to `groups`, **Multivalued** on, **Add to ID token** on and **Add to userinfo** on. Then set `groupsClaim` to `groups`.
   - Client roles (alternative): in the built-in `roles` client scope, open the `client roles` mapper and turn **Add to ID token** (or **Add to userinfo**) on. Then set `groupsClaim` to `resource_access.<client>.roles`.
   - Keycloak puts client roles only in the access token by default, so a client role mapping does nothing until one of these mappers adds the roles to the ID token or userinfo.
   - Example mappings: client role `team_admin` → Role "Team Admin"; client role `perftest` → Team.
5. Provider: `issuerUrl` = `https://<host>/realms/<realm>`, `groupsClaim` = `groups` (or `realm_access.roles`, or `resource_access.<client>.roles`).
6. Make sure users have a verified email if you use domain restriction or linking.

### Okta

1. Applications, create an **OIDC - Web Application** with the Authorization Code grant. Sign-in redirect URI: the callback URL.
2. Use a **custom authorization server** (Security, API). The org authorization server does not support custom claims. Issuer: `https://<org>.okta.com/oauth2/<server-id>`.
3. On that server add a **groups claim**: name `groups`, include in the ID token (Always), value type Groups, filter for example Starts with `fluxgate-` or Matches regex. Keep the filter narrow so the claim stays small.
4. Assign the app to the people or groups who may sign in.
5. Provider: `issuerUrl` = the custom server issuer, `groupsClaim` = `groups`.

### Microsoft Entra ID

1. App registrations, new registration, platform **Web**, redirect URI: the callback URL. Create a client secret under Certificates and secrets.
2. Issuer: use the v2.0 endpoint, `https://login.microsoftonline.com/<tenant-id>/v2.0`. Set the app manifest `requestedAccessTokenVersion` to 2 if the issuer in tokens is the v1 form.
3. Recommended: **app roles** and the `roles` claim. Define app roles in the app registration, assign users or groups to them in Enterprise applications, and set `groupsClaim` to `roles`. Role values are the group values in mappings.
4. Alternative: **group object IDs** through Token configuration, add groups claim, and use the group object ID as `groupValue`. A user in many groups (more than 200 for JWTs) triggers overage: Entra sends `_claim_names` instead of groups, FluxGate treats this as no groups and does not call Graph. With `authoritative` mode this removes SSO-managed roles of that user. Prefer app roles, or assign groups to the application and select "Groups assigned to the application".
5. Entra does not send `email_verified`. With a non-empty `allowedEmailDomains`, or with `allowEmailLinking`, FluxGate therefore rejects these logins. Leave the domain list empty and restrict access through user assignment on the enterprise application.
