CREATE TABLE IF NOT EXISTS feature_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    version_number INTEGER NOT NULL,
    snapshot JSONB NOT NULL,
    change_summary JSONB NOT NULL DEFAULT '[]'::jsonb,
    actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
    actor_name TEXT,
    source TEXT NOT NULL DEFAULT 'update',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT feature_versions_feature_number_key UNIQUE (feature_id, version_number),
    CONSTRAINT feature_versions_source_check CHECK (source IN ('update', 'rollback', 'kill_switch'))
);

CREATE INDEX IF NOT EXISTS idx_feature_versions_feature_created
ON feature_versions (feature_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_feature_versions_feature_number
ON feature_versions (feature_id, version_number DESC);
