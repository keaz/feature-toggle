-- Persist sticky user assignments per feature/environment
CREATE TABLE IF NOT EXISTS user_flag_assignments (
  user_id TEXT NOT NULL,
  feature_id UUID NOT NULL,
  environment_id UUID NOT NULL,
  assigned BOOLEAN NOT NULL,
  assigned_at TIMESTAMP NOT NULL DEFAULT now(),
  PRIMARY KEY (user_id, feature_id, environment_id)
);
