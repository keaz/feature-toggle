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
fluxgate logout                      # revokes the session on the server
fluxgate logout --all
```

Access tokens are refreshed automatically. When the refresh token has expired, commands fail with `session expired: run fluxgate login --profile <p>`. SSO login is not available yet; SSO users can use a static token.

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
