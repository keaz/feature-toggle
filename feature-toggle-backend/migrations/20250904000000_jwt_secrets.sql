-- Create JWT secrets table
CREATE TABLE jwt_secrets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    secret TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by UUID REFERENCES users(id),
    expires_at TIMESTAMPTZ NULL -- Optional expiry for secret rotation
);

-- Only one active secret at a time
CREATE UNIQUE INDEX idx_jwt_secrets_active_unique ON jwt_secrets (is_active) WHERE is_active = TRUE;

-- Index for quick lookup of active secret
CREATE INDEX idx_jwt_secrets_active ON jwt_secrets (is_active, created_at DESC);
