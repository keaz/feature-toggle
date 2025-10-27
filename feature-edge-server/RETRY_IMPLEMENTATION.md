# Edge Server Retry Implementation

## Overview

This document describes the retry mechanism implemented for the edge server to handle connection failures with the backend server gracefully.

## Problem Statement

Previously, the edge server had no retry mechanism for:
1. Direct gRPC calls (`fetch_feature_via_grpc`, `fetch_client_info_via_grpc`)
2. Limited retry for streaming connection with fixed 3-second delay
3. No retry for push operations (user assignments and evaluation events)

This meant that temporary network issues or backend unavailability would immediately fail requests without attempting recovery.

## Solution

### 1. Added Retry Library

**Dependency**: `tokio-retry = "0.3"` added to `Cargo.toml`

This library provides:
- Exponential backoff strategies
- Retry spawn functionality
- Iterator-based retry timing

### 2. Retry for Direct gRPC Calls

#### `fetch_feature_via_grpc`
- **Strategy**: Exponential backoff starting at 500ms
- **Retries**: 3 attempts (500ms, 1s, 2s)
- **Behavior**: 
  - Retries automatically on any gRPC error
  - Logs success when feature is fetched
  - Logs error after all retries are exhausted
  - Returns `None` if all retries fail

#### `fetch_client_info_via_grpc`
- **Strategy**: Exponential backoff starting at 500ms
- **Retries**: 3 attempts (500ms, 1s, 2s)
- **Behavior**:
  - Retries automatically on any gRPC error
  - Logs success when client info is fetched
  - Logs error after all retries are exhausted
  - Returns `None` if all retries fail

### 3. Enhanced Streaming Connection Retry

#### `run_stream_task`
**Previous behavior**: Fixed 3-second delay between reconnection attempts

**New behavior**:
- **Initial delay**: 1 second
- **Max delay**: 30 seconds
- **Strategy**: Exponential backoff (doubles each time: 1s → 2s → 4s → 8s → 16s → 30s)
- **Reset**: Delay resets to 1s on successful connection
- **Logging**: 
  - Warns about retry delays
  - Logs successful connection
  - Logs stream errors with context

This prevents hammering the backend during prolonged outages while quickly recovering from brief interruptions.

### 4. Push Operations

#### User Assignments (`run_flush_task`)
- **Approach**: Single attempt per flush cycle
- **Reason**: Streaming calls consume the stream and can't be easily retried
- **Fallback**: Failed items are requeued for the next flush cycle
- **Logging**: Warns about requeueing on failure

#### Evaluation Events (`run_evaluation_flush_task`)
- **Strategy**: Exponential backoff starting at 500ms
- **Retries**: 3 attempts (500ms, 1s, 2s)
- **Behavior**:
  - Retries automatically on any gRPC error
  - Logs success with processed count
  - Requeues failed events for next flush cycle
  - Warns about requeueing on failure

## Configuration

All retry strategies are currently hardcoded but can be made configurable via environment variables if needed:

```rust
// Current values
const RETRY_BASE_DELAY_MS: u64 = 500;
const MAX_RETRIES: usize = 3;
const STREAM_INITIAL_DELAY_SECS: u64 = 1;
const STREAM_MAX_DELAY_SECS: u64 = 30;
```

Potential environment variables:
- `EDGE_RETRY_BASE_DELAY_MS` - Base delay for exponential backoff
- `EDGE_MAX_RETRIES` - Maximum retry attempts
- `EDGE_STREAM_INITIAL_DELAY` - Initial stream reconnection delay
- `EDGE_STREAM_MAX_DELAY` - Maximum stream reconnection delay

## Benefits

1. **Resilience**: Handles temporary network issues and backend restarts
2. **Reduced errors**: Most transient failures are resolved within retries
3. **Better UX**: Clients experience fewer 502 errors
4. **Intelligent backoff**: Prevents overwhelming the backend during outages
5. **Fast recovery**: Quick retry on first failure, slower on repeated failures

## Testing Recommendations

1. **Simulate network failure**: Use `iptables` or similar to block backend temporarily
2. **Backend restart**: Restart backend while edge is running
3. **Load testing**: Verify retries don't cause thundering herd
4. **Metrics**: Monitor retry counts and success rates

## Monitoring

Key metrics to track:
- Retry success rate per operation
- Average retry attempts before success
- Time spent in retry loops
- Requeue counts for flush operations

## Future Improvements

1. Make retry parameters configurable via environment variables
2. Add circuit breaker pattern for prolonged outages
3. Expose retry metrics via `/metrics` endpoint
4. Add jitter to prevent thundering herd (currently using fixed delays)
5. Implement connection pooling with health checks
