-- Links between a feature and issues in an external tracker (Jira only for now).
-- One feature can link many issues; one issue can link many features.
CREATE TABLE feature_external_links (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    system VARCHAR(20) NOT NULL CHECK (system IN ('jira')),
    -- Issue key such as 'PROJ-123', stored upper case.
    external_key TEXT NOT NULL,
    -- Optional browse URL, http(s) only (validated by the API).
    url TEXT,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT feature_external_links_unique UNIQUE (feature_id, system, external_key)
);

CREATE INDEX idx_feature_external_links_key ON feature_external_links (system, external_key);
