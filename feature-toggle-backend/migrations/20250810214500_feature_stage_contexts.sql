-- Feature Stage to Contexts association (many-to-many)
CREATE TABLE IF NOT EXISTS feature_stage_contexts (
    stage_id UUID NOT NULL REFERENCES features_pipeline_stages(id) ON DELETE CASCADE,
    context_id UUID NOT NULL REFERENCES contexts(id) ON DELETE CASCADE,
    PRIMARY KEY(stage_id, context_id)
);
