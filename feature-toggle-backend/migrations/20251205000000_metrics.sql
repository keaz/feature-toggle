CREATE TABLE metrics (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    key VARCHAR(100) NOT NULL,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    metric_type VARCHAR(50) NOT NULL CHECK (metric_type IN ('conversion', 'numeric', 'duration')),
    unit VARCHAR(50),
    success_criteria JSONB,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(team_id, key)
);

CREATE TABLE metric_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    metric_id UUID NOT NULL REFERENCES metrics(id) ON DELETE CASCADE,
    feature_key VARCHAR(255),
    environment_id UUID REFERENCES environments(id) ON DELETE SET NULL,
    user_context VARCHAR(255) NOT NULL,
    variant VARCHAR(100),
    value DOUBLE PRECISION NOT NULL,
    metadata JSONB,
    is_conversion BOOLEAN NOT NULL DEFAULT FALSE,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_metric_events_feature_variant
    ON metric_events(feature_key, variant, occurred_at);
CREATE INDEX idx_metric_events_metric
    ON metric_events(metric_id, occurred_at);
CREATE INDEX idx_metric_events_user
    ON metric_events(user_context, occurred_at);
CREATE INDEX idx_metric_events_environment
    ON metric_events(environment_id, occurred_at);

-- Deduplicate conversion events to one per user/feature/environment/variant
CREATE UNIQUE INDEX metric_events_conversion_unique
    ON metric_events(metric_id, feature_key, environment_id, user_context, variant)
    WHERE is_conversion;

-- Aggregations table (pre-computed for performance)
CREATE TABLE metric_aggregations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    metric_id UUID NOT NULL REFERENCES metrics(id) ON DELETE CASCADE,
    feature_key VARCHAR(255),
    environment_id UUID REFERENCES environments(id) ON DELETE SET NULL,
    variant VARCHAR(100),
    time_bucket TIMESTAMPTZ NOT NULL, -- hour/day bucket
    sample_size BIGINT NOT NULL,
    sum_value DOUBLE PRECISION,
    mean_value DOUBLE PRECISION,
    min_value DOUBLE PRECISION,
    max_value DOUBLE PRECISION,
    p50_value DOUBLE PRECISION,
    p95_value DOUBLE PRECISION,
    p99_value DOUBLE PRECISION,
    conversion_count BIGINT, -- for conversion metrics
    conversion_rate DOUBLE PRECISION,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(metric_id, feature_key, environment_id, variant, time_bucket)
);

CREATE INDEX idx_metric_aggregations_lookup
    ON metric_aggregations(metric_id, feature_key, environment_id, variant, time_bucket);
