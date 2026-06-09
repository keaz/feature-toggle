ALTER TABLE approval_policies
    ADD COLUMN IF NOT EXISTS approver_user_ids UUID[] NOT NULL DEFAULT '{}',
    ADD COLUMN IF NOT EXISTS allow_admin_override BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS fallback_to_roles BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE approval_requests
    ADD COLUMN IF NOT EXISTS eligible_approver_ids UUID[] NOT NULL DEFAULT '{}',
    ADD COLUMN IF NOT EXISTS routing_reason TEXT,
    ADD COLUMN IF NOT EXISTS admin_override_enabled BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_approval_requests_eligible_approver_ids
    ON approval_requests USING GIN (eligible_approver_ids);
