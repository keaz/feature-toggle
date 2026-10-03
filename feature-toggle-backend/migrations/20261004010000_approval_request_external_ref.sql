-- JI-11: who asked for a stage change (a ticket or change id, e.g. a Jira
-- issue key) and why. Both optional; validated in the REST layer.
ALTER TABLE approval_requests
    ADD COLUMN external_ref TEXT NULL,
    ADD COLUMN request_reason TEXT NULL;
