# B06: OFREP returns 502 for bad credentials instead of 401/403

| Field | Value |
|---|---|
| Type | Bug (spec compliance) |
| Severity | Low |
| Status | Fixed on branch `fix/b06-ofrep-auth-status` |
| Crate | `feature-edge-server` |
| Behavior change | Yes, OFREP only: 502 becomes 401/403 for auth failures |
| Depends on | [B05](B05-ofrep-empty-client-secret.md) |

## Problem

`get_or_fetch_client_info` (`feature-edge-server/src/grpc_client.rs:124`) returns `Option`. Every failure becomes `None`, and the OFREP handlers map `None` to `502 Bad Gateway`:

```rust
// feature-edge-server/src/handlers.rs:1144 (also :1244)
let client_info = match get_or_fetch_client_info(&app, &client_id, &client_secret).await {
    Some(info) => info,
    None => return Err(actix_web::error::ErrorBadGateway("Failed to fetch client info")),
};
```

The OFREP spec says:
- 401: "Authentication credentials are missing, invalid, or expired."
- 403: "The client does not have permission to access the requested resource."

The backend `get_client_info` (`feature-toggle-backend/src/grpc/mod.rs:975`) returns these statuses:
- `InvalidArgument` for an empty or non-UUID client ID or secret
- `NotFound` for an unknown client
- `PermissionDenied` for a disabled client
- `Unauthenticated` for a wrong secret

## Fix

1. Add `try_get_or_fetch_client_info(..) -> Result<pb::GetClientInfoResponse, tonic::Status>`. Keep `get_or_fetch_client_info` as a thin wrapper (`.ok()`) so `/evaluate` (`handlers.rs:624`) keeps its current 502 behavior.
2. Have `fetch_client_info_via_grpc_uncached` return `Result` as well, keeping the same logging.
3. In both OFREP handlers, map the status:
   - `Unauthenticated`, `InvalidArgument`, `NotFound`: 401, using the OFREP error body shape already used in the handlers (`ofrep_error`)
   - `PermissionDenied`: 403
   - anything else: 502, as today

## Behavior impact

OFREP auth failures change from 502 to 401/403. `/evaluate` is unchanged.

## Tests

Use the mock backend in `feature-edge-server/src/grpc_client.rs` tests. Make the mock return each status and assert the OFREP HTTP status.

## Acceptance criteria

- OFREP returns 401 for a wrong secret or unknown client and 403 for a disabled client.
- `/evaluate` responses are unchanged.
- `cargo test -p feature-edge-server` passes.

## Update (2026-10-01): error body

OFREP 401/403 bodies are now `{"errorCode": "UNAUTHORIZED" | "FORBIDDEN", "errorDetails": "..."}` instead of `errorCode: "GENERAL"`. Feature-fetch auth failures map to 401/403 as well. Missing or malformed SDK keys and an `environment_id` mismatch return 401 with the same body. `/evaluate` still returns 502 when the configured client cannot be authenticated.
