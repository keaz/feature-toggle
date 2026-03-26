CREATE TABLE rollout_canary_gates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    stage_id UUID NOT NULL REFERENCES features_pipeline_stages(id) ON DELETE CASCADE,
    feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    environment_id UUID NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    metric_key VARCHAR(100) NOT NULL,
    baseline_variant VARCHAR(100) NOT NULL,
    canary_variant VARCHAR(100) NOT NULL,
    direction VARCHAR(32) NOT NULL CHECK (direction IN ('HIGHER_IS_BETTER', 'LOWER_IS_BETTER')),
    threshold_pct DOUBLE PRECISION NOT NULL CHECK (threshold_pct >= 0),
    min_sample_size BIGINT NOT NULL CHECK (min_sample_size > 0),
    window_minutes INT NOT NULL CHECK (window_minutes > 0),
    auto_rollback_on_fail BOOLEAN NOT NULL DEFAULT FALSE,
    rollback_in_minutes INT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(stage_id, metric_key, baseline_variant, canary_variant)
);

CREATE INDEX idx_rollout_canary_gates_stage
    ON rollout_canary_gates(stage_id);

CREATE INDEX idx_rollout_canary_gates_feature
    ON rollout_canary_gates(feature_id);

CREATE INDEX idx_rollout_canary_gates_enabled
    ON rollout_canary_gates(enabled)
    WHERE enabled = TRUE;

CREATE TABLE rollout_canary_gate_results (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    gate_id UUID NOT NULL REFERENCES rollout_canary_gates(id) ON DELETE CASCADE,
    feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    passed BOOLEAN NOT NULL,
    reason TEXT NOT NULL,
    baseline_sample_size BIGINT NOT NULL,
    canary_sample_size BIGINT NOT NULL,
    baseline_value DOUBLE PRECISION,
    canary_value DOUBLE PRECISION,
    regression_pct DOUBLE PRECISION,
    threshold_pct DOUBLE PRECISION NOT NULL,
    rollback_triggered BOOLEAN NOT NULL DEFAULT FALSE,
    rollback_error TEXT,
    evaluated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_rollout_canary_gate_results_gate_time
    ON rollout_canary_gate_results(gate_id, evaluated_at DESC);

CREATE INDEX idx_rollout_canary_gate_results_feature_time
    ON rollout_canary_gate_results(feature_id, evaluated_at DESC);
