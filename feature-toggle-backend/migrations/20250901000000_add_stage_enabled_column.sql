-- Add enabled column to features_pipeline_stages table
-- This column is managed by the application logic, not database triggers
ALTER TABLE features_pipeline_stages
ADD COLUMN enabled BOOLEAN NOT NULL DEFAULT false;

-- Update existing records: enabled = true only when status = 'DEPLOYED'
UPDATE features_pipeline_stages
SET enabled = (status = 'DEPLOYED');