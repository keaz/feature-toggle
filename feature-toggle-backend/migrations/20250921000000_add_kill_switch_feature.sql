-- Add kill switch functionality to features table
-- This allows emergency disable/enable of features globally

ALTER TABLE features
ADD COLUMN kill_switch_enabled BOOLEAN NOT NULL DEFAULT true;

-- Add timestamps for tracking kill switch activation and automatic rollback
ALTER TABLE features 
ADD COLUMN kill_switch_activated_at TIMESTAMPTZ NULL;

ALTER TABLE features
ADD COLUMN rollback_scheduled_at TIMESTAMPTZ NULL;

-- Add index for efficient queries of disabled features
CREATE INDEX idx_features_kill_switch_enabled ON features (kill_switch_enabled);

-- Add index for rollback scheduling queries
CREATE INDEX idx_features_rollback_scheduled ON features (rollback_scheduled_at) WHERE rollback_scheduled_at IS NOT NULL;