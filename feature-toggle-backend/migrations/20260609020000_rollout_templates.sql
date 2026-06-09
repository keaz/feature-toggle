CREATE TABLE rollout_templates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NULL REFERENCES teams(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description TEXT,
    template_type VARCHAR(50) NOT NULL,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    is_system BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT rollout_templates_scope_check CHECK (
        (is_system = TRUE AND team_id IS NULL)
        OR (is_system = FALSE AND team_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_rollout_templates_team_name
    ON rollout_templates(team_id, lower(name))
    WHERE is_system = FALSE;

CREATE INDEX idx_rollout_templates_team
    ON rollout_templates(team_id)
    WHERE is_system = FALSE;
