-- Outbox for Jira write-back (JI-42). The sender claims due `pending` rows.
CREATE TABLE jira_outbound_jobs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  integration_id UUID NOT NULL REFERENCES jira_integrations(id) ON DELETE CASCADE,
  issue_key TEXT NOT NULL,
  feature_id UUID NULL REFERENCES features(id) ON DELETE SET NULL,
  kind VARCHAR(20) NOT NULL CHECK (kind IN ('comment', 'remote_link', 'remote_link_delete')),
  payload JSONB NOT NULL DEFAULT '{}',
  dedupe_key TEXT NOT NULL UNIQUE,
  status VARCHAR(10) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'dead')),
  attempts INT NOT NULL DEFAULT 0,
  next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_error TEXT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  sent_at TIMESTAMPTZ NULL
);
-- due-job scan
CREATE INDEX ON jira_outbound_jobs (next_attempt_at) WHERE status = 'pending';
-- job list in the UI
CREATE INDEX ON jira_outbound_jobs (integration_id, created_at DESC);
-- at most one pending remote link refresh per integration, issue and feature
CREATE UNIQUE INDEX ON jira_outbound_jobs (integration_id, issue_key, feature_id)
  WHERE status = 'pending' AND kind = 'remote_link';
