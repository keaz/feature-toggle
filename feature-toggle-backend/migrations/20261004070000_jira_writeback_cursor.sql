-- Position of the Jira write-back capture scheduler in activity_log (JI-43).
-- One row. The first run inserts now(): no backfill of older activity.
CREATE TABLE jira_writeback_cursor (
  id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
  last_created_at TIMESTAMPTZ NOT NULL
);
