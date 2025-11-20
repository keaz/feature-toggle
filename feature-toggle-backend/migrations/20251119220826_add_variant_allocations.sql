-- Add variant allocations for multi-variant weighted traffic splits
-- This allows distributing traffic across multiple variants with percentage weights
-- Example: Variant A: 25%, B: 25%, C: 50%

-- Variant allocations table to store weight distributions
CREATE TABLE IF NOT EXISTS variant_allocations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    criteria_id UUID NOT NULL REFERENCES feature_stage_criteria(id) ON DELETE CASCADE,
    variant_control VARCHAR(100) NOT NULL,
    weight INT NOT NULL CHECK (weight >= 0 AND weight <= 100),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,

    -- Ensure unique variant per criteria
    UNIQUE(criteria_id, variant_control)
);

-- Index for performance when loading allocations for criteria
CREATE INDEX IF NOT EXISTS idx_variant_allocations_criteria ON variant_allocations(criteria_id);

-- Trigger function to validate that total weights don't exceed 100%
CREATE OR REPLACE FUNCTION check_allocation_weights()
RETURNS TRIGGER AS $$
DECLARE
    total_weight INT;
BEGIN
    -- Calculate total weight for this criteria
    SELECT COALESCE(SUM(weight), 0) INTO total_weight
    FROM variant_allocations
    WHERE criteria_id = NEW.criteria_id;

    -- Check if total exceeds 100
    IF total_weight > 100 THEN
        RAISE EXCEPTION 'Total weight for criteria % exceeds 100 (current total: %)', NEW.criteria_id, total_weight;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply trigger to validate weights on INSERT and UPDATE
CREATE TRIGGER validate_allocation_weights
AFTER INSERT OR UPDATE ON variant_allocations
FOR EACH ROW EXECUTE FUNCTION check_allocation_weights();

-- Comments for documentation
COMMENT ON TABLE variant_allocations IS 'Stores weight distributions for multi-variant traffic splits';
COMMENT ON COLUMN variant_allocations.criteria_id IS 'References the stage criterion this allocation belongs to';
COMMENT ON COLUMN variant_allocations.variant_control IS 'The variant control name (must match a variant in the feature)';
COMMENT ON COLUMN variant_allocations.weight IS 'Weight percentage (0-100) for this variant. Total across all variants should equal 100';
