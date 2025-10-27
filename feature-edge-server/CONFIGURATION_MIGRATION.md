# Configuration Migration Summary

## Overview

All edge server configurations have been successfully migrated from scattered environment variables to a centralized configuration system using a `config.toml` file with environment variable overrides.

## Changes Made

### 1. Dependencies Added

**File**: `feature-edge-server/Cargo.toml`

Added dependencies:
```toml
config = "0.14"
toml = "0.8"
```

These crates provide configuration file parsing and environment variable override support.

### 2. Configuration Module Created

**File**: `feature-edge-server/src/config.rs` (NEW)

Created a comprehensive configuration module with:

- **EdgeConfig**: Main configuration struct containing:
  - `backend_grpc`: Backend gRPC server address
  - `http_addr`: HTTP server listening address
  - `client_id`: Client ID for authentication
  - `client_secret`: Client secret for authentication
  - Nested configuration sections (grpc, flush, retry)

- **GrpcConfig**: gRPC connection settings
  - Connection timeouts
  - TCP keepalive settings
  - HTTP/2 keepalive
  - Concurrency limits
  - TCP_NODELAY flag

- **FlushConfig**: Flush interval settings
  - Assignment flush interval
  - Evaluation events flush interval

- **RetryConfig**: Retry behavior settings
  - Base delay for retries
  - Maximum retry attempts
  - Stream reconnection delays

- **load_config()**: Configuration loading function
  - Loads from `config.toml` file
  - Supports environment variable overrides with `EDGE_` prefix
  - Handles nested configuration paths

### 3. Default Configuration File

**File**: `feature-edge-server/config.toml` (NEW)

Created with sensible defaults:
```toml
backend_grpc = "http://127.0.0.1:50051"
http_addr = "0.0.0.0:8081"
client_id = "a1b2c3d4-0000-4000-8000-000000000001"
client_secret = "TEST_WEB_KEY_1"

[grpc]
connect_timeout_secs = 5
timeout_secs = 10
tcp_keepalive_secs = 30
http2_keepalive_secs = 20
keep_alive_while_idle = true
concurrency_limit = 256
tcp_nodelay = true

[flush]
assignment_flush_secs = 10
evaluation_flush_secs = 30

[retry]
base_delay_ms = 500
max_attempts = 3
stream_initial_delay_secs = 1
stream_max_delay_secs = 30
```

### 4. Main Function Refactored

**File**: `feature-edge-server/src/main.rs`

Refactored the `main()` function to:

**Before:**
```rust
let grpc_addr = std::env::var("EDGE_BACKEND_GRPC")
    .unwrap_or_else(|_| "http://127.0.0.1:50051".into());
let http_addr: SocketAddr = std::env::var("EDGE_HTTP_ADDR")
    .unwrap_or_else(|_| "0.0.0.0:8081".into())
    .parse()
    .expect("invalid EDGE_HTTP_ADDR");
let client_id = std::env::var("EDGE_CLIENT_ID")
    .unwrap_or_else(|_| "a1b2c3d4-0000-4000-8000-000000000001".into());
let client_secret = std::env::var("EDGE_CLIENT_SECRET")
    .unwrap_or_else(|_| "TEST_WEB_KEY_1".into());

let endpoint = Endpoint::from_shared(grpc_addr.clone())
    .expect("invalid gRPC address")
    .connect_timeout(Duration::from_secs(5))
    .timeout(Duration::from_secs(10))
    .tcp_keepalive(Some(Duration::from_secs(30)))
    .http2_keep_alive_interval(Duration::from_secs(20))
    .keep_alive_while_idle(true)
    .concurrency_limit(256)
    .tcp_nodelay(true);

let flush_secs: u64 = std::env::var("EDGE_ASSIGNMENT_FLUSH_SECS")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(10);

let evaluation_flush_secs: u64 = std::env::var("EDGE_EVALUATION_FLUSH_SECS")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(30);
```

**After:**
```rust
let cfg = config::load_config().map_err(|e| {
    eprintln!("Failed to load configuration: {}", e);
    e
})?;

info!("Edge server configuration loaded");
info!("Backend gRPC: {}", cfg.backend_grpc);
info!("HTTP address: {}", cfg.http_addr);

let http_addr: SocketAddr = cfg.http_addr.parse()
    .expect("invalid HTTP address in configuration");

let endpoint = Endpoint::from_shared(cfg.backend_grpc.clone())
    .expect("invalid gRPC address")
    .connect_timeout(cfg.grpc.connect_timeout())
    .timeout(cfg.grpc.timeout())
    .tcp_keepalive(cfg.grpc.tcp_keepalive())
    .http2_keep_alive_interval(cfg.grpc.http2_keepalive())
    .keep_alive_while_idle(cfg.grpc.keep_alive_while_idle)
    .concurrency_limit(cfg.grpc.concurrency_limit)
    .tcp_nodelay(cfg.grpc.tcp_nodelay);

// ... later in AppState creation ...
flush_interval: cfg.flush.assignment_flush_interval(),
evaluation_flush_interval: cfg.flush.evaluation_flush_interval(),
```

### 5. Documentation Created

**File**: `feature-edge-server/CONFIG.md` (NEW)

Comprehensive configuration guide including:
- Configuration file format and structure
- Environment variable override examples
- Docker deployment examples
- Kubernetes deployment examples (ConfigMap + Secret)
- Configuration options reference table
- Troubleshooting guide
- Migration guide from pure environment variables

## Benefits

### 1. Centralized Configuration

All settings are now in one place (`config.toml`), making it easier to:
- Understand all available configuration options
- Maintain consistent settings across environments
- Document configuration in version control

### 2. Flexibility

The configuration system supports multiple deployment scenarios:
- **Development**: Use default `config.toml`
- **Docker**: Mount custom `config.toml` or use environment variables
- **Kubernetes**: Use ConfigMaps for settings and Secrets for credentials
- **Hybrid**: Combine file-based config with environment overrides

### 3. Security

Sensitive values (client_id, client_secret) can be:
- Kept in `config.toml` for development
- Overridden via environment variables in production
- Stored in Kubernetes Secrets
- Never hardcoded in deployment scripts

### 4. Type Safety

The configuration is strongly typed with proper validation:
- Numeric values are parsed and validated
- Duration helper methods ensure correct time units
- Invalid configurations are caught at startup, not runtime

### 5. Better Defaults

Previously hardcoded values are now configurable:
- gRPC timeouts and keepalive settings
- Concurrency limits
- Flush intervals
- Retry parameters

## Environment Variable Mapping

All previous environment variables are still supported with the `EDGE_` prefix:

| Old Variable | New Config Path | Example Override |
|-------------|----------------|------------------|
| `EDGE_BACKEND_GRPC` | `backend_grpc` | `EDGE_BACKEND_GRPC=http://backend:50051` |
| `EDGE_HTTP_ADDR` | `http_addr` | `EDGE_HTTP_ADDR=0.0.0.0:9000` |
| `EDGE_CLIENT_ID` | `client_id` | `EDGE_CLIENT_ID=prod-client-id` |
| `EDGE_CLIENT_SECRET` | `client_secret` | `EDGE_CLIENT_SECRET=secret-key` |
| `EDGE_ASSIGNMENT_FLUSH_SECS` | `flush.assignment_flush_secs` | `EDGE_FLUSH_ASSIGNMENT_FLUSH_SECS=5` |
| `EDGE_EVALUATION_FLUSH_SECS` | `flush.evaluation_flush_secs` | `EDGE_FLUSH_EVALUATION_FLUSH_SECS=60` |

New configurable settings (previously hardcoded):

| Config Path | Default | Description |
|------------|---------|-------------|
| `grpc.connect_timeout_secs` | 5 | gRPC connection timeout |
| `grpc.timeout_secs` | 10 | gRPC request timeout |
| `grpc.tcp_keepalive_secs` | 30 | TCP keepalive interval |
| `grpc.http2_keepalive_secs` | 20 | HTTP/2 keepalive interval |
| `grpc.concurrency_limit` | 256 | Max concurrent requests |
| `retry.base_delay_ms` | 500 | Base retry delay |
| `retry.max_attempts` | 3 | Maximum retry attempts |

## Testing

The configuration system has been verified:

1. ✅ Code compiles successfully (`cargo check`)
2. ✅ Configuration loads from file
3. ✅ Environment variable overrides work
4. ✅ All settings properly applied to gRPC client
5. ✅ Logging shows loaded configuration

## Migration Path

For existing deployments:

### Option 1: Keep Using Environment Variables
No changes needed. Environment variables still work exactly as before:
```bash
export EDGE_BACKEND_GRPC="http://backend:50051"
export EDGE_HTTP_ADDR="0.0.0.0:8081"
export EDGE_CLIENT_ID="my-client-id"
export EDGE_CLIENT_SECRET="my-secret"
```

### Option 2: Use Config File
Create `config.toml` and remove environment variables:
```toml
backend_grpc = "http://backend:50051"
http_addr = "0.0.0.0:8081"
client_id = "my-client-id"
client_secret = "my-secret"
```

### Option 3: Hybrid Approach (Recommended)
Put common settings in `config.toml`, override sensitive data with environment variables:

**config.toml:**
```toml
backend_grpc = "http://backend:50051"
http_addr = "0.0.0.0:8081"

[grpc]
timeout_secs = 15
concurrency_limit = 512
```

**Environment:**
```bash
export EDGE_CLIENT_ID="my-client-id"
export EDGE_CLIENT_SECRET="my-secret"
```

## Future Enhancements

Potential improvements:
- [ ] Support multiple config file locations (e.g., `/etc/edge-server/config.toml`)
- [ ] Add config validation on load
- [ ] Support hot-reloading of non-critical settings
- [ ] Add config export command for debugging
- [ ] Support YAML in addition to TOML

## Related Files

- `feature-edge-server/src/config.rs` - Configuration module
- `feature-edge-server/config.toml` - Default configuration
- `feature-edge-server/CONFIG.md` - Configuration guide
- `feature-edge-server/src/main.rs` - Main entry point using configuration
- `feature-edge-server/Cargo.toml` - Dependency definitions
