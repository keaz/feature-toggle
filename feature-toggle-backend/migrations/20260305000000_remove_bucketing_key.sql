-- Remove bucketing_key column from features_pipeline_stages
-- This is now replaced by targeting_key in the evaluation request per OpenFeature standard

ALTER TABLE features_pipeline_stages
DROP COLUMN IF EXISTS bucketing_key;
