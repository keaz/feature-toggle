-- JI-14: who closed an approval request. 'fluxgate' = votes in FluxGate,
-- 'auto' = the auto-approval job, 'jira' = a Jira status rule in an
-- environment that trusts Jira (no vote row). `external_approver` holds the
-- external actor for 'jira' (account id, display name, issue key, status).
ALTER TABLE approval_requests
    ADD COLUMN approval_source VARCHAR(20) NOT NULL DEFAULT 'fluxgate'
        CHECK (approval_source IN ('fluxgate', 'jira', 'auto')),
    ADD COLUMN external_approver JSONB NULL;

UPDATE approval_requests SET approval_source = 'auto' WHERE status = 'auto_approved';
