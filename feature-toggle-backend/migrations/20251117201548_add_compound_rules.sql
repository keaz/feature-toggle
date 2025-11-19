-- Add compound rule support for advanced AND/OR logic
-- This allows criteria to have multiple conditions combined with boolean operators

-- Rule groups table to hold AND/OR logic operators
CREATE TABLE IF NOT EXISTS rule_groups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    criteria_id UUID NOT NULL REFERENCES feature_stage_criteria(id) ON DELETE CASCADE,
    logic_operator VARCHAR(10) NOT NULL CHECK (logic_operator IN ('AND', 'OR')),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

-- Individual conditions within a rule group
CREATE TABLE IF NOT EXISTS rule_conditions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    group_id UUID NOT NULL REFERENCES rule_groups(id) ON DELETE CASCADE,
    context_key VARCHAR(100) NOT NULL,
    operator VARCHAR(50) NOT NULL,
    value JSONB NOT NULL,
    order_index INT NOT NULL DEFAULT 0,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT valid_operator CHECK (operator IN (
        'EQUALS', 'NOT_EQUALS',
        'GREATER_THAN', 'LESS_THAN', 'GREATER_THAN_OR_EQUAL', 'LESS_THAN_OR_EQUAL',
        'CONTAINS', 'STARTS_WITH', 'ENDS_WITH', 'REGEX',
        'IN', 'NOT_IN',
        'SEMVER_GREATER_THAN', 'SEMVER_LESS_THAN'
    ))
);

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_rule_groups_criteria ON rule_groups(criteria_id);
CREATE INDEX IF NOT EXISTS idx_rule_conditions_group ON rule_conditions(group_id, order_index);
CREATE INDEX IF NOT EXISTS idx_rule_conditions_context_key ON rule_conditions(context_key);

-- Comments for documentation
COMMENT ON TABLE rule_groups IS 'Groups of conditions combined with AND/OR logic operators';
COMMENT ON COLUMN rule_groups.logic_operator IS 'Boolean operator: AND (all conditions must match) or OR (at least one must match)';
COMMENT ON TABLE rule_conditions IS 'Individual conditions within a rule group';
COMMENT ON COLUMN rule_conditions.value IS 'JSONB value to compare against (supports arrays for IN/NOT_IN operators)';
COMMENT ON COLUMN rule_conditions.order_index IS 'Display order of conditions within a group (lower values first)';
