-- JI-51 (docs/jira-integration/phase-2/design.md §3.8, decision J25):
-- a judgment can be skipped before it finishes, for example the risk
-- assessment of an approval request that Jira approved first. A skipped row
-- is final: it is never started, finished, failed or retried.
ALTER TABLE ai_judgments DROP CONSTRAINT ai_judgments_status_check;
ALTER TABLE ai_judgments ADD CONSTRAINT ai_judgments_status_check
    CHECK (status IN ('pending', 'done', 'failed', 'skipped'));
