# fluxgate CLI

Command line client for the FluxGate admin API: flag reads, evaluation, approvals, stage changes, and AWS-style profiles and login sessions.

## Install

```bash
cargo install --path fluxgate-cli
```

## Quick start

```bash
fluxgate configure                 # url, login, team, environment, output
fluxgate whoami
fluxgate flags list
fluxgate evaluate --flag checkout --targeting-key user-1 --exit-code
```

## Profiles

Settings live in `~/.fluxgate/config`:

```ini
[default]
session = corp
team = payments
environment = staging
output = table

[profile checkout-prod]
session = corp
team = checkout
environment = production

[session corp]
url = https://fluxgate.example.com/api/v1
```

Profiles that name the same `session` share one login: log in once, then switch teams with `--profile checkout-prod` or `fluxgate teams use checkout`.

Static tokens, such as system-client tokens for CI, live in `~/.fluxgate/credentials` (mode 0600):

```ini
[ci-payments]
token = <system-client token>
```

A profile that uses a static token needs a `url` (and optionally `team`) in its config section.

`fluxgate` rewrites these files with `configure`, `login` and `teams use`. Comments are not kept when a file is rewritten, and comments must be on their own line (`key = value # note` makes `# note` part of the value).

## Where values come from

Each setting is taken from the first place that has it:

1. Command line flag
2. Environment variable
3. Profile in `~/.fluxgate/config`
4. The profile's session (url only)
5. Default

| Setting     | Flag                           | Environment variable                          | Default                        |
|-------------|--------------------------------|-----------------------------------------------|--------------------------------|
| profile     | `--profile`                    | `FLUXGATE_PROFILE`                            | `default`                      |
| url         | `--url` (`--base-url`)         | `FLUXGATE_URL`                                | `http://localhost:8080/api/v1` |
| team        | `--team` (`--team-id`)         | `FLUXGATE_TEAM` (`FLUXGATE_TEAM_ID`)          | none                           |
| environment | `--env` (`--environment-id`)   | `FLUXGATE_ENVIRONMENT` (`FLUXGATE_ENVIRONMENT_ID`) | none                      |
| output      | `--output json\|table\|text`, `--json` | `FLUXGATE_OUTPUT`                     | `table` on a terminal, else `json` |
| timeout     | `--timeout`                    | `FLUXGATE_TIMEOUT`                            | 30 seconds                     |

Credentials: `--token`, then `FLUXGATE_TOKEN`, then the profile's token in the credentials file, then the profile's login session. `fluxgate configure list` shows every value and its source.

File locations can be changed with `FLUXGATE_CONFIG_FILE` and `FLUXGATE_SHARED_CREDENTIALS_FILE`. Login sessions are cached in a `sessions` directory next to the config file.

Teams and environments can be given by name or id. A system-client token is bound to one team: `fluxgate` uses that team and refuses a different one.

## Login sessions

```bash
fluxgate login --password            # asks for username and password
fluxgate login --sso okta            # browser single sign-on; the session remembers the provider
fluxgate login                       # SSO when the session has sso_provider, else password
fluxgate login --use-device-code     # approve a code at <ui>/device from any machine (SSH, containers)
fluxgate login --no-browser          # with an SSO session: same as --use-device-code
fluxgate logout                      # revokes the session on the server
fluxgate logout --all
```

- **SSO** opens the browser at the backend's `/auth/sso/<slug>/authorize` with a local callback (`http://127.0.0.1:<random port>/callback`) and a PKCE challenge. The backend sends the one-time code to that address and the CLI exchanges it with the verifier, so an intercepted code is useless. If the browser does not open, the CLI prints the URL. It waits 5 minutes.
- **Device code** prints a link and a code like `BCDF-GHJK`. Open the link on any device, sign in to the FluxGate UI (password or SSO), check the code and approve. The CLI polls until the approval, a denial or the 10-minute expiry. Use it when the terminal has no browser.
- `fluxgate login --profile staging --url https://...` creates the profile and its session.
- A session belongs to one server. With a session credential, a different `--url`, `FLUXGATE_URL` or profile `url` is refused; `fluxgate login --url <new>` moves the session (and every profile that uses it) to the new server.
- Logging in again revokes the session it replaces.

Access tokens are refreshed automatically. When the refresh token has expired, commands fail with `session expired: run fluxgate login --profile <p>`.

## Commands

Flags and keys can be given by id or key. Write commands take JSON bodies with `--data`: inline JSON, `@file`, or `-` for stdin. `fluxgate <command> --help` lists every option.

| Area | Commands |
|------|----------|
| Flags | `flags list` (filters: `--tag --owner --flag-kind --external-key --lifecycle-stage --stale --include-archived --name`, paging: `--limit --offset --all`), `get`, `create`, `update`, `archive --yes`, `bulk`, `versions`, `diff`, `rollback`, `impact`, `impact-preview`, `search "<question>"` |
| Incidents | `flags kill <flag> --reason ... [--rollback-in MINUTES]`, `flags unkill`, `flags kill-switches` |
| Scheduling | `flags schedules`, `flags schedule --action ... --at <RFC 3339> --reason ...`, `flags unschedule`, `flags reschedule` |
| Jira links | `flags links`, `flags link <flag> PROJ-1 [--issue-url ...]`, `flags unlink` |
| Rollout | `rollout promote --flag <key> --env <name> --request ... [--reason --external-ref --freeze-override-reason]`, `evaluate [--exit-code]`, `freeze active`, `canary gates/set/analyze`, `criteria get/set/variants` |
| Approvals | `approvals list [--status pending,approved]`, `approve`, `reject`, `cancel`, `preview` |
| Jira | `jira rules/set-rules/rotate-secret/webhook-secret/writeback/writeback-test/writeback-resume/events/jobs/retry` |
| AI | `ai status/settings/set-settings/justify/suggest/backfill-kinds` |
| Observability | `metrics summary/rates/count/by-feature/system/experiments/growth/features [--period 24h --since 7d --flag ...]`, `audit`, `activity`, `watch <stream> [--count N]` (one JSON document per line) |
| Resources | `admin <resource> list/get/create/update/delete` for environments, contexts, clients, system-clients, pipelines, rollout-templates, metric-definitions, teams, users, roles, sso-providers, jira-integrations, approval-policies, freeze-windows, rule-groups |
| Accounts | `system-clients tokens/create-token/revoke-token/rotate-token`, `jwt-secrets list/rotate/deactivate-all`, `sso mappings/set-mappings/test/settings/set-settings`, `users roles/set-roles/set-teams/temporary-password`, `notifications show/channel/preference` |
| Config | `config export`, `config import --file <export> [--dry-run]` (creates missing environments and flags; stages, variants and criteria are not imported) |
| Edge | `edge evaluate <flag> --targeting-key ... [--exit-code]`, `edge evaluate-all` (OFREP; `edge_url` profile key or `FLUXGATE_EDGE_URL`, SDK key `edge_key` in credentials or `FLUXGATE_EDGE_KEY`) |
| Anything else | `api <METHOD> <PATH> [--data ...] [--query k=v]`; `{team}` and `{env}` in the path become the resolved ids |
| Shell | `completions bash/zsh/fish/powershell/elvish` |

## CI usage

```bash
export FLUXGATE_URL=https://fluxgate.example.com/api/v1
export FLUXGATE_TOKEN=$SYSTEM_CLIENT_TOKEN
if fluxgate evaluate --flag new-checkout --env production --targeting-key ci --exit-code; then
  echo "flag on"
fi
fluxgate rollout promote --flag new-checkout --env production --request DEPLOYMENT_REQUESTED \
  --reason "release 1.4" --external-ref PROJ-123
```

## Exit codes

| Code | Meaning                                                   |
|------|-----------------------------------------------------------|
| 0    | Success                                                   |
| 1    | Other error                                               |
| 2    | Usage or configuration error                              |
| 3    | Authentication: HTTP 401, no credentials, session expired |
| 4    | Forbidden (HTTP 403)                                      |
| 5    | Not found (HTTP 404)                                      |
| 6    | Conflict (HTTP 409)                                       |
| 7    | Server or network error (HTTP 5xx, timeout, connection)   |
| 10   | `evaluate --exit-code` and the flag is off                |

Errors go to stderr: `error: <message> (code <code>, HTTP <status>)`, or the server's error JSON when the output format is json.
