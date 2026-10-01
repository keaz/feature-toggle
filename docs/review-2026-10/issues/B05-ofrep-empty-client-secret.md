# B05: OFREP endpoints send an empty client secret, so OFREP never works

| Field | Value |
|---|---|
| Type | Bug (correctness, regression) |
| Severity | Critical |
| Status | Fixed on branch `fix/b05-ofrep-empty-secret` |
| Crate | `feature-edge-server` |
| Behavior change | Yes, intended: OFREP requests with the configured client ID start working |
| Decision | Use the **fallback** option: fill the secret from edge config when the header client ID matches the configured client ID. The OFREP spec allows this (see below). |
| Related | [B06](B06-ofrep-auth-error-status.md), [B07](B07-retry-backoff-math.md), [B08](B08-retry-permanent-grpc-errors.md), [B16](B16-edge-cache-not-team-scoped.md) |

## Problem

`extract_auth_from_headers` returns the header value as the client ID and an empty string as the secret:

```rust
// feature-edge-server/src/handlers.rs:850
if let Some(token) = auth_str.strip_prefix("Bearer ") {
    return Some((token.to_string(), String::new()));
}
if let Ok(key_str) = api_key.to_str() {           // X-API-Key
    return Some((key_str.to_string(), String::new()));
}
```

The OFREP handlers pass that empty secret to the backend:
- Single flag: `handlers.rs:1128`, then `get_or_fetch_client_info` at `:1144` and `get_or_fetch_feature` at `:1161`
- Bulk: `handlers.rs:1231`, then `get_or_fetch_client_info` at `:1244`

The backend rejects the request before looking at the client:

```rust
// feature-toggle-backend/src/grpc/mod.rs:984
if req.client_secret.is_empty() {
    return Err(Status::invalid_argument("client_secret is required"));
}
```

The edge server loads `client_id` and `client_secret` from `config.toml` into `AppState` (`feature-edge-server/src/main.rs:389`). All other backend calls use them (stream subscribe, flush, `/evaluate`). Only OFREP ignores them.

This is a regression. The old `src/main.rs.backup:200` (`resolve_credentials`) fell back to `app.client_secret` when the request had no secret. The fallback was lost when handlers moved to `handlers.rs`.

## How it fails

1. Client sends `POST /ofrep/v1/evaluate/flags/my-flag` with `Authorization: Bearer <configured client_id>`.
2. The edge calls `GetClientInfo` with `client_secret = ""`.
3. The backend returns `InvalidArgument`.
4. The edge retries a permanent error (B08) with broken backoff (B07): waits of 500 ms, 250 s, then ~34.7 h. The request hangs.
5. If it ever finishes, the edge returns 502 "Failed to fetch client info".

## OFREP spec check

The OFREP OpenAPI (`open-feature/protocol/service/openapi.yaml`) defines `BearerAuth` and `ApiKeyAuth` (`X-API-Key`) as optional, "if supported by the flag management system". It does not define what the token means. Mapping a client ID to its configured secret on the server side is an implementation detail and does not violate the spec.

Security trade-off: with the fallback, the client ID alone authenticates OFREP calls for the configured client. That matches `/evaluate`, which already uses config credentials without caller auth. Web clients still pass through `validate_web_origin`.

## Fix

1. Keep `extract_auth_from_headers` unchanged. Its unit tests (`handlers.rs:1411-1426`) assert the empty secret and stay valid.
2. Add a resolver next to it:

   ```rust
   /// Resolve OFREP credentials. When the caller supplies only the configured
   /// client ID, use the configured secret.
   fn resolve_ofrep_credentials(
       app: &AppState,
       http_req: &actix_web::HttpRequest,
   ) -> Option<(String, String)> {
       let (client_id, client_secret) = extract_auth_from_headers(http_req)?;
       if client_secret.is_empty() && client_id == app.client_id {
           return Some((client_id, app.client_secret.clone()));
       }
       Some((client_id, client_secret))
   }
   ```

3. Call `resolve_ofrep_credentials` in both OFREP handlers instead of `extract_auth_from_headers`.
4. Requests with any other client ID behave as today (still rejected). B06 turns that rejection into a 401.

## Behavior impact

- OFREP with the configured client ID: starts working.
- OFREP with any other client ID: unchanged.
- Missing headers: unchanged (401 "Missing explicit client credentials").

## Tests

Use the test module in `feature-edge-server/src/handlers.rs` (around line 1319) and the mock gRPC backend in `src/grpc_client.rs` tests (around line 201):
- `resolve_ofrep_credentials` with a Bearer header equal to `app.client_id` returns the configured secret.
- The same for `X-API-Key`.
- A different client ID returns an empty secret.
- No header returns `None`.
- Handler test: an OFREP single-flag request with the configured client ID reaches the mock backend with the configured secret and returns 200.

## Acceptance criteria

- Both OFREP handlers use the resolver.
- Existing `extract_auth_*` tests pass unchanged.
- `cargo test -p feature-edge-server` passes.
- Manual check: `make up`, then `curl -XPOST localhost:8081/ofrep/v1/evaluate/flags/<key> -H "X-API-Key: <client_id>" -H 'content-type: application/json' -d '{"context":{"targetingKey":"u1"}}'` returns 200. This needs B10, because compose currently ignores env vars.
