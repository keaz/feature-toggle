# S01: Live-looking edge client secret is committed in feature-edge-server/config.toml

| Field | Value |
|---|---|
| Type | Security |
| Severity | High, if the credential is used in any shared or production environment |
| Status | Confirmed (`git ls-files feature-edge-server/config.toml` lists the file) |
| Crate | `feature-edge-server` (config) |
| Behavior change | No code change. Needs credential rotation by a human. |

## Problem

`feature-edge-server/config.toml` is tracked in git and contains `client_id` and a 48-character `client_secret`. Anyone with repo access, including all of git history, has this credential.

## Fix

1. **Human action:** if this client exists anywhere other than a disposable local DB, rotate its secret in the backend (or disable the client). An agent must not do this.
2. Replace the value in `feature-edge-server/config.toml` with a placeholder, for example `client_secret = "change-me"`, and add a comment pointing to `EDGE_CLIENT_SECRET`. This depends on [B10](B10-edge-env-overrides-ignored.md), or the env override will not apply.
3. Optional: rename the file to `config.example.toml`, add `feature-edge-server/config.toml` to `.gitignore`, and update `docker-compose.yml` and `feature-edge-server/CONFIG.md`. This changes the local setup, so confirm with the maintainer first.
4. Removing the secret from git history requires a history rewrite. Rotation alone is usually enough. Ask the maintainer before rewriting history.

## Acceptance criteria

- No real secret remains in tracked files.
- The local dev setup still works, documented in `CONFIG.md`.
