-- Feature metadata, tags, bulk actions, and analytics support for issues #22-#26.

ALTER TABLE features
ADD COLUMN IF NOT EXISTS purpose TEXT NULL;

ALTER TABLE features
ADD COLUMN IF NOT EXISTS reference_url TEXT NULL;

ALTER TABLE features
ADD COLUMN IF NOT EXISTS tags TEXT[] NOT NULL DEFAULT '{}';

CREATE INDEX IF NOT EXISTS idx_features_owner
ON features (team_id, owner);

CREATE INDEX IF NOT EXISTS idx_features_expiry
ON features (team_id, expires_at);

CREATE INDEX IF NOT EXISTS idx_features_tags
ON features USING GIN (tags);

CREATE INDEX IF NOT EXISTS idx_feature_dependencies_depends_on
ON feature_dependencies (depends_on_id);

CREATE INDEX IF NOT EXISTS idx_approval_requests_feature_status
ON approval_requests (feature_id, status, created_at DESC);
