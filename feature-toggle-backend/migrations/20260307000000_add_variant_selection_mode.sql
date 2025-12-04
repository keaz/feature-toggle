-- Add variant selection mode to feature_stage_criteria
-- This allows choosing between weighted traffic split or specific variant selection

-- Create enum for variant selection mode
CREATE TYPE variant_selection_mode AS ENUM (
    'WEIGHTED_SPLIT',    -- Use variant_allocations for weighted distribution (existing behavior)
    'SPECIFIC_VARIANT'   -- Always return a specific variant for all users matching this criterion
);

-- Add columns to feature_stage_criteria
ALTER TABLE feature_stage_criteria
ADD COLUMN variant_selection_mode variant_selection_mode DEFAULT 'WEIGHTED_SPLIT' NOT NULL,
ADD COLUMN selected_variant_control VARCHAR(100);

-- Add index for performance
CREATE INDEX idx_feature_stage_criteria_selection_mode ON feature_stage_criteria(variant_selection_mode);

-- Add check constraint: if mode is SPECIFIC_VARIANT, selected_variant_control must be set
ALTER TABLE feature_stage_criteria
ADD CONSTRAINT check_specific_variant_set
CHECK (
    variant_selection_mode = 'WEIGHTED_SPLIT' OR
    (variant_selection_mode = 'SPECIFIC_VARIANT' AND selected_variant_control IS NOT NULL)
);

-- Add comments for documentation
COMMENT ON COLUMN feature_stage_criteria.variant_selection_mode IS
'Determines how variants are selected: WEIGHTED_SPLIT uses variant_allocations for distribution, SPECIFIC_VARIANT always returns the same variant';

COMMENT ON COLUMN feature_stage_criteria.selected_variant_control IS
'The specific variant control to return when variant_selection_mode is SPECIFIC_VARIANT. Must reference a valid feature_variants.control value.';
