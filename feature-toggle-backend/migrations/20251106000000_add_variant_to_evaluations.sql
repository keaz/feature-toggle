-- Add variant column to user_flag_assignments table
ALTER TABLE user_flag_assignments
ADD COLUMN IF NOT EXISTS variant VARCHAR(100);

-- Add variant column to feature_evaluations table
ALTER TABLE feature_evaluations
ADD COLUMN IF NOT EXISTS variant VARCHAR(100);

-- Add index for variant lookups
CREATE INDEX IF NOT EXISTS idx_user_flag_assignments_variant
ON user_flag_assignments(variant);

CREATE INDEX IF NOT EXISTS idx_feature_evaluations_variant
ON feature_evaluations(variant);
