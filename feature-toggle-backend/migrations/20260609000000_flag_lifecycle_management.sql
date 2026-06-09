-- Expand feature lifecycle management for issue #2.

UPDATE features
SET lifecycle_stage = 'active'
WHERE lifecycle_stage = 'permanent';

ALTER TABLE features
DROP CONSTRAINT IF EXISTS features_lifecycle_stage_check;

ALTER TABLE features
ADD CONSTRAINT features_lifecycle_stage_check
CHECK (lifecycle_stage IN ('draft', 'active', 'deprecated', 'archived'));

ALTER TABLE features
ADD COLUMN IF NOT EXISTS owner TEXT NULL;

ALTER TABLE features
ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ NULL;

ALTER TABLE features
ADD COLUMN IF NOT EXISTS cleanup_reason TEXT NULL;

ALTER TABLE features
ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ NULL;

CREATE INDEX IF NOT EXISTS idx_features_lifecycle_visibility
ON features (team_id, lifecycle_stage, expires_at);
