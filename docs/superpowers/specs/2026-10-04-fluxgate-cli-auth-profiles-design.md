# FluxGate CLI: authentication, profiles and fixes (design)

Date: 2026-10-04
Status: approved design, awaiting spec review

## 1. Goal

Give the `fluxgate` CLI the same authentication and multi-team experience as
the AWS CLI:

- named profiles in `~/.fluxgate/config`, static tokens in
  `~/.fluxgate/credentials`;
- `fluxgate login` with password, SSO (browser, loopback redirect) and device
  code (headless);
- one login shared by many team profiles, the way AWS `[sso-session]` is shared
  by many `[profile]` sections;
- a predictable resolution order for profile, settings and credentials.

The same spec fixes the defects found in the existing commands and moves the
CLI to its own workspace crate.

### Users

- People at a terminal who work across several teams.
- CI jobs, which use system-client tokens (one token per team) from env vars or
  a credentials profile.

### Success criteria

- A user runs `fluxgate login` once and switches teams with `--profile` or
  `fluxgate teams use`, without logging in again.
- SSO-only users can log in from a laptop (browser) and over SSH (device code).
- CI keeps working with `FLUXGATE_URL`, `FLUXGATE_TOKEN` and `FLUXGATE_TEAM_ID`
  as today, with no config file.
- Existing commands return correct, complete results and meaningful exit codes.

### Out of scope

New command groups (kill switch, approve/reject, Jira, AI, admin),
`config import`/apply, OS keychain storage, shell completions. These follow in
later specs.

## 2. Facts from the current code

- User JWTs carry no team claim. `GET /teams` returns the teams of the caller
  (all teams for admins). One user session can therefore serve every team the
  user belongs to; the team is chosen per request.
- System-client tokens are bound to one team (`middleware/jwt_guard.rs`,
  claim team must match the resolved resource team).
- `POST /auth/login` returns `token`, `refreshToken`, `expiresIn`,
  `isTemporary`. `POST /auth/refresh` rotates both tokens. Reusing a rotated
  refresh token outside the grace window revokes the whole family
  (`logic/jwt_token_tx.rs`).
- SSO login today: `/auth/sso/{slug}/authorize` accepts only a local path in
  `redirect` (`sanitize_redirect`), and the callback always redirects to
  `<ui>/auth/sso/complete?code=...`. The one-time code is exchanged at
  `POST /auth/sso/exchange`. There is no way to return the code to a CLI.
- The current CLI (`feature-toggle-backend/src/bin/fluxgate.rs`) uses nothing
  from the backend library.

## 3. Files and resolution

### 3.1 `~/.fluxgate/config` (INI)

```ini
[default]
session = corp
team = payments
environment = staging
output = table

[profile checkout-prod]
session = corp
team = checkout            # team name or UUID
environment = production

[profile ci-payments]
url = https://fluxgate.example.com/api/v1   # no session: uses credentials file

[session corp]
url = https://fluxgate.example.com/api/v1
sso_provider = okta        # optional; when set, login defaults to SSO, else password
```

Profile keys: `session`, `url`, `team`, `environment`, `output`, `timeout`.
Session keys: `url`, `sso_provider`.

### 3.2 `~/.fluxgate/credentials` (INI, mode 0600)

```ini
[ci-payments]
token = <system-client token>
```

Only static tokens live here.

### 3.3 Session cache

`~/.fluxgate/sessions/<session>.json`, mode 0600:

```json
{ "accessToken": "...", "refreshToken": "...", "expiresAt": "RFC3339", "user": { "id": "...", "username": "..." } }
```

Refresh happens under an exclusive `fd-lock` on
`~/.fluxgate/sessions/<session>.lock`. After taking the lock the CLI reads the
file again; if another process refreshed in the meantime, it uses the new
token and does not refresh. This prevents two parallel commands from presenting
the same refresh token and revoking the family.

### 3.4 Profile selection

`--profile`, then `FLUXGATE_PROFILE`, then `default`.

### 3.5 Value resolution

For each key: flag, then env var, then profile, then session (`url` only),
then built-in default.

| Key         | Flag                                     | Env var                                         | Default                         |
|-------------|------------------------------------------|-------------------------------------------------|---------------------------------|
| url         | `--url` (alias `--base-url`)             | `FLUXGATE_URL`                                  | `http://localhost:8080/api/v1`  |
| team        | `--team` (alias `--team-id`)             | `FLUXGATE_TEAM` (alias `FLUXGATE_TEAM_ID`)      | none                            |
| environment | `--env`                                  | `FLUXGATE_ENVIRONMENT` (alias `FLUXGATE_ENVIRONMENT_ID`) | none                   |
| output      | `--output json\|table\|text` (alias `--json`) | `FLUXGATE_OUTPUT`                          | `table` on a TTY, else `json`   |
| timeout     | `--timeout <secs>`                       | `FLUXGATE_TIMEOUT`                              | 30                              |

File locations can be overridden with `FLUXGATE_CONFIG_FILE` and
`FLUXGATE_SHARED_CREDENTIALS_FILE`. Missing files are not an error.

### 3.6 Credential resolution

First match wins:

1. `--token`
2. `FLUXGATE_TOKEN`
3. `token` of the profile in the credentials file
4. the profile's `session` cache; refresh when less than 60 s remain. If the
   refresh is rejected: `session expired: run fluxgate login --profile <p>`
   (exit 3).
5. otherwise: `no credentials for profile <p>: run fluxgate configure or
   fluxgate login` (exit 3).

Commands that do not need credentials (`health`, `configure list`,
`configure list-profiles`, `configure get/set`) skip this step.

### 3.7 Team and environment resolution

- A team value that parses as a UUID is used as is. Otherwise the CLI calls
  `GET /teams` and matches the name exactly, case-insensitive. No match: error
  listing the available team names. Several matches: error listing their ids.
- For a system-client token, the team comes from the token. When the resolved
  team differs from the token's team claim, the CLI stops with an error
  (exit 2) instead of sending a request the server would answer for another
  team. The claim is read from the JWT payload without signature verification,
  for this check and for display only.
- An environment value that parses as a UUID is used as is. Otherwise the CLI
  resolves it by name through `GET /teams/{team}/environments`.

### 3.8 Backward compatibility

All current flags and env vars keep working through the aliases above. With no
config or credentials file, behaviour is the same as today: flags, then env,
then defaults.

## 4. Login and logout

### 4.1 Command

`fluxgate login [--profile p] [--sso <slug> | --password | --use-device-code] [--no-browser]`

Logs in the profile's `session`; every profile that references the session is
then logged in. A profile without a `session` gets an implicit session named
after the profile (`login` writes `session = <profile>` and a `[session
<profile>]` section with the resolved url). Method selection: explicit flag,
else SSO when the session has `sso_provider`, else password. `--no-browser`
with SSO switches to the device code flow.

### 4.2 Password (no backend change)

1. Prompt for username and password (hidden input via `rpassword`).
2. `POST /auth/login`.
3. If `isTemporary` is true, prompt for a new password and call
   `POST /auth/reset-password`, then continue with the returned session.
4. Write the session cache.

### 4.3 SSO with loopback redirect

CLI:

1. Bind a `tokio::net::TcpListener` on `127.0.0.1:0`.
2. Generate a PKCE verifier (43+ chars, URL-safe) and its S256 challenge.
3. Open the browser at
   `<url>/auth/sso/{slug}/authorize?cli_redirect=http://127.0.0.1:<port>/callback&cli_challenge=<challenge>`.
   If the browser cannot open, print the URL.
4. Accept exactly one HTTP request on `/callback`. With `code`: answer a short
   "Login complete, you can close this tab" page. With `error`: answer an error
   page and fail with the error code. Timeout after 5 minutes.
5. `POST /auth/sso/exchange {code, codeVerifier}` and write the session cache.

Backend:

- `authorize` accepts optional `cli_redirect` and `cli_challenge`. Both must be
  present together. `cli_redirect` must be exactly
  `http://127.0.0.1:<port>/callback` or `http://[::1]:<port>/callback`
  (port 1–65535, no userinfo, query or fragment); anything else fails with
  `sso_invalid_cli_redirect`. `cli_challenge` must be a 43-character base64url
  string. Both are stored on the `sso_login_state` row. The state cookie is
  set exactly as today, so the login-CSRF binding is kept.
- `callback`: when the consumed state has `cli_redirect_uri`, the one-time code
  is created with `cli_code_challenge` copied onto the `sso_login_code` row, and
  the browser is redirected to `<cli_redirect_uri>?code=<code>`. Errors go to
  `<cli_redirect_uri>?error=<code>` with the same error codes as the UI flow.
- `exchange` accepts optional `codeVerifier`. When the code row has a
  challenge, the request must carry a verifier whose S256 hash matches
  (constant-time compare); otherwise `invalid_sso_code` (401). Codes without a
  challenge (UI flow) behave as today. A verifier sent for a code without a
  challenge is ignored.

### 4.4 Device code

Backend, new table `cli_device_authorizations`:

| Column            | Type        | Notes                                              |
|-------------------|-------------|----------------------------------------------------|
| id                | uuid pk     |                                                    |
| device_code_hash  | text unique | SHA-256 of the device code                         |
| user_code         | text unique | `XXXX-XXXX`, letters from an alphabet without lookalikes |
| status            | text        | `pending`, `approved`, `denied`, `consumed`        |
| user_id           | uuid null   | set on approve/deny, fk users                      |
| interval_secs     | int         | 5                                                  |
| last_polled_at    | timestamptz null |                                               |
| expires_at        | timestamptz | created_at + 10 minutes                            |
| created_at        | timestamptz |                                                    |

Endpoints:

- `POST /auth/device/authorize` (public). Returns
  `{deviceCode, userCode, verificationUri, verificationUriComplete, expiresIn, interval}`.
  `verificationUri` is `<allowed_origin>/device`.
- `POST /auth/device/token {deviceCode}` (public). Returns 400 with
  `authorization_pending`, `slow_down` (polled faster than `interval`),
  `expired_token` or `access_denied`. When approved, it marks the row
  `consumed` in the same statement that reads it and returns a `LoginResponse`
  issued for the approving user. A second poll after success gets
  `expired_token`.
- `POST /auth/device/approve {userCode, approve}` (requires a user JWT; system
  clients are rejected). Only `pending`, unexpired rows can change. Writes an
  activity-log entry (CLI login approved or denied).
- Both public routes get an in-process rate limit built on `governor`, like
  `rest/jira_inbound_limit.rs`; over the limit they answer 429 with
  `Retry-After`.
- A scheduler job deletes rows past `expires_at`, next to the expired
  session-token cleanup.

UI (`feature-toggle-ui` repository): new `/device?code=...` page. The user signs
in by any method, sees the code, and approves or denies.

CLI: print the verification URL and user code, try to open
`verificationUriComplete`, poll `token` every `interval` seconds, add 5 s on
each `slow_down`, stop on `expired_token`, `access_denied` or after
`expiresIn`.

### 4.5 Logout

`fluxgate logout [--profile p | --all]`: `POST /auth/logout {refreshToken}`
(revokes the token family), then delete the session cache. The cache is
deleted even when the server call fails; the CLI prints a warning in that case.

## 5. Commands

### 5.1 New

- `fluxgate configure [--profile p]`: interactive. Ask for url and login method
  (password, SSO slug or static token). For password/SSO, run login, then let
  the user pick a team from `GET /teams` and an environment from the team's
  environments, then an output format, and write the config file. For a static
  token, write the credentials file and the profile's url/team.
- `fluxgate configure set <key> <value> [--profile p]`,
  `fluxgate configure get <key> [--profile p]`.
- `fluxgate configure list [--profile p]`: each key with its value and source
  (flag, env, profile, session, default) and the token type (static, session,
  none). Tokens are masked to the last 4 characters.
- `fluxgate configure list-profiles`.
- `fluxgate whoami`: user or system client, admin flag, profile, session,
  active team (name and id), available teams, token expiry. Uses
  `GET /users/{id}` for user sessions and the token claims for system clients.
- `fluxgate teams list`: available teams, the active one marked.
- `fluxgate teams use <name|id> [--profile p]`: same as
  `configure set team`.

### 5.2 Fixes to existing commands

1. **Output.** `--output json|table|text`; `--json` is an alias for
   `--output json`. Default `table` on a TTY and `json` otherwise.
2. **Approvals filter.** `approvals list --status` sends `statuses=` (the API
   parameter) and accepts a comma-separated list.
3. **Pagination.** `flags list` and `approvals list` get `--limit`, `--offset`
   and `--all` (follow `meta` until every page is fetched). `config export`
   always fetches every page.
4. **Team mismatch.** See 3.7.
5. **Promote fields.** `rollout promote` gets `--reason`, `--external-ref` and
   `--freeze-override-reason`.
6. **Key-based addressing.**
   - `flags get <id|key>`: UUID uses `/features/{id}`, otherwise
     `/teams/{team}/features/by-key/{key}`.
   - `rollout promote --flag <key> --env <name> --request <REQ>` uses
     `/teams/{team}/features/by-key/{key}/environments/{env}/request-change`.
     `rollout promote <stage-id>` stays.
   - `evaluate --env` accepts a name or UUID (`--environment-id` stays as an
     alias).
7. **Config export.** Returns the team, its environments and all features
   (all pages). The OFREP status call is removed.
8. **HTTP timeouts.** Request timeout from `timeout` (default 30 s), connect
   timeout 10 s.
9. **Exit codes.**

   | Code | Meaning                                                    |
   |------|------------------------------------------------------------|
   | 0    | success                                                    |
   | 1    | other error                                                |
   | 2    | usage or configuration error                               |
   | 3    | authentication: HTTP 401, no credentials, session expired  |
   | 4    | forbidden: HTTP 403                                        |
   | 5    | not found: HTTP 404                                        |
   | 6    | conflict or blocked: HTTP 409, freeze window               |
   | 7    | server or network error: HTTP 5xx, timeout, connect error  |
   | 10   | `evaluate --exit-code` and the flag is off                 |

   `evaluate --exit-code` exits 0 when the evaluated value is `true` and 10
   when it is `false`. Non-boolean values with `--exit-code` are a usage error
   (exit 2).
10. **URL encoding.** Path segments and query values are encoded through
    `reqwest::Url` path segments and `.query()`, not `format!`.

Errors print to stderr as `error: <message> (code <code>, HTTP <status>)`.
With `--output json` the server's error body is printed to stderr as JSON.

## 6. Crate layout

New workspace member `fluxgate-cli`, binary `fluxgate`:

```
fluxgate-cli/
  Cargo.toml
  README.md
  src/main.rs          parse args, dispatch, map errors to exit codes
  src/cli.rs           clap definitions; global args: profile, url, team, env, output, token, timeout
  src/error.rs         CliError with exit-code mapping
  src/config/files.rs  INI read/write, 0600 permissions, permission warning
  src/config/resolve.rs profile, value and credential resolution
  src/auth/session.rs  session cache, locked refresh
  src/auth/password.rs
  src/auth/sso_loopback.rs
  src/auth/device.rs
  src/api.rs           HTTP client: auth header, timeouts, error decoding, pagination helper
  src/output.rs        json, table, text rendering
  src/commands/        configure, login, logout, whoami, teams, flags, evaluate,
                       approvals, config_export, rollout, health
```

- Delete `feature-toggle-backend/src/bin/fluxgate.rs`. Remove `clap` from the
  backend dependencies if nothing else uses it.
- Dependencies: `clap` (derive, env), `reqwest` (rustls, json), `tokio`,
  `serde`, `serde_json`, `rust-ini`, `dirs`, `rpassword`, `open`, `fd-lock`,
  `sha2`, `base64`, `rand`, `comfy-table`, `is-terminal`, `chrono`, `url`.
  Dev: `wiremock`, `tempfile`.

## 7. Security

- No tokens, codes or verifiers in logs on either side. CLI session and token
  types redact them in `Debug`.
- Credentials and session cache files are created with mode 0600; the CLI warns
  when an existing file is readable by group or others.
- The loopback listener binds 127.0.0.1 only and serves one request.
- PKCE (S256) is mandatory for codes issued to the CLI.
- Device codes are stored hashed and are single use; user codes expire after
  10 minutes; public device routes are rate limited.
- Device approvals are recorded in the activity log.

## 8. Testing

CLI (no database):

- `config/resolve`: table tests for profile, value and credential order,
  including aliases, missing profile, static token over session. Temporary
  `HOME`, `FLUXGATE_CONFIG_FILE`, `FLUXGATE_SHARED_CREDENTIALS_FILE`.
- `config/files`: INI round trip keeps unknown keys; files are 0600 on unix.
- `auth/session`: two concurrent refreshes on one cache send one refresh
  request to the mock server; a rejected refresh gives `session expired` and
  exit 3.
- `api` and commands against `wiremock`: `statuses=` query, `--all`
  pagination, by-key routes, URL encoding, status to exit-code mapping,
  `evaluate --exit-code`, team mismatch, team and environment name resolution
  (no match, several matches).
- `sso_loopback`: one request with `code` succeeds, `error` fails, timeout.
- `device`: `authorization_pending`, `slow_down` (+5 s), `expired_token`,
  `access_denied`.

Backend:

- Unit: `cli_redirect` validation (accepted and rejected forms), PKCE S256
  check, user-code generation and format.
- DB repository tests in `tests/database/`: device authorization create,
  approve, single-use consume, expiry, cleanup.
- REST: `authorize` with `cli_redirect`, `callback` redirects to loopback,
  `exchange` rejects a missing or wrong verifier and still accepts UI codes,
  device endpoints including `access_denied` and 429.
- api-tests (Jest): device flow end to end (authorize, approve as admin, poll,
  call the API with the token, second poll refused). PKCE exchange end to end if
  the SSO api-tests already have a fake IdP; otherwise covered by the REST
  tests.
- Update the contract baseline on purpose; regenerate `.sqlx` with
  `cargo sqlx prepare -- --all-targets`.

## 9. Delivery order

Each step can ship on its own.

1. New `fluxgate-cli` crate with the module split; port the current commands
   and apply fixes 1–10 from 5.2.
2. Config and credentials files, resolution, `configure`, `teams`, `whoami`,
   password `login`, `logout`.
3. Backend SSO PKCE extension; `login --sso` with loopback.
4. Backend device flow, UI `/device` page (`feature-toggle-ui`), `login
   --use-device-code`.
5. Docs: `fluxgate-cli/README.md`, CLI section in `docs/sso.md`, CLAUDE.md
   entry for the new crate.
