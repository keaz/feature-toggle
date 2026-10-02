-- AI-10: per-policy mode for AI approval risk assessment (docs/ai-judgments/design.md §4.3).
-- Every mode except 'off' behaves like 'advisory' until AI-11 adds enforcement.
ALTER TABLE approval_policies
  ADD COLUMN ai_risk_mode TEXT NOT NULL DEFAULT 'advisory'
  CHECK (ai_risk_mode IN ('off', 'advisory', 'gate_auto_approve', 'require_extra_approver'));
