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
- Discovery, JWKS, token and userinfo responses are limited to 1 MiB, with a 10 s timeout and no redirects.

### State cookie

`/authorize` sets the cookie `fluxgate_sso_state`: HttpOnly, SameSite=Lax, `Path=/api/v1/auth/sso/`, 10 minutes. It carries the hash of the state, which binds the login to the browser that started it (login CSRF protection). The cookie is `Secure` when the browser-facing base URL is https. The callback clears it on success and on failure.

- A reverse proxy must forward cookies on `/api/v1/auth/sso/`. A proxy or CDN that strips them makes every login fail with `sso_state_invalid`.
- The cookie name is fixed. If a user starts SSO in two tabs, the later tab overwrites the cookie and the earlier tab fails with `sso_state_invalid`. The user starts again.

## Configuration

### Backend settings

| Setting | Purpose |
| --- | --- |
| `allowed_origin` (`config.toml`) | The UI origin. Callbacks redirect to `<allowed_origin>/auth/sso/complete` and `/login`. |
| `public_base_url` (`config.toml`, optional) | Public URL of the backend as the browser and the IdP reach it, for example `https://fluxgate.example.com`. |
| `FLUXGATE_ENCRYPTION_KEY` (env) | Key that encrypts stored client secrets. |
| `FLUXGATE_SSO_<SLUG>_CLIENT_SECRET` (env, optional) | Client secret override per provider. |

**`public_base_url`.** Set it in production. The redirect URI sent to the IdP is `<public_base_url>/api/v1/auth/sso/<slug>/callback`. When it is unset, the backend logs a warning at startup and derives the URL from each request (scheme and host, honouring `Forwarded` and `X-Forwarded-*` headers). If a proxy changes the host between `/authorize` and the callback, the IdP rejects the flow. A trailing slash is removed.

**`FLUXGATE_ENCRYPTION_KEY`.** It must be the standard base64 encoding of exactly 32 bytes (not base64url, no other length). Generate it with:

```sh
openssl rand -base64 32
```

Client secrets are encrypted with AES-256-GCM, with a random 96-bit nonce per value, and stored as `base64(nonce || ciphertext)`. The provider ID is bound to the ciphertext as associated data, so a secret copied between provider rows does not decrypt. Keep the key stable: if you change it, stored secrets stop decrypting and you must save them again. If the variable is unset or invalid, saving a non-empty `clientSecret` returns `400 {"error":"encryption_key_missing"}`. An invalid key also logs a startup warning.

**`FLUXGATE_SSO_<SLUG>_CLIENT_SECRET`.** Take the provider slug, uppercase it and replace every character that is not a letter or digit (for example `-`) with `_`. The slug `my-idp` gives `FLUXGATE_SSO_MY_IDP_CLIENT_SECRET`. When this variable is set and not empty, it wins over the stored secret and the provider reports `clientSecretFromEnv: true`. This needs no encryption key, which suits secret managers and Kubernetes secrets.

### Managing providers

Admins manage providers in the admin UI SSO page or with the API under `/api/v1/sso/*`. All of these routes are system-admin only. The OpenAPI document lists them under the `SSO` tag.

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

`POST /api/v1/sso/providers/{id}/test` fetches the discovery document and the JWKS and returns `{"ok", "issuer", "authorizationEndpoint", "error"}`. Use it before you enable a provider. The test is admin-only and requests the issuer URL from the backend host, so it can reach internal hosts. We accept this server-side request forgery risk because only administrators, who are trusted, can call it. Deleting a provider deletes its identities and mappings. Users remain.

Client authentication is `client_secret_basic`. FluxGate uses `client_secret_post` only when the provider does not advertise basic.

### Redirect URI to register at the IdP

```
<public_base_url>/api/v1/auth/sso/<slug>/callback
```

For example `https://fluxgate.example.com/api/v1/auth/sso/okta/callback`. The match is exact.

## Who may sign in

Checks run in this order after the id_token is valid:

1. **Email.** The IdP must send an `email` claim (from the id_token or userinfo). Otherwise the login fails with `sso_email_missing`.
2. **Allowed domains.** If `allowedEmailDomains` is not empty, the email domain must be in the list and the IdP must send `email_verified: true`. An unverified or missing `email_verified` never satisfies a non-empty list (`sso_email_domain_not_allowed`). With an empty list, unverified emails are accepted.
3. **Identity.** A known `(provider, sub)` signs in the linked user.
4. **Linking.** For an unknown identity with an email that matches an existing user (case-insensitive), FluxGate links the identity only if `allowEmailLinking` is on and `email_verified` is `true`. It never links to a system admin account or to a system-client user. Otherwise the login fails with `sso_linking_not_allowed`.
5. **JIT.** Otherwise, if `jitProvisioning` is on, FluxGate creates a user without a password, with `authSource: "sso"`, not admin. If it is off, the login fails with `sso_user_not_provisioned`.
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
- **Admin.** Sync grants admin only to users who were not admins and marks the flag as SSO-managed. It revokes only an SSO-granted flag. Admins made manually, and admins that existed before SSO, are never revoked. If an admin saves a user who is already an SSO-granted admin in the UI with admin still on, the flag stays SSO-managed. To take ownership of the flag, revoke admin, then grant it again.
- **Last admin.** Sync never removes the last enabled admin. It keeps the flag, logs a warning and writes an activity `sso_role_sync_warning`, at most once per 24 hours per user.
- **Deleted targets.** Deleting a role or team deletes its mappings. Mappings whose target no longer exists are ignored.
- **Audit.** A login that changes anything writes an activity `sso_role_sync` with the added and removed role and team IDs and the admin change. Other activities: `sso_user_provisioned`, `sso_identity_linked`, `sso_login`, `sso_provider_created|updated|deleted`, `sso_mappings_updated`, `sso_settings_updated`.
- The users API shows `authSource`, `ssoManagedRoleIds`, `ssoManagedTeamIds` and `identities`.
- If sync is `authoritative` and the groups cannot be read (for example userinfo fails), the user logs in with SSO-managed roles removed. This fails closed.

## Enforce SSO and break-glass admin

`PUT /api/v1/sso/settings` with `{"enforceSso": true}` blocks password login for everyone who is not a system admin: `POST /api/v1/auth/login` returns `403 {"error":"sso_required"}` after the password check. System admins can always use a password. This is the break-glass path when the IdP is down or misconfigured, so keep at least one local admin with a strong password and do not let SSO manage every admin.

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
4. Groups: add a client scope mapper of type **Group Membership**, token claim name `groups`, **Full group path off**, add to ID token and userinfo. Group names then arrive as `fluxgate-devs`, not `/fluxgate-devs`. Or use realm roles: set `groupsClaim` to `realm_access.roles` and make sure the realm roles mapper has Add to ID token or Add to userinfo on (FluxGate reads userinfo when the id_token lacks the claim).
5. Provider: `issuerUrl` = `https://<host>/realms/<realm>`, `groupsClaim` = `groups` (or `realm_access.roles`).
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
