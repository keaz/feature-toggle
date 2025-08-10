-- Team-scoped contexts
CREATE TABLE IF NOT EXISTS contexts (
    id UUID PRIMARY KEY,
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    key VARCHAR(100) NOT NULL,
    UNIQUE(team_id, key)
);

CREATE TABLE IF NOT EXISTS context_entries (
    id UUID PRIMARY KEY,
    context_id UUID NOT NULL REFERENCES contexts(id) ON DELETE CASCADE,
    value TEXT NOT NULL CHECK (value <> ''),
    UNIQUE(context_id, value)
);
