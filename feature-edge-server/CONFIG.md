# Edge Server Configuration Guide

## Overview

The Feature Toggle Edge Server reads its settings from an optional configuration file (`config.toml`) and from environment variables, which override the file. The sectioned settings (`[grpc]`, `[flush]`, `[retry]` and `[cache]`) have built-in defaults. The four top-level settings do not, so every deployment must supply them.

## Configuration File

The edge server reads `config.toml` from the current working directory. To use a different file, set `EDGE_CONFIG_FILE` to its path.

The file is optional. If it is missing (including when `EDGE_CONFIG_FILE` points to a path that does not exist), the edge server is configured from environment variables alone. In that case `backend_grpc`, `http_addr`, `client_id` and `client_secret` must all be set through `EDGE_BACKEND_GRPC`, `EDGE_HTTP_ADDR`, `EDGE_CLIENT_ID` and `EDGE_CLIENT_SECRET`. These four settings have no default: if any of them is missing from both the file and the environment, the edge server fails at startup with a `Failed to load configuration` error that names the missing field.

### Example Configuration

The top-level values below are examples (the client credentials match the client seeded by `init.sql` for local use). Every value in the sections is the built-in default, so those lines can be left out.

```toml
# Required: backend gRPC server address
backend_grpc = "http://127.0.0.1:50051"

# Required: HTTP server listening address
http_addr = "0.0.0.0:8081"

# Required: client credentials for authentication
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

# Request compression: "none" or "gzip"
compression = "none"

[flush]
# Assignment flush interval in seconds
assignment_flush_secs = 10

# Longest wait for a partial batch of evaluation events, in seconds;
# whole batches are sent as soon as they fill
evaluation_flush_secs = 30

# Evaluation events waiting to be sent (bounded channel, and as many again
# in the send buffer); new events are dropped while both are full
evaluation_event_queue_capacity = 10000

# Max assignments per gRPC stream flush
assignment_flush_batch_size = 1000

# Max sticky assignments queued for the backend; new assignments are
# dropped while the queue is full
assignment_queue_capacity = 100000

# Max evaluation events per gRPC request
evaluation_flush_batch_size = 500

[retry]
# Base delay for exponential backoff in milliseconds
base_delay_ms = 500

# Maximum number of retries after the first attempt
max_attempts = 3

# Retry only applies to transient gRPC failures; NotFound is treated as a
# definitive miss and is not retried.

# Initial delay for stream reconnection in seconds
stream_initial_delay_secs = 1

# Maximum delay for stream reconnection in seconds
stream_max_delay_secs = 30

# Every (re)connect subscribes with an empty key set, so each stream starts
# with a full snapshot. When the backend emits a `lagged` stream marker, the
# edge also drops its local feature/assignment caches before reconnecting.

[cache]
# Maximum number of features to cache (LRU eviction when exceeded)
max_capacity = 10000

# Client info cache TTL in seconds
client_ttl_secs = 300

# Maximum number of client credentials whose info is cached
client_max_capacity = 1000

# Maximum number of sticky assignments (user, feature, environment) cached;
# the least recently used one is evicted when exceeded
assignment_max_capacity = 50000

# Seconds a cached assignment survives without being read or written
# (0 disables idle expiry)
assignment_time_to_idle_secs = 0
```

## Environment Variable Overrides

All configuration values can be overridden using environment variables with the `EDGE_` prefix. A single underscore follows the prefix, and a **double underscore (`__`)** separates a section from its key. Single underscores inside a name are kept as part of the key name.

- Top-level: `EDGE_<KEY>` (e.g., `EDGE_BACKEND_GRPC` sets `backend_grpc`, `EDGE_HTTP_ADDR` sets `http_addr`)
- Nested sections: `EDGE_<SECTION>__<KEY>` (e.g., `EDGE_GRPC__TIMEOUT_SECS` sets `grpc.timeout_secs`, `EDGE_FLUSH__ASSIGNMENT_FLUSH_SECS` sets `flush.assignment_flush_secs`)

A nested variable written with a single underscore (e.g. `EDGE_GRPC_TIMEOUT_SECS`) does not match any setting and is ignored.

Values that look like numbers or booleans are parsed as such. `EDGE_CLIENT_ID` and `EDGE_CLIENT_SECRET` are the exception: they are always read verbatim as strings, so credentials such as `0123` or `1e5` keep their exact form.

`EDGE_CONFIG_FILE` is not a setting. It selects the configuration file (see [Configuration File](#configuration-file)).

**Deprecated:** `EDGE_GRPC_COMPRESSION` (single underscore) is still accepted for `grpc.compression` and logs a deprecation warning. Use `EDGE_GRPC__COMPRESSION` instead. If both are set, `EDGE_GRPC__COMPRESSION` wins.

### Examples

```bash
# Override backend gRPC address
export EDGE_BACKEND_GRPC="http://backend.example.com:50051"

# Override HTTP listening address
export EDGE_HTTP_ADDR="0.0.0.0:9000"

# Override client credentials
export EDGE_CLIENT_ID="production-client-id"
export EDGE_CLIENT_SECRET="production-secret-key"

# Override gRPC settings (note the double underscore after the section)
export EDGE_GRPC__TIMEOUT_SECS=15
export EDGE_GRPC__CONCURRENCY_LIMIT=512
export EDGE_GRPC__COMPRESSION=gzip

# Override flush intervals
export EDGE_FLUSH__ASSIGNMENT_FLUSH_SECS=5
export EDGE_FLUSH__EVALUATION_FLUSH_SECS=60

# Override retry settings
export EDGE_RETRY__MAX_ATTEMPTS=5
export EDGE_RETRY__BASE_DELAY_MS=1000

# Override cache settings
export EDGE_CACHE__MAX_CAPACITY=50000
```

## Configuration Precedence

Environment variables take precedence over values in `config.toml`, which in turn take precedence over built-in defaults.

1. **Environment variables** (highest priority)
2. **config.toml file**
3. **Default values** (lowest priority; only the settings in `[grpc]`, `[flush]`, `[retry]` and `[cache]` have them)

## Docker Deployment

The image runs from `/app` and ships without a `config.toml`, so either mount one at `/app/config.toml` or set the four required settings through environment variables.

### Using config.toml

Mount your configuration file into the container. It must contain `backend_grpc`, `http_addr`, `client_id` and `client_secret`:

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
      EDGE_GRPC__TIMEOUT_SECS: "15"
      EDGE_FLUSH__ASSIGNMENT_FLUSH_SECS: "5"
      EDGE_CACHE__MAX_CAPACITY: "20000"
    ports:
      - "8081:8081"
```

### Hybrid Approach

Combine both for maximum flexibility. Together, the mounted file and the environment must supply all four required settings; in this example the file provides `http_addr`:

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

These settings have no default. Each one must be set in `config.toml` or through its environment variable, or the edge server fails to start.

| Setting | Type | Default | Environment variable | Description |
|---------|------|---------|----------------------|-------------|
| `backend_grpc` | String | None (required) | `EDGE_BACKEND_GRPC` | Backend gRPC server address, for example `http://127.0.0.1:50051` |
| `http_addr` | String | None (required) | `EDGE_HTTP_ADDR` | HTTP server listening address, for example `0.0.0.0:8081` |
| `client_id` | String | None (required) | `EDGE_CLIENT_ID` | Client ID for authentication |
| `client_secret` | String | None (required) | `EDGE_CLIENT_SECRET` | Client secret for authentication |

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
| `compression` | String | `none` | gRPC request compression (`none` or `gzip`, lowercase) |

### Flush Settings (`[flush]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `assignment_flush_secs` | u64 | 10 | Assignment flush interval in seconds |
| `evaluation_flush_secs` | u64 | 30 | Longest time an evaluation event waits in a partial batch, and the interval of the evaluation log report. Whole batches are sent as soon as they fill (0 is treated as 1 ms) |
| `evaluation_event_queue_capacity` | usize | 10000 | Evaluation events waiting to be sent: the bounded channel holds this many, and the send buffer as many again. While both are full, new events are dropped (values below 1 are treated as 1) |
| `assignment_flush_batch_size` | usize | 1000 | Max assignments per gRPC stream flush (values below 1 are treated as 1) |
| `evaluation_flush_batch_size` | usize | 500 | Evaluation events per gRPC request; a whole batch is sent as soon as it is buffered (values below 1 are treated as 1; capped at `evaluation_event_queue_capacity`) |
| `assignment_queue_capacity` | usize | 100000 | Max sticky assignments queued for the backend. While the queue is full, new and requeued assignments are dropped (values below 1 are treated as 1; env: `EDGE_FLUSH__ASSIGNMENT_QUEUE_CAPACITY`) |

**Evaluation events:** The edge records one event per evaluation (`/evaluate` and OFREP), and the backend stores one row per event. Analytics count these rows: evaluation counts, cache hit rate, unique users and the live evaluation stream. Events are not sampled or aggregated, so every dropped event is missing from these numbers.

A handler puts the event on a bounded channel without waiting. The flush task moves events from the channel into its send buffer and pushes each batch of `evaluation_flush_batch_size` as soon as it is full, one request at a time. Every `evaluation_flush_secs` it also pushes the remaining partial batch and logs what it sent:

```
Successfully pushed 115200 evaluation events in 231 batch(es) (115200 processed) in the last 30001 ms: 9800 ms pushing (310 ms building requests), 120 buffered
```

So the edge records as many events per second as the backend accepts, not `evaluation_event_queue_capacity` per interval. The queue only has to absorb bursts and the time one push takes. If a push still fails after the retries in `[retry]`, the batch goes back into the buffer and the task waits for the next interval before it pushes again. During a backend outage or when the backend is slower than the evaluation rate, the buffer and then the channel fill up, and new events are dropped. Each interval the task logs the drops: `Dropped N evaluation events due to full queue (capacity=..., M dropped since start)`.

Memory stays bounded: at most `evaluation_event_queue_capacity` events in the channel and as many in the send buffer and the batch in flight. An event uses about 300 bytes plus its context attributes, about 1.5 KB with ten attributes, so the default of 10,000 holds up to about 30 MB in the worst case. Raise the capacity to ride out longer backend slowdowns without drops; lower it on small containers.

**Assignment queue:** The edge queues a sticky assignment for the backend once, when a user first gets a truthy result for a feature in an environment. Results served from the assignment cache are not queued again. Every `assignment_flush_secs` the flush task sends the queue to the backend, so while the backend is reachable the queue holds about the number of new assignments made in one flush interval. When a push fails, the batch is requeued and retried on the next cycle. During a backend outage the queue grows until it reaches `assignment_queue_capacity`, and assignments that do not fit are dropped. The flush task logs the number dropped in each cycle: `Dropped N user assignments due to full queue (capacity=...)`.

One queued assignment uses about 300 bytes, so the default of 100,000 holds up to about 30 MB. A dropped assignment affects only the record on the backend, not evaluation: the user keeps their cached result, and the backend does not read assignments back when the edge evaluates. The edge sends the assignment again when it evaluates the user anew, for example after the cached assignment is evicted or purged, or after a restart. Raise the capacity if the log reports drops and you need every assignment recorded through long outages.

### Retry Settings (`[retry]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `base_delay_ms` | u64 | 500 | Base delay for retries in milliseconds; the delay doubles on each retry, capped at 16 times this value |
| `max_attempts` | usize | 3 | Maximum number of retries after the first attempt (`0` disables retries) |
| `stream_initial_delay_secs` | u64 | 1 | Initial delay for stream reconnection in seconds |
| `stream_max_delay_secs` | u64 | 30 | Maximum delay for stream reconnection in seconds; lagged streams trigger a full snapshot resync on reconnect |

### Cache Settings (`[cache]`)

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `max_capacity` | u64 | 10000 | Maximum number of features to cache (LRU eviction when exceeded) |
| `client_ttl_secs` | u64 | 300 | How long a successful client authentication (client info fetched from the backend) is cached, in seconds. Rejected credentials are cached separately for a fixed 30 seconds. Both caches are keyed by client ID and a SHA-256 hash of the secret. |
| `client_max_capacity` | u64 | 1000 | Maximum number of client credentials whose info is cached (env: `EDGE_CACHE__CLIENT_MAX_CAPACITY`) |
| `assignment_max_capacity` | u64 | 50000 | Maximum number of sticky assignments (one per user, feature and environment) cached. The least recently used assignment is evicted when exceeded (env: `EDGE_CACHE__ASSIGNMENT_MAX_CAPACITY`) |
| `assignment_time_to_idle_secs` | u64 | 0 | Seconds a cached assignment survives without being read or written. `0` disables idle expiry, so assignments are evicted only at capacity (env: `EDGE_CACHE__ASSIGNMENT_TIME_TO_IDLE_SECS`) |

**Cache Capacity Recommendations:**

- **Small deployment** (< 100 features): `max_capacity = 1000`
- **Medium deployment** (100-1000 features): `max_capacity = 5000`
- **Large deployment** (1000-10000 features): `max_capacity = 10000` (default)
- **Very large deployment** (> 10000 features): `max_capacity = 50000` or higher

Memory usage estimate: Each feature uses approximately 1-5 KB depending on configuration complexity. A cache of 10,000 features typically uses 10-50 MB of memory.

**Sticky assignment cache:** The edge caches each truthy evaluation result per user, feature and environment, so a user keeps the same result until the feature changes. One entry uses about 450-500 bytes, so the default of 50,000 entries holds up to about 25 MB. Size `assignment_max_capacity` from your memory budget, not from the number of users: a 1 GB budget fits about 2,000,000 entries. Earlier versions had no bound, and an edge that served many distinct users across many flags grew until it ran out of memory.

Eviction does not change a user's variant. The cache only saves re-evaluation: evaluation is deterministic for the same feature configuration, targeting key and attributes, and weighted splits always place a targeting key in the same bucket (`SHA256("<flag_key>:<targetingKey>")`). An evicted user is evaluated again and gets the same variant. The difference from a cached result appears only when the result depends on something that changed since the assignment was cached: a targeting rule now matches the user's attributes differently, or the feature was changed without the edge purging its assignments. The same applies after an edge restart, because the cache is not persisted. The assignments the edge reports to the backend are recorded there but are not read back during evaluation.

**LRU Eviction:** When the cache reaches `max_capacity`, the least recently used features are automatically evicted to make room for new ones. This prevents unbounded memory growth while maintaining performance for frequently accessed features.

**Stream sync and stale entries:** The edge keeps its feature cache in sync over the backend's `StreamUpdates` stream. Every connection starts with a full snapshot of the team's features (`SNAPSHOT` messages), followed by live `UPSERT` and `DELETE` messages. A backend that supports it ends the snapshot with one `SNAPSHOT_COMPLETE` message. On that marker the edge removes cached features of its team that the snapshot did not contain (flags renamed or removed while the edge was disconnected) and drops their cached and pending assignments, as a `DELETE` does. If the stream drops before the marker, nothing is removed and the next connection starts over. With an older backend that never sends the marker, stale entries stay until LRU eviction.

**Rejected client credentials:** When the backend rejects a client ID and secret (wrong secret, unknown client or disabled client), the edge remembers the rejection for 30 seconds and answers repeated requests with the same credentials without calling the backend. A client that is created or re-enabled right after a failed attempt can therefore be rejected for up to 30 seconds. Transient backend errors are not cached.

## Troubleshooting

### Configuration Not Loading

1. **Check file location**: Ensure `config.toml` is in the current working directory when starting the edge server, or that `EDGE_CONFIG_FILE` points to it. A missing file is not reported as an error; the edge server simply runs without it.

   If startup fails with `Failed to load configuration: missing field ...`, one of the required settings (`backend_grpc`, `http_addr`, `client_id`, `client_secret`) is set neither in the file nor in the environment.

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

2. **Check nesting**: For nested values, separate the section and the key with a double underscore: `EDGE_GRPC__TIMEOUT_SECS`. With a single underscore (`EDGE_GRPC_TIMEOUT_SECS`) the variable matches no setting and is silently ignored.

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

   With many distinct users, sticky assignments usually take more memory than features. Reduce `assignment_max_capacity` (about 450-500 bytes per entry), or set `assignment_time_to_idle_secs` to release assignments of inactive users:
   ```toml
   [cache]
   assignment_max_capacity = 20000  # Reduce from default 50000
   assignment_time_to_idle_secs = 1800
   ```

2. **Frequent backend requests**: If you see many gRPC calls to fetch features, your cache may be too small. Increase `max_capacity`:
   ```toml
   [cache]
   max_capacity = 20000  # Increase from default 10000
   ```

3. **Check the configured capacity**: The edge server does not log individual evictions, but it logs the capacity in use at startup (`Initializing MappedFeatureCache with max_capacity=...` and `Initializing AssignmentCache with max_capacity=..., time_to_idle=...`). Confirm that it matches the value you intended.

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
export EDGE_FLUSH__ASSIGNMENT_FLUSH_SECS="10"
export EDGE_FLUSH__EVALUATION_FLUSH_SECS="30"
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
