-- Migration to properly support variant evaluations with success tracking
--
-- Changes:
-- 1. Rename evaluation_result to evaluation_success (keeps boolean for tracking success/failure)
-- 2. Add evaluation_value column (jsonb) to store the actual resolved value
-- 3. The variant column already exists from previous migration
--
-- This allows us to:
-- - Track whether an evaluation succeeded (evaluation_success)
-- - Store the actual value returned (evaluation_value) - can be boolean, string, number, or JSON
-- - Store the variant name if applicable (variant)

-- Step 1: Add new evaluation_value column (jsonb to support any type)
ALTER TABLE feature_evaluations
ADD COLUMN IF NOT EXISTS evaluation_value JSONB;

-- Step 2: Add new evaluation_success column (for tracking if evaluation succeeded)
ALTER TABLE feature_evaluations
ADD COLUMN IF NOT EXISTS evaluation_success BOOLEAN;

-- Step 3: Migrate existing data
-- For existing rows, copy evaluation_result to both evaluation_success and evaluation_value
UPDATE feature_evaluations
SET
    evaluation_success = evaluation_result,
    evaluation_value = to_jsonb(evaluation_result)
WHERE evaluation_success IS NULL;

-- Step 4: Make evaluation_success NOT NULL after migration
ALTER TABLE feature_evaluations
ALTER COLUMN evaluation_success SET NOT NULL;

-- Step 5: Add index for evaluation_success (used in dashboard queries)
CREATE INDEX IF NOT EXISTS idx_feature_evaluations_evaluation_success
ON feature_evaluations(evaluation_success);

-- Note: We're keeping the old evaluation_result column for backward compatibility
-- It can be dropped in a future migration after all clients are updated
-- For now, we'll deprecate it but keep it populated for safety

-- Step 6: Add comment to deprecated column
COMMENT ON COLUMN feature_evaluations.evaluation_result IS
'DEPRECATED: Use evaluation_success for success tracking and evaluation_value for the actual value. This column is kept for backward compatibility and will be removed in a future release.';
