-- Approval workflow schema for multi-stage approvals
CREATE TABLE IF NOT EXISTS approval_policies (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    applies_to VARCHAR(50) NOT NULL CHECK (applies_to IN ('all', 'production_only', 'specific_environments')),
    environment_ids UUID[],
    required_approvers INT NOT NULL DEFAULT 1,
    approver_role_ids UUID[] NOT NULL,
    auto_approve_after_hours INT,
    enabled BOOLEAN DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS approval_requests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    policy_id UUID NOT NULL REFERENCES approval_policies(id),
    feature_id UUID NOT NULL REFERENCES features(id),
    environment_id UUID,
    change_type VARCHAR(50) NOT NULL,
    change_payload JSONB NOT NULL,
    change_description TEXT,
    requested_by UUID NOT NULL REFERENCES users(id),
    status VARCHAR(50) NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected', 'cancelled', 'auto_approved')),
    approved_count INT DEFAULT 0,
    rejected_count INT DEFAULT 0,
    executed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_approval_requests_pending
    ON approval_requests(status, created_at)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS approval_votes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id UUID NOT NULL REFERENCES approval_requests(id) ON DELETE CASCADE,
    approver_id UUID NOT NULL REFERENCES users(id),
    vote VARCHAR(50) NOT NULL CHECK (vote IN ('approve', 'reject')),
    comment TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(request_id, approver_id)
);

CREATE INDEX IF NOT EXISTS idx_approval_votes_request ON approval_votes(request_id);
