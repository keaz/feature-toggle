-- Add bucketing_key to feature stages and introduce criteria per stage

-- 1) Add bucketing_key to features_pipeline_stages for sticky sessions
ALTER TABLE IF EXISTS features_pipeline_stages
ADD COLUMN IF NOT EXISTS bucketing_key VARCHAR(100);

-- 2) Create feature_stage_criteria to replace stage-contexts with richer criteria
CREATE TABLE IF NOT EXISTS feature_stage_criteria (
    id UUID PRIMARY KEY,
    stage_id UUID NOT NULL REFERENCES features_pipeline_stages(id) ON DELETE CASCADE,
    context_key VARCHAR(100) NOT NULL,
    context_id UUID NOT NULL REFERENCES contexts(id) ON DELETE CASCADE,
    rollout_percentage INT NOT NULL CHECK (rollout_percentage >= 0 AND rollout_percentage <= 100)
);

-- Optional: helper index to speed up lookups by stage
CREATE INDEX IF NOT EXISTS idx_feature_stage_criteria_stage ON feature_stage_criteria(stage_id);
