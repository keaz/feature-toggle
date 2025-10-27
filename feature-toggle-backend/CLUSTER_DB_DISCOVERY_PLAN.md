# Database-Backed Cluster Discovery Implementation Plan

## Overview

This document outlines the implementation plan for migrating from static/DNS-based cluster discovery to a database-backed discovery mechanism similar to JGroups JDBC_PING. This will provide more robust peer discovery in containerized environments like Kubernetes.

## Current State

### Issues with Current Implementation
1. **Static Discovery**: Requires pre-configuration of all peer addresses
2. **DNS Discovery**: Limited reliability in dynamic environments
3. **Test Failures**: All three cluster replication tests hang indefinitely
   - `cluster_propagates_feature_updates_between_nodes`
   - `cluster_propagates_evaluation_events_between_nodes`
   - `cluster_dns_discovery_picks_up_peer`

### Existing Components
- ✅ Migration created: `20251025061946_create_cluster_nodes_table.sql`
- ❌ Current cluster implementation removed from `src/cluster/mod.rs`
- ❌ Implementation not yet started

## Architecture Design

### Database Schema

```sql
CREATE TABLE cluster_nodes (
    node_id VARCHAR(255) PRIMARY KEY,
    listen_addr VARCHAR(255) NOT NULL,
    last_heartbeat TIMESTAMP NOT NULL DEFAULT NOW(),
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_cluster_nodes_heartbeat ON cluster_nodes(last_heartbeat);
```

### Discovery Flow

```
┌─────────────────────────────────────────────────────────────┐
│  Node Startup                                                │
│  1. Generate or load node_id                                 │
│  2. Bind to listen_addr                                      │
│  3. Register self in cluster_nodes table                     │
│  4. Query for active peers (heartbeat < 60s old)             │
│  5. Connect to discovered peers                              │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  Heartbeat Loop (every 30s)                                  │
│  1. UPDATE cluster_nodes SET last_heartbeat = NOW()          │
│     WHERE node_id = self                                     │
│  2. Query for new/changed peers                              │
│  3. Connect to new peers                                     │
│  4. Disconnect from stale peers (heartbeat > 90s)            │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  Cleanup on Shutdown                                         │
│  1. DELETE FROM cluster_nodes WHERE node_id = self           │
│  2. Close all peer connections gracefully                    │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Steps

### Phase 1: Database Access Layer (Priority: HIGH)

**File**: `src/cluster/db_discovery.rs`

Create database operations for cluster node management:

```rust
pub struct ClusterNodeRepo {
    pool: sqlx::PgPool,
}

impl ClusterNodeRepo {
    // Register this node in the database
    pub async fn register_node(
        &self,
        node_id: &str,
        listen_addr: &str,
    ) -> Result<()>;

    // Update heartbeat timestamp
    pub async fn heartbeat(&self, node_id: &str) -> Result<()>;

    // Get all active peers (excluding self)
    pub async fn get_active_peers(
        &self,
        node_id: &str,
        timeout_secs: u64,
    ) -> Result<Vec<String>>;

    // Remove this node from cluster
    pub async fn deregister_node(&self, node_id: &str) -> Result<()>;

    // Clean up stale nodes (maintenance task)
    pub async fn cleanup_stale_nodes(&self, timeout_secs: u64) -> Result<u64>;
}
```

**Testing Requirements**:
- Unit tests for each database operation
- Integration tests with test database
- Edge cases: duplicate node_id, concurrent registrations

### Phase 2: Discovery Service (Priority: HIGH)

**File**: `src/cluster/discovery.rs`

Implement the discovery service that uses the database:

```rust
pub struct DbDiscoveryService {
    repo: ClusterNodeRepo,
    node_id: String,
    listen_addr: String,
    heartbeat_interval: Duration,
    stale_threshold: Duration,
}

impl DbDiscoveryService {
    // Start discovery service
    pub async fn start(self) -> Result<DiscoveryHandle>;

    // Background task: periodic heartbeat and peer discovery
    async fn heartbeat_loop(&self, shutdown: Receiver<()>);

    // Background task: periodic cleanup of stale nodes
    async fn cleanup_loop(&self, shutdown: Receiver<()>);
}

pub struct DiscoveryHandle {
    // Channel to receive newly discovered peers
    pub new_peers: Receiver<String>,

    // Channel to receive stale/removed peers
    pub removed_peers: Receiver<String>,

    // Shutdown handle
    shutdown_tx: Sender<()>,
}
```

**Key Features**:
- Non-blocking peer discovery
- Automatic reconnection to new peers
- Detection and removal of stale peers
- Graceful shutdown with cleanup

### Phase 3: Integration with Cluster Module (Priority: HIGH)

**File**: `src/cluster/mod.rs`

Update the cluster module to use database discovery:

```rust
pub enum DiscoveryConfig {
    Static { peers: Vec<String> },
    Dns { record: String, port: u16, refresh_ms: u64 },
    Database {
        heartbeat_interval_secs: u64,
        stale_threshold_secs: u64,
    },
}

pub struct ClusterConfig {
    pub enabled: bool,
    pub node_id: Option<String>,
    pub listen_addr: String,
    pub discovery: DiscoveryConfig,
    pub reconnect_delay_ms: u64,
}

pub fn start(
    config: &ClusterConfig,
    db_pool: sqlx::PgPool,  // Add database pool
    updates: broadcast::Sender<pb::FeatureUpdate>,
    evaluations: broadcast::Sender<FeatureEvaluationEvent>,
) -> Result<ClusterGuard>;
```

**Integration Points**:
- Pass database pool to cluster start function
- Initialize DbDiscoveryService when config uses Database mode
- Connect to peers discovered via database
- Handle peer additions/removals dynamically

### Phase 4: Update Tests (Priority: HIGH)

**File**: `tests/cluster_replication.rs`

Rewrite cluster tests to use database discovery:

```rust
#[tokio::test]
async fn cluster_db_discovery_propagates_updates() {
    // Setup test database
    let pool = setup_test_db().await;

    // Create two nodes with database discovery
    let cfg_a = ClusterConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{}", free_port()),
        discovery: DiscoveryConfig::Database {
            heartbeat_interval_secs: 1,
            stale_threshold_secs: 5,
        },
        node_id: Some("node-a".into()),
        reconnect_delay_ms: 100,
    };

    // Start nodes and verify they discover each other
    // Test feature update propagation
    // Test evaluation event propagation

    // Cleanup
    cleanup_test_db(pool).await;
}
```

**Test Coverage**:
- Node registration and discovery
- Feature update propagation between nodes
- Evaluation event propagation
- Stale node detection and removal
- Graceful shutdown and cleanup
- Concurrent node registration
- Network partition recovery

### Phase 5: Configuration Updates (Priority: MEDIUM)

**File**: `config.toml`

Add database discovery configuration:

```toml
[cluster]
enabled = true
listen_addr = "0.0.0.0:50052"
node_id = "${NODE_ID}"  # Can be set via env var

[cluster.discovery]
type = "database"
heartbeat_interval_secs = 30
stale_threshold_secs = 90
```

**Environment Variables**:
- `NODE_ID`: Unique identifier for this node (default: generated UUID)
- `CLUSTER_ENABLED`: Enable/disable clustering (default: false)
- `CLUSTER_LISTEN_ADDR`: Address to listen on for cluster connections

### Phase 6: Kubernetes Support (Priority: MEDIUM)

**File**: `k8s/statefulset.yaml` (if applicable)

Configure for Kubernetes deployment:

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: feature-toggle-backend
spec:
  serviceName: feature-toggle-backend
  replicas: 3
  template:
    spec:
      containers:
      - name: backend
        env:
        - name: POD_NAME
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: NODE_ID
          value: "$(POD_NAME)"
        - name: CLUSTER_ENABLED
          value: "true"
        - name: CLUSTER_LISTEN_ADDR
          value: "0.0.0.0:50052"
```

**Benefits**:
- No need for headless service DNS configuration
- Automatic peer discovery as pods scale
- Resilient to pod restarts and rescheduling

## Technical Considerations

### Concurrency

- Use optimistic locking or UPSERT for node registration
- Handle race conditions during simultaneous startup
- Ensure heartbeat updates don't block peer discovery

### Performance

- Index on `last_heartbeat` for efficient stale node queries
- Batch peer lookups to reduce database roundtrips
- Cache peer list locally, refresh periodically

### Reliability

- Retry database operations with exponential backoff
- Continue operating even if discovery temporarily fails
- Log discovery events for debugging

### Testing Strategy

1. **Unit Tests**: Database operations in isolation
2. **Integration Tests**: Multi-node cluster formation and message passing
3. **Chaos Tests**: Node failures, network partitions, database unavailability
4. **Performance Tests**: Scalability with 10+ nodes

## Migration Path

### For Existing Deployments

1. **Add Migration**: Apply `20251025061946_create_cluster_nodes_table.sql`
2. **Update Config**: Change discovery mode to `database`
3. **Rolling Update**: Deploy new version with graceful shutdown
4. **Monitor**: Check logs for successful peer discovery
5. **Cleanup**: Remove old static/DNS configuration

### Backward Compatibility

- Keep support for Static and DNS discovery modes
- Add deprecation warnings for non-database modes
- Provide migration guide in documentation

## Success Criteria

- ✅ All three cluster tests pass consistently
- ✅ Nodes discover each other within 5 seconds
- ✅ Feature updates propagate to all nodes within 100ms
- ✅ Evaluation events propagate to all nodes within 100ms
- ✅ Stale nodes removed within 2x stale_threshold
- ✅ Graceful shutdown cleans up database entries
- ✅ System operates correctly with 10+ nodes
- ✅ Zero downtime during rolling updates in production

## Timeline Estimate

| Phase | Estimated Time | Complexity |
|-------|---------------|------------|
| Phase 1: DB Access Layer | 4 hours | Medium |
| Phase 2: Discovery Service | 6 hours | High |
| Phase 3: Integration | 4 hours | Medium |
| Phase 4: Update Tests | 4 hours | Medium |
| Phase 5: Configuration | 2 hours | Low |
| Phase 6: K8s Support | 2 hours | Low |
| **Total** | **22 hours** | **Medium-High** |

## Next Steps

1. ✅ Create database migration (COMPLETED)
2. 🔄 Implement database access layer (`src/cluster/db_discovery.rs`)
3. Implement discovery service (`src/cluster/discovery.rs`)
4. Integrate with cluster module (`src/cluster/mod.rs`)
5. Update and fix cluster tests
6. Update configuration and documentation
7. Test in staging environment
8. Deploy to production with monitoring

## References

- JGroups JDBC_PING: https://github.com/jgroups-extras/jgroups-jdbc-ping
- Kubernetes StatefulSets: https://kubernetes.io/docs/concepts/workloads/controllers/statefulset/
- Original cluster tests: `tests/cluster_replication.rs`
- Migration file: `migrations/20251025061946_create_cluster_nodes_table.sql`

## Notes

- Current cluster implementation has been removed and needs to be rebuilt
- All existing cluster tests are failing (hanging indefinitely)
- Database-backed discovery provides better support for dynamic environments
- This approach aligns well with the existing Postgres-backed architecture
