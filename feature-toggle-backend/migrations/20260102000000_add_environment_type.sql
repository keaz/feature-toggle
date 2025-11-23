-- Add environment_type column to environments table
-- This allows us to categorize environments as Development or Production
-- and apply approval policies based on environment type rather than name matching

-- Add the environment_type column with a default value
ALTER TABLE environments
ADD COLUMN environment_type VARCHAR(50) NOT NULL DEFAULT 'Development';

-- Update existing environments to set type based on name (best effort)
-- You can adjust this logic based on your naming conventions
UPDATE environments
SET environment_type = 'Production'
WHERE LOWER(name) IN ('production', 'prod', 'live', 'prd');

-- Create an index for faster filtering by environment type
CREATE INDEX idx_environments_type ON environments(environment_type);

-- Add a comment explaining the column
COMMENT ON COLUMN environments.environment_type IS 'Type of environment: Development or Production. Used for approval policy scoping.';
