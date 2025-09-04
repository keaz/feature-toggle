-- Create feature evaluations table to track all feature evaluation events
CREATE TABLE feature_evaluations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    feature_key VARCHAR NOT NULL,
    environment_id VARCHAR NOT NULL,
    client_id UUID NOT NULL,
    evaluated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    evaluation_result BOOLEAN NOT NULL,
    evaluation_context JSONB,
    user_context VARCHAR, -- extracted user identifier for easier querying
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

-- Indexes for performance
CREATE INDEX idx_feature_evaluations_feature_key ON feature_evaluations(feature_key);
CREATE INDEX idx_feature_evaluations_environment_id ON feature_evaluations(environment_id);
CREATE INDEX idx_feature_evaluations_client_id ON feature_evaluations(client_id);
CREATE INDEX idx_feature_evaluations_evaluated_at ON feature_evaluations(evaluated_at);
CREATE INDEX idx_feature_evaluations_user_context ON feature_evaluations(user_context);

-- Composite index for common queries
CREATE INDEX idx_feature_evaluations_feature_env_time ON feature_evaluations(feature_key, environment_id, evaluated_at DESC);

-- Foreign key constraint to clients table
ALTER TABLE feature_evaluations 
ADD CONSTRAINT fk_feature_evaluations_client 
FOREIGN KEY (client_id) REFERENCES clients(id) ON DELETE CASCADE;
