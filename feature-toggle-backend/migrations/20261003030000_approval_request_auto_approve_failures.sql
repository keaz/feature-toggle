-- Bounds auto-approval retries (known-issue fix round 2026-10-03). Counts
-- failed attempts to auto-approve a due request whose change cannot be
-- applied. After 3 failures the request is no longer due for auto-approval
-- and an activity entry asks a person to decide (admin override or cancel).
ALTER TABLE approval_requests
    ADD COLUMN auto_approve_failures INT NOT NULL DEFAULT 0;
