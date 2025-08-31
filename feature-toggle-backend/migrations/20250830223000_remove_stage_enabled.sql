-- Remove legacy enabled column from features_pipeline_stages in favor of status
ALTER TABLE IF EXISTS features_pipeline_stages
DROP COLUMN IF EXISTS enabled;