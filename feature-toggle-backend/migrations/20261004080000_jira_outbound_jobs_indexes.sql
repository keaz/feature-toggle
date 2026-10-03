-- Phase 2 final review: indexes the outbox was missing.
-- Deleting a feature sets jira_outbound_jobs.feature_id to NULL (ON DELETE SET NULL):
-- without an index on feature_id each delete scans the whole table.
CREATE INDEX jira_outbound_jobs_feature_id_idx ON jira_outbound_jobs (feature_id);
-- claim_due holds back the later pending jobs of an issue: it looks up pending jobs
-- by integration and issue, oldest first.
CREATE INDEX jira_outbound_jobs_pending_issue_idx
  ON jira_outbound_jobs (integration_id, issue_key, created_at)
  WHERE status = 'pending';
