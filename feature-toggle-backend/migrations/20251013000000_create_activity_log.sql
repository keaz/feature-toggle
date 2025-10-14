-- Create activity log table to track all user activities and system events
CREATE TABLE activity_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    activity_type VARCHAR(50) NOT NULL, -- e.g., 'feature_created', 'feature_deployed', 'user_added'
    entity_type VARCHAR(50) NOT NULL,   -- e.g., 'feature', 'user', 'client', 'team'
    entity_id VARCHAR NOT NULL,         -- ID of the affected entity
    actor_id UUID,                      -- User who performed the action (nullable for system events)
    actor_name VARCHAR,                 -- Cached actor name for easier display
    description TEXT NOT NULL,          -- Human-readable description of the activity
    metadata JSONB,                     -- Additional context/details about the activity
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Indexes for performance
CREATE INDEX idx_activity_log_created_at ON activity_log(created_at DESC);
CREATE INDEX idx_activity_log_activity_type ON activity_log(activity_type);
CREATE INDEX idx_activity_log_entity_type ON activity_log(entity_type);
CREATE INDEX idx_activity_log_entity_id ON activity_log(entity_id);
CREATE INDEX idx_activity_log_actor_id ON activity_log(actor_id);

-- Composite index for common queries (filtering by type and time)
CREATE INDEX idx_activity_log_type_time ON activity_log(activity_type, created_at DESC);
CREATE INDEX idx_activity_log_entity_time ON activity_log(entity_type, entity_id, created_at DESC);

-- Foreign key constraint to users table (nullable since some activities are system-generated)
ALTER TABLE activity_log 
ADD CONSTRAINT fk_activity_log_actor 
FOREIGN KEY (actor_id) REFERENCES users(id) ON DELETE SET NULL;
