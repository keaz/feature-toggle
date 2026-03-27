# Edge Server Configuration Guide

## Overview

The Feature Toggle Edge Server uses a configuration file (`config.toml`) for all settings, with support for environment variable overrides. This approach provides flexibility for different deployment environments while maintaining sensible defaults.

## Configuration File

The edge server looks for a `config.toml` file in the current working directory. If the file is not found, it will use default values.

### Default Configuration

```toml
# Backend gRPC server address
backend_grpc = "http://127.0.0.1:50051"

# HTTP server listening address
http_addr = "0.0.0.0:8081"

# Client credentials for authentication
client_id = "a1b2c3d4-0000-4000-8000-000000000001"
client_secret = "TEST_WEB_KEY_1"

[grpc]
# Connection timeout in seconds
connect_timeout_secs = 5

# Request timeout in seconds
timeout_secs = 10

# TCP keepalive interval in seconds
tcp_keepalive_secs = 30

# HTTP/2 keepalive interval in seconds
http2_keepalive_secs = 20

# Keep connection alive while idle
keep_alive_while_idle = true

# Maximum concurrent requests
concurrency_limit = 256

# Enable TCP_NODELAY
tcp_nodelay = true

[flush]
# Assignment flush interval in seconds
assignment_flush_secs = 10

# Evaluation events flush interval in seconds
evaluation_flush_secs = 30

[retry]
# Base delay for retries in milliseconds
base_delay_ms = 500

# Maximum number of retry attempts
max_attempts = 3

# Retry only applies to transient gRPC failures; NotFound is treated as a
# definitive miss and is not retried.

# Initial delay for stream reconnection in seconds
stream_initial_delay_secs = 1

# Maximum delay for stream reconnection in seconds
stream_max_delay_secs = 30

# When the backend emits a `lagged` stream marker, the edge drops its local
# feature/assignment caches and reconnects with an empty subscription key set
# so the next stream starts with a full snapshot resync.

[cache]
# Maximum number of features to cache (LRU eviction when exceeded)
max_capacity = 10000
```

## Environment Variable Overrides

All configuration values can be overridden using environment variables with the `EDGE_` prefix. The variable names follow the pattern:

- Top-level: `EDGE_<KEY>` (e.g., `EDGE_BACKEND_GRPC`, `EDGE_HTTP_ADDR`)
- Nested sections: `EDGE_<SECTION>_<KEY>` (e.g., `EDGE_GRPC_TIMEOUT_SECS`, `EDGE_FLUSH_ASSIGNMENT_FLUSH_SECS`)

### Examples

```bash
# Override backend gRPC address
export EDGE_BACKEND_GRPC="http://backend.example.com:50051"

# Override HTTP listening address
export EDGE_HTTP_ADDR="0.0.0.0:9000"

# Override client credentials
export EDGE_CLIENT_ID="production-client-id"
export EDGE_CLIENT_SECRET="production-secret-key"

# Override gRPC settings
export EDGE_GRPC_TIMEOUT_SECS=15
export EDGE_GRPC_CONCURRENCY_LIMIT=512

# Override flush intervals
export EDGE_FLUSH_ASSIGNMENT_FLUSH_SECS=5
export EDGE_FLUSH_EVALUATION_FLUSH_SECS=60

# Override retry settings
export EDGE_RETRY_MAX_ATTEMPTS=5
export EDGE_RETRY_BASE_DELAY_MS=1000

# Override cache settings
export EDGE_CACHE_MAX_CAPACITY=50000
```

## Configuration Precedence

Environment variables take precedence over values in `config.toml`, which in turn take precedence over hardcoded defaults.

1. **Environment variables** (highest priority)
2. **config.toml file**
3. **Default values** (lowest priority)

## Docker Deployment

### Using config.toml

Mount your configuration file into the container:

```yaml
services:
  edge-server:
    image: feature-edge-server:latest
    volumes:
      - ./config.toml:/app/config.toml
    ports:
      - "8081:8081"
```

### Using Environment Variables

```yaml
services:
  edge-server:
    image: feature-edge-server:latest
    environment:
      EDGE_BACKEND_GRPC: "http://backend:50051"
      EDGE_HTTP_ADDR: "0.0.0.0:8081"
      EDGE_CLIENT_ID: "${CLIENT_ID}"
      EDGE_CLIENT_SECRET: "${CLIENT_SECRET}"
      EDGE_GRPC_TIMEOUT_SECS: "15"
      EDGE_FLUSH_ASSIGNMENT_FLUSH_SECS: "5"
      EDGE_CACHE_MAX_CAPACITY: "20000"
    ports:
      - "8081:8081"
```

### Hybrid Approach

Combine both for maximum flexibility:

```yaml
services:
  edge-server:
    image: feature-edge-server:latest
    volumes:
      - ./config.toml:/app/config.toml  # Base configuration
    environment:
      EDGE_BACKEND_GRPC: "http://backend:50051"  # Override specific values
      EDGE_CLIENT_ID: "${CLIENT_ID}"
      EDGE_CLIENT_SECRET: "${CLIENT_SECRET}"
    ports:
      - "8081:8081"
```

## Kubernetes Deployment

### ConfigMap for config.toml

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: edge-server-config
data:
  config.toml: |
    backend_grpc = "http://feature-toggle-backend:50051"
    http_addr = "0.0.0.0:8081"

    [grpc]
    timeout_secs = 15
    concurrency_limit = 512

    [flush]
    assignment_flush_secs = 5
    evaluation_flush_secs = 60

    [cache]
    max_capacity = 20000
```

### Secret for Credentials

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: edge-server-credentials
type: Opaque
stringData:
  client-id: "production-client-id"
  client-secret: "production-secret-key"
```

### Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: edge-server
spec:
  replicas: 3
  selector:
    matchLabels:
      app: edge-server
  template:
    metadata:
      labels:
        app: edge-server
    spec:
      containers:
      - name: edge-server
        image: feature-edge-server:latest
        ports:
        - containerPort: 8081
        env:
        - name: EDGE_CLIENT_ID
          valueFrom:
            secretKeyRef:
              name: edge-server-credentials
              key: client-id
        - name: EDGE_CLIENT_SECRET
          valueFrom:
            secretKeyRef:
              name: edge-server-credentials
              key: client-secret
        volumeMounts:
        - name: config
          mountPath: /app/config.toml
          subPath: config.toml
      volumes:
      - name: config
        configMap:
          name: edge-server-config
```

## Configuration Options Reference

### Top-Level Settings

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `backend_grpc` | String | `http://127.0.0.1:50051` | Backend gRPC server address |
| `http_addr` | String | `0.0.0.0:8081` | HTTP server listening address |
| `client_id` | String | `a1b2c3d4-0000-4000-8000-000000000001` | Client ID for authentication |
| `client_secret` | String | `TEST_WEB_KEY_1` | Client secret for authentication |

### gRPC Settings (`[grpc]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `connect_timeout_secs` | u64 | 5 | Connection timeout in seconds |
| `timeout_secs` | u64 | 10 | Request timeout in seconds |
| `tcp_keepalive_secs` | u64 | 30 | TCP keepalive interval in seconds |
| `http2_keepalive_secs` | u64 | 20 | HTTP/2 keepalive interval in seconds |
| `keep_alive_while_idle` | bool | true | Keep connection alive while idle |
| `concurrency_limit` | usize | 256 | Maximum concurrent requests |
| `tcp_nodelay` | bool | true | Enable TCP_NODELAY |
| `compression` | String | `none` | gRPC request compression (`none` or `gzip`) |

### Flush Settings (`[flush]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `assignment_flush_secs` | u64 | 10 | Assignment flush interval in seconds |
| `evaluation_flush_secs` | u64 | 30 | Evaluation events flush interval in seconds |
| `evaluation_event_queue_capacity` | usize | 10000 | Evaluation event queue capacity (bounded channel) |
| `assignment_flush_batch_size` | usize | 1000 | Max assignments per gRPC stream flush |
| `evaluation_flush_batch_size` | usize | 500 | Max evaluation events per gRPC request |

### Retry Settings (`[retry]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `base_delay_ms` | u64 | 500 | Base delay for retries in milliseconds |
| `max_attempts` | usize | 3 | Maximum number of retry attempts |
| `stream_initial_delay_secs` | u64 | 1 | Initial delay for stream reconnection in seconds |
| `stream_max_delay_secs` | u64 | 30 | Maximum delay for stream reconnection in seconds; lagged streams trigger a full snapshot resync on reconnect |

### Cache Settings (`[cache]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `max_capacity` | u64 | 10000 | Maximum number of features to cache (LRU eviction when exceeded) |

**Cache Capacity Recommendations:**

- **Small deployment** (< 100 features): `max_capacity = 1000`
- **Medium deployment** (100-1000 features): `max_capacity = 5000`
- **Large deployment** (1000-10000 features): `max_capacity = 10000` (default)
- **Very large deployment** (> 10000 features): `max_capacity = 50000` or higher

Memory usage estimate: Each feature uses approximately 1-5 KB depending on configuration complexity. A cache of 10,000 features typically uses 10-50 MB of memory.

**LRU Eviction:** When the cache reaches `max_capacity`, the least recently used features are automatically evicted to make room for new ones. This prevents unbounded memory growth while maintaining performance for frequently accessed features.

## Troubleshooting

### Configuration Not Loading

1. **Check file location**: Ensure `config.toml` is in the current working directory when starting the edge server.

2. **Check file format**: Verify the TOML syntax is correct:
   ```bash
   # Use a TOML validator
   cat config.toml | python -c "import sys, toml; toml.load(sys.stdin)"
   ```

3. **Check logs**: The edge server logs configuration loading:
   ```
   Edge server configuration loaded
   Backend gRPC: http://127.0.0.1:50051
   HTTP address: 0.0.0.0:8081
   ```

### Environment Variables Not Working

1. **Check variable names**: Ensure they follow the `EDGE_` prefix convention.

2. **Check nesting**: For nested values, use underscores: `EDGE_GRPC_TIMEOUT_SECS`

3. **Check types**: Numeric values should be valid numbers, booleans should be `true` or `false`.

### Connection Issues

1. **Verify backend address**: Check `backend_grpc` is correct and reachable.

2. **Check timeouts**: Increase `connect_timeout_secs` and `timeout_secs` if needed.

3. **Verify client credentials**: Ensure `client_id` and `client_secret` are correct.

### Cache Issues

1. **High memory usage**: If the edge server is consuming too much memory, reduce `max_capacity`:
   ```toml
   [cache]
   max_capacity = 5000  # Reduce from default 10000
   ```

2. **Frequent backend requests**: If you see many gRPC calls to fetch features, your cache may be too small. Increase `max_capacity`:
   ```toml
   [cache]
   max_capacity = 20000  # Increase from default 10000
   ```

3. **Check cache statistics**: Monitor the edge server logs for cache eviction messages. If you see frequent evictions, consider increasing capacity.

## Migration from Environment Variables

If you were previously using environment variables exclusively, you can:

1. **Create a config.toml** with your common settings
2. **Keep environment-specific overrides** as environment variables
3. **Remove hardcoded environment variables** from deployment scripts/manifests

Example migration:

**Before:**
```bash
export EDGE_BACKEND_GRPC="http://backend:50051"
export EDGE_HTTP_ADDR="0.0.0.0:8081"
export EDGE_CLIENT_ID="my-client-id"
export EDGE_CLIENT_SECRET="my-secret"
export EDGE_ASSIGNMENT_FLUSH_SECS="10"
export EDGE_EVALUATION_FLUSH_SECS="30"
```

**After:**

Create `config.toml`:
```toml
backend_grpc = "http://backend:50051"
http_addr = "0.0.0.0:8081"

[flush]
assignment_flush_secs = 10
evaluation_flush_secs = 30
```

Keep only sensitive data as environment variables:
```bash
export EDGE_CLIENT_ID="my-client-id"
export EDGE_CLIENT_SECRET="my-secret"
```

This approach keeps sensitive credentials secure while making other settings easier to manage.
