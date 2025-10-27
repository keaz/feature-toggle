# Retry Configuration Integration

## Overview

Successfully integrated retry configuration from `config.toml` into the edge server code. Previously, retry parameters were defined in the config file but were hardcoded in the actual implementation.

## Changes Made

### 1. Updated AppState Structure

**File**: `src/main.rs`

Added `retry_config` field to `AppState`:

```rust
pub struct AppState {
    // ... existing fields ...
    // Retry configuration
    retry_config: config::RetryConfig,
}
```

This makes retry configuration accessible throughout the application via the shared state.

### 2. Updated fetch_feature_via_grpc

**Before**:
```rust
let retry_strategy = ExponentialBackoff::from_millis(500).take(3);
```

**After**:
```rust
let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
    .take(app.retry_config.max_attempts);
```

Now uses:
- `base_delay_ms` from config (default: 500ms)
- `max_attempts` from config (default: 3)

### 3. Updated fetch_client_info_via_grpc

**Before**:
```rust
let retry_strategy = ExponentialBackoff::from_millis(500).take(3);
```

**After**:
```rust
let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
    .take(app.retry_config.max_attempts);
```

Same configuration as `fetch_feature_via_grpc`.

### 4. Updated run_stream_task

**Before**:
```rust
let mut retry_delay = Duration::from_secs(1);
let max_retry_delay = Duration::from_secs(30);
```

**After**:
```rust
let mut retry_delay = app.retry_config.stream_initial_delay();
let max_retry_delay = app.retry_config.stream_max_delay();
```

Now uses:
- `stream_initial_delay_secs` from config (default: 1 second)
- `stream_max_delay_secs` from config (default: 30 seconds)

Also updated the reset logic:
```rust
retry_delay = app.retry_config.stream_initial_delay();
```

### 5. Updated run_evaluation_flush_task

**Before**:
```rust
let retry_strategy = ExponentialBackoff::from_millis(500).take(3);
```

**After**:
```rust
let retry_strategy = ExponentialBackoff::from_millis(app.retry_config.base_delay_ms)
    .take(app.retry_config.max_attempts);
```

Uses the same retry configuration as direct gRPC calls.

### 6. Updated Main Function

Added retry config to AppState initialization:

```rust
let state = AppState {
    // ... existing fields ...
    retry_config: cfg.retry.clone(),
};
```

### 7. Updated Test Helpers

Updated both test helper functions to include retry config:

```rust
fn test_state_with_feature(...) -> AppState {
    let state = AppState {
        // ... existing fields ...
        retry_config: config::RetryConfig::default(),
    };
    // ...
}

fn test_state_empty_cache() -> AppState {
    AppState {
        // ... existing fields ...
        retry_config: config::RetryConfig::default(),
    }
}
```

### 8. Cleaned Up Config Module

**File**: `src/config.rs`

Removed unused `base_delay()` method from `RetryConfig` impl since the code now directly accesses `base_delay_ms` field.

## Configuration Options

All retry behavior is now configurable via `config.toml`:

```toml
[retry]
# Base delay for exponential backoff in milliseconds
base_delay_ms = 500

# Maximum retry attempts for direct gRPC calls
max_attempts = 3

# Initial delay for stream reconnection in seconds
stream_initial_delay_secs = 1

# Maximum delay for stream reconnection in seconds
stream_max_delay_secs = 30
```

## Environment Variable Overrides

All retry settings can be overridden with environment variables:

```bash
# Override base delay
export EDGE_RETRY_BASE_DELAY_MS=1000

# Override max attempts
export EDGE_RETRY_MAX_ATTEMPTS=5

# Override stream delays
export EDGE_RETRY_STREAM_INITIAL_DELAY_SECS=2
export EDGE_RETRY_STREAM_MAX_DELAY_SECS=60
```

## Where Retry Config is Used

### 1. Direct gRPC Calls (base_delay_ms, max_attempts)

Used in:
- `fetch_feature_via_grpc()` - Fetching feature definitions
- `fetch_client_info_via_grpc()` - Fetching client information
- `run_evaluation_flush_task()` - Pushing evaluation events

**Behavior**: Exponential backoff starting at `base_delay_ms`, doubling each attempt, up to `max_attempts`.

**Example with defaults**:
- Attempt 1: Immediate
- Attempt 2: Wait 500ms
- Attempt 3: Wait 1000ms (1s)
- Attempt 4: Wait 2000ms (2s)
- Give up after 3 retries

### 2. Stream Reconnection (stream_initial_delay_secs, stream_max_delay_secs)

Used in:
- `run_stream_task()` - Maintaining persistent gRPC stream connection

**Behavior**: Exponential backoff starting at `stream_initial_delay_secs`, doubling each failure, capped at `stream_max_delay_secs`.

**Example with defaults**:
- First failure: Wait 1s
- Second failure: Wait 2s
- Third failure: Wait 4s
- Fourth failure: Wait 8s
- Fifth failure: Wait 16s
- Sixth+ failures: Wait 30s (capped)

On successful connection, the delay resets to `stream_initial_delay_secs`.

## Benefits

### 1. Configurability
Retry behavior can now be tuned for different environments:
- **Development**: Fast retries for quick feedback
- **Production**: More patient retries for stability
- **Testing**: Faster timeouts to speed up tests

### 2. No Code Changes Required
Operators can adjust retry behavior without rebuilding the application:

```toml
# Production with slower backend
[retry]
base_delay_ms = 1000
max_attempts = 5
stream_max_delay_secs = 60
```

```toml
# Development with fast local backend
[retry]
base_delay_ms = 100
max_attempts = 2
stream_max_delay_secs = 10
```

### 3. Documentation
Configuration is self-documenting in `config.toml` with comments explaining each setting.

### 4. Environment-Specific Tuning
Different retry strategies for different deployment scenarios:

**Load-balanced backend** (may need more retries):
```bash
export EDGE_RETRY_MAX_ATTEMPTS=5
export EDGE_RETRY_STREAM_MAX_DELAY_SECS=60
```

**Single backend server** (may need longer delays):
```bash
export EDGE_RETRY_BASE_DELAY_MS=1000
export EDGE_RETRY_STREAM_INITIAL_DELAY_SECS=5
```

**High-availability setup** (can be aggressive):
```bash
export EDGE_RETRY_BASE_DELAY_MS=200
export EDGE_RETRY_MAX_ATTEMPTS=2
export EDGE_RETRY_STREAM_INITIAL_DELAY_SECS=1
```

## Testing

### Verification Steps

1. ✅ Code compiles successfully
2. ✅ All retry locations updated to use config
3. ✅ Test helpers updated with default config
4. ✅ No hardcoded retry values remain

### Testing Different Configurations

**Test fast retries**:
```toml
[retry]
base_delay_ms = 100
max_attempts = 2
```

**Test patient retries**:
```toml
[retry]
base_delay_ms = 2000
max_attempts = 10
stream_max_delay_secs = 120
```

**Test with environment variables**:
```bash
EDGE_RETRY_BASE_DELAY_MS=50 \
EDGE_RETRY_MAX_ATTEMPTS=1 \
./feature-edge-server
```

## Migration

### For Existing Deployments

No changes required! The default values match the previous hardcoded values:
- `base_delay_ms = 500` (was hardcoded)
- `max_attempts = 3` (was hardcoded)
- `stream_initial_delay_secs = 1` (was hardcoded)
- `stream_max_delay_secs = 30` (was hardcoded)

### To Customize Retry Behavior

Option 1 - Update `config.toml`:
```toml
[retry]
base_delay_ms = 1000
max_attempts = 5
```

Option 2 - Set environment variables:
```bash
export EDGE_RETRY_BASE_DELAY_MS=1000
export EDGE_RETRY_MAX_ATTEMPTS=5
```

Option 3 - Hybrid (config + env vars):
```toml
# config.toml
[retry]
base_delay_ms = 1000
max_attempts = 5
```

```bash
# Override only for specific deployment
export EDGE_RETRY_MAX_ATTEMPTS=10
```

## Related Files

- `feature-edge-server/src/main.rs` - Retry logic implementation
- `feature-edge-server/src/config.rs` - Configuration structures
- `feature-edge-server/config.toml` - Default configuration
- `feature-edge-server/CONFIG.md` - Configuration guide
- `feature-edge-server/RETRY_IMPLEMENTATION.md` - Retry implementation details
