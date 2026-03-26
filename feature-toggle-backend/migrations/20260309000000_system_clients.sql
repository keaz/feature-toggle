-- System clients for REST automation with JWT tokens

CREATE TABLE IF NOT EXISTS system_clients (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_used_at TIMESTAMPTZ,
    UNIQUE (team_id, name)
);

CREATE INDEX IF NOT EXISTS idx_system_clients_team_id ON system_clients(team_id);
CREATE INDEX IF NOT EXISTS idx_system_clients_enabled ON system_clients(enabled);
CREATE INDEX IF NOT EXISTS idx_system_clients_expires_at ON system_clients(expires_at);

CREATE TABLE IF NOT EXISTS system_client_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    system_client_id UUID NOT NULL REFERENCES system_clients(id) ON DELETE CASCADE,
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TIMESTAMPTZ,
    is_revoked BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_system_client_tokens_hash ON system_client_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_system_client_tokens_client_id ON system_client_tokens(system_client_id);
CREATE INDEX IF NOT EXISTS idx_system_client_tokens_expires_at ON system_client_tokens(expires_at);
CREATE INDEX IF NOT EXISTS idx_system_client_tokens_is_revoked ON system_client_tokens(is_revoked);
CREATE INDEX IF NOT EXISTS idx_system_client_tokens_cleanup ON system_client_tokens(expires_at, is_revoked);
