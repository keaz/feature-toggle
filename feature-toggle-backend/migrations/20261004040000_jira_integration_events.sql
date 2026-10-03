-- JI-15: inbound Jira events and what the rule engine did with each one.
-- `results` is a list of {featureKey, environment, ruleId, action, outcome,
-- from, to, reason, ...}. `delivery_hash` identifies a delivery (issue key,
-- status, environment values, changelog id or `fields.updated`); a repeat
-- within 10 minutes returns the stored result. Rows older than 30 days are
-- deleted by the cleanup scheduler.
CREATE TABLE jira_integration_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    integration_id UUID NOT NULL REFERENCES jira_integrations(id) ON DELETE CASCADE,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    issue_key TEXT,
    jira_status TEXT,
    jira_actor JSONB,
    delivery_hash TEXT,
    results JSONB NOT NULL DEFAULT '[]',
    unknown_environments TEXT[] NOT NULL DEFAULT '{}',
    unknown_features TEXT[] NOT NULL DEFAULT '{}',
    ignored TEXT,
    error TEXT
);

CREATE INDEX idx_jira_integration_events_received
    ON jira_integration_events (integration_id, received_at DESC);
CREATE INDEX idx_jira_integration_events_delivery
    ON jira_integration_events (integration_id, delivery_hash);
CREATE INDEX idx_jira_integration_events_received_at
    ON jira_integration_events (received_at);
