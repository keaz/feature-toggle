-- AI-11: approvals needed for one request when AI risk enforcement raises the
-- policy's required_approvers (docs/ai-judgments/design.md §4.3, §5.1).
-- NULL means the policy's required_approvers applies.
ALTER TABLE approval_requests
  ADD COLUMN required_approvers_override INT NULL;
