-- Clients and Web Origins tables

CREATE TABLE clients (
    id UUID PRIMARY KEY,
    team_id UUID NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT true,
    client_type TEXT NOT NULL CHECK (client_type IN ('Web', 'Backend')),
    api_key TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ DEFAULT now(),
    UNIQUE (name, team_id)
);

CREATE TABLE client_web_origins (
    id UUID PRIMARY KEY,
    client_id UUID NOT NULL REFERENCES clients (id) ON DELETE CASCADE,
    origin TEXT NOT NULL,
    UNIQUE (client_id, origin)
);
