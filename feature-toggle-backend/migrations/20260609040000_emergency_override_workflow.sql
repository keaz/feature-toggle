ALTER TABLE features
    ADD COLUMN IF NOT EXISTS emergency_override_reason TEXT,
    ADD COLUMN IF NOT EXISTS emergency_override_expires_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS emergency_override_actor_id UUID REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS emergency_override_applied_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_features_emergency_override_expires_at
    ON features (emergency_override_expires_at)
    WHERE emergency_override_expires_at IS NOT NULL;
