-- Add prior_assignment column to feature_evaluations table
ALTER TABLE feature_evaluations 
ADD COLUMN prior_assignment BOOLEAN NOT NULL DEFAULT FALSE;

-- Add index on prior_assignment for filtering queries
CREATE INDEX idx_feature_evaluations_prior_assignment ON feature_evaluations(prior_assignment);

-- Add composite index for common analytics queries
CREATE INDEX idx_feature_evaluations_feature_prior ON feature_evaluations(feature_key, prior_assignment, evaluated_at DESC);
