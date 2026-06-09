-- Fine-grained system-client token scopes and token metadata.

ALTER TABLE system_client_tokens
    ADD COLUMN IF NOT EXISTS name VARCHAR(100) NOT NULL DEFAULT 'default',
    ADD COLUMN IF NOT EXISTS scopes TEXT[] NOT NULL DEFAULT ARRAY['evaluate', 'metrics:write', 'admin:read', 'flag:write']::TEXT[],
    ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_system_client_tokens_scopes ON system_client_tokens USING GIN(scopes);
CREATE INDEX IF NOT EXISTS idx_system_client_tokens_last_used_at ON system_client_tokens(last_used_at);
