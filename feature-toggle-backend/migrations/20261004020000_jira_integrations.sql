-- Jira integrations of a team and the status rules that map a Jira status to a
-- rollout action (design §3.7). Nothing acts on these rows until inbound events (JI-15).
CREATE TABLE jira_integrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    -- Base URL for links in the UI; optional, http(s) only (validated by the API).
    jira_base_url TEXT,
    -- SHA-256 (hex) of the inbound secret. The secret itself is shown once.
    secret_hash TEXT NOT NULL,
    -- 'labels', a custom field id such as 'customfield_10042', or a system field name.
    environment_field TEXT NOT NULL,
    -- Jira value -> FluxGate environment id, for example {"Production": "<uuid>"}.
    -- Keys are compared ignoring case.
    environment_aliases JSONB NOT NULL DEFAULT '{}',
    -- Environments where Jira is a trusted approver.
    jira_approved_environment_ids UUID[] NOT NULL DEFAULT '{}',
    -- Optional field holding FluxGate feature keys, in addition to feature links.
    feature_key_field TEXT,
    -- Shadow user that requests and executes changes for the integration.
    actor_user_id UUID NOT NULL REFERENCES users(id),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT jira_integrations_team_name_unique UNIQUE (team_id, name)
);

CREATE TABLE jira_status_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    integration_id UUID NOT NULL REFERENCES jira_integrations(id) ON DELETE CASCADE,
    -- Trimmed; compared ignoring case and surrounding spaces.
    jira_status TEXT NOT NULL,
    action VARCHAR(20) NOT NULL CHECK (action IN ('request', 'approve', 'deploy', 'rollback')),
    -- Optional filter; NULL means every environment the issue names.
    environment_ids UUID[],
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    position INT NOT NULL DEFAULT 0,
    CONSTRAINT jira_status_rules_unique UNIQUE (integration_id, jira_status, action)
);

CREATE INDEX idx_jira_status_rules_integration ON jira_status_rules (integration_id, position);
