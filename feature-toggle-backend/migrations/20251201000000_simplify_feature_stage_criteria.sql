-- Simplify feature_stage_criteria to only retain id, stage_id, and priority
-- Drops legacy targeting fields now handled by compound rules and variant allocations

-- Remove legacy columns
ALTER TABLE feature_stage_criteria
    DROP COLUMN IF EXISTS context_key,
    DROP COLUMN IF EXISTS context_id,
    DROP COLUMN IF EXISTS rollout_percentage,
    DROP COLUMN IF EXISTS serve,
    DROP COLUMN IF EXISTS operator;

-- Drop indexes tied to removed columns
DROP INDEX IF EXISTS idx_feature_stage_criteria_serve;
DROP INDEX IF EXISTS idx_feature_stage_criteria_operator;

-- Ensure priority is present and ordered for evaluation
ALTER TABLE feature_stage_criteria
    ALTER COLUMN priority SET DEFAULT 0,
    ALTER COLUMN priority SET NOT NULL;

-- Re-create priority index with the simplified schema
DROP INDEX IF EXISTS idx_feature_stage_criteria_priority;
CREATE INDEX IF NOT EXISTS idx_feature_stage_criteria_priority
    ON feature_stage_criteria(stage_id, priority);
