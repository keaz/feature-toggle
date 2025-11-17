-- Add operator support to feature_stage_criteria
-- This enables rich comparison operators beyond simple equality checks

-- Add operator column with default 'IN' for backward compatibility
ALTER TABLE feature_stage_criteria
ADD COLUMN IF NOT EXISTS operator VARCHAR(50) DEFAULT 'IN';

-- Add comment to explain supported operators
COMMENT ON COLUMN feature_stage_criteria.operator IS
'Operator for context matching: EQUALS, NOT_EQUALS, GREATER_THAN, LESS_THAN, GREATER_THAN_OR_EQUAL, LESS_THAN_OR_EQUAL, CONTAINS, STARTS_WITH, ENDS_WITH, REGEX, IN, NOT_IN, SEMVER_GREATER_THAN, SEMVER_LESS_THAN';

-- Create index for efficient operator-based queries
CREATE INDEX IF NOT EXISTS idx_feature_stage_criteria_operator ON feature_stage_criteria(operator);
