-- Bounds the approval reconciliation (known-issue fix round 2026-10-03).
-- Counts failed attempts to approve a request whose AI-11 override can no
-- longer be reached. After 3 failures the reconciliation stops retrying it and
-- records an activity entry, so a person decides (admin override or cancel).
ALTER TABLE approval_requests
    ADD COLUMN reconcile_failures INT NOT NULL DEFAULT 0;
