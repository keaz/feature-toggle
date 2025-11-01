-- Create cluster_nodes table for database-backed peer discovery
-- Similar to JGroups JDBC ping, nodes register themselves in the database
-- and discover other nodes by querying this table

ALTER TABLE features ADD COLUMN active BOOLEAN NOT NULL DEFAULT TRUE;