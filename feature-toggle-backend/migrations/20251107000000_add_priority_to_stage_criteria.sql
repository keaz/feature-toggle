-- Add priority column to feature_stage_criteria for ordered evaluation
-- Lower priority values are evaluated first

ALTER TABLE feature_stage_criteria
ADD COLUMN IF NOT EXISTS priority INT NOT NULL DEFAULT 0;

-- Create index for efficient ordering
CREATE INDEX IF NOT EXISTS idx_feature_stage_criteria_priority
ON feature_stage_criteria(stage_id, priority);

-- Update existing rows to have sequential priority based on their current order
-- This ensures smooth transition for existing data
WITH numbered_criteria AS (
    SELECT
        id,
        ROW_NUMBER() OVER (PARTITION BY stage_id ORDER BY id) - 1 AS new_priority
    FROM feature_stage_criteria
)
UPDATE feature_stage_criteria
SET priority = numbered_criteria.new_priority
FROM numbered_criteria
WHERE feature_stage_criteria.id = numbered_criteria.id;
