-- Create cluster_nodes table for database-backed peer discovery
-- Similar to JGroups JDBC ping, nodes register themselves in the database
-- and discover other nodes by querying this table

CREATE TABLE IF NOT EXISTS cluster_nodes (
    node_id VARCHAR(255) PRIMARY KEY,
    listen_addr VARCHAR(255) NOT NULL,
    last_heartbeat TIMESTAMP NOT NULL DEFAULT NOW(),
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Index for efficient cleanup of stale nodes
CREATE INDEX idx_cluster_nodes_heartbeat ON cluster_nodes(last_heartbeat);

-- Nodes are considered dead if heartbeat is older than 60 seconds
-- Cleanup can be done periodically or on-demand
