# B10: Edge env var overrides for top-level keys are silently ignored

| Field | Value |
|---|---|
| Type | Bug (configuration) |
| Severity | High |
| Status | Confirmed (code traced, checked against config-rs 0.14.1 source) |
| Crate | `feature-edge-server` |
| Behavior change | Yes: env vars that are documented but ignored today start to apply. See "Behavior impact". |

## Problem

```rust
// feature-edge-server/src/config.rs:288 (load_config)
.add_source(
    config::Environment::with_prefix("EDGE")
        .separator("_")
        .try_parsing(true),
)
```

config-rs 0.14.1 replaces every separator in the key with `.` (`src/env.rs:274`, `key = key.replace(separator, ".")`). Because the separator is `_`:

| Env var | Key produced | Expected key |
|---|---|---|
| `EDGE_CLIENT_SECRET` | `client.secret` | `client_secret` |
| `EDGE_CLIENT_ID` | `client.id` | `client_id` |
| `EDGE_BACKEND_GRPC` | `backend.grpc` | `backend_grpc` |
| `EDGE_HTTP_ADDR` | `http.addr` | `http_addr` |
| `EDGE_GRPC_TIMEOUT_SECS` | `grpc.timeout.secs` | `grpc.timeout_secs` |

`EdgeConfig` does not use `deny_unknown_fields`, so the wrong keys are dropped silently.

The only nested key that works today is `EDGE_GRPC_COMPRESSION` (`grpc.compression`), because its field name has no underscore.

## How it fails

- `docker-compose.yml:35-38` sets `EDGE_BACKEND_GRPC`, `EDGE_HTTP_ADDR`, `EDGE_CLIENT_ID` and `EDGE_CLIENT_SECRET`. All four are ignored. The values come from the mounted `feature-edge-server/config.toml`, where `backend_grpc = "http://localhost:50051"`. Inside the container, `localhost` is the edge container itself, so the compose edge cannot reach the backend.
- `DOCKER.md:60-63` documents `docker run -e EDGE_CLIENT_ID=... -e EDGE_CLIENT_SECRET=...` with no config file. The image has no `config.toml` (`feature-edge-server/Dockerfile` copies only `log4rs.yaml`), so startup fails with a missing-field error such as `missing field client_id`.

## Fix

Use `_` after the prefix and `__` between nesting levels, and keep the old form for `EDGE_GRPC_COMPRESSION`:

```rust
.add_source(
    config::Environment::with_prefix("EDGE")
        .prefix_separator("_")
        .separator("__")
        .try_parsing(true),
)
```

Result:
- `EDGE_CLIENT_SECRET` becomes `client_secret`
- `EDGE_GRPC__TIMEOUT_SECS` becomes `grpc.timeout_secs`

Compatibility for `EDGE_GRPC_COMPRESSION`: before the new source, add a small explicit override that reads `EDGE_GRPC_COMPRESSION` with `std::env::var` and calls `set_override("grpc.compression", v)`. Log a deprecation warning that names `EDGE_GRPC__COMPRESSION`.

Update docs:
- `DOCKER.md` and `feature-edge-server/CONFIG.md`: describe the `__` rule for nested keys.
- `docker-compose.yml`: no change needed for the top-level vars.

## Behavior impact

- The compose stack: the edge now uses the env values (`feature_toggle_backend:50051`, seed client `a1b2c3d4-0000-4000-8000-000000000001` / `TEST_WEB_KEY_1`, which exists in `init.sql`) instead of the mounted `config.toml`. This is what the compose file intends.
- Any deployment that sets these env vars and relies on them being ignored changes. Before merging, check the deploy manifests for `EDGE_` vars with values that differ from their config files.

## Tests

In the `config.rs` tests (around line 306), use a temp config file and set env vars:
- `EDGE_CLIENT_SECRET=x` overrides `client_secret`.
- `EDGE_GRPC__TIMEOUT_SECS=7` overrides `grpc.timeout_secs`.
- `EDGE_GRPC_COMPRESSION=gzip` still works.

Env vars are process-global. Run these tests serially, for example with a `static Mutex` guard or the `serial_test` crate if it is already a dependency.

## Acceptance criteria

- The env overrides documented in `DOCKER.md` work without a config file.
- `cargo test -p feature-edge-server` passes.
- `make up`: the edge logs "Connected to backend gRPC http://feature_toggle_backend:50051".
