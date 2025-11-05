-- Add feature variants support
-- This migration adds support for feature variants with JSONB values and type system

-- Create enum for variant value types
CREATE TYPE variant_value_type AS ENUM (
    'string',
    'number',
    'boolean',
    'json'
);

-- Create feature_variants table
CREATE TABLE feature_variants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    control VARCHAR(100) NOT NULL,  -- Variant key identifier (e.g., "control", "variant-a")
    value JSONB NOT NULL,  -- The actual value to return when this variant is selected
    value_type variant_value_type NOT NULL DEFAULT 'boolean',  -- Type hint for deserialization
    description TEXT,  -- Optional description of the variant
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT unique_variant_per_feature UNIQUE(feature_id, control)
);

-- Create indexes for faster lookups
CREATE INDEX idx_feature_variants_feature_id ON feature_variants(feature_id);
CREATE INDEX idx_feature_variants_control ON feature_variants(control);

-- Add serve column to feature_stage_criteria to specify which variant to serve
ALTER TABLE feature_stage_criteria
ADD COLUMN serve VARCHAR(100);

-- Create index for serve column for faster lookups
CREATE INDEX idx_feature_stage_criteria_serve ON feature_stage_criteria(serve);

-- Add comments for documentation
COMMENT ON TABLE feature_variants IS 'Feature variants with JSONB values for flexible typing. A feature can have multiple variants, each with its own value.';
COMMENT ON COLUMN feature_variants.control IS 'Variant identifier (e.g., "control", "variant-a", "variant-b")';
COMMENT ON COLUMN feature_variants.value IS 'JSONB value returned when variant is served. Can be any JSON type: string, number, boolean, object, array.';
COMMENT ON COLUMN feature_variants.value_type IS 'Type hint for deserialization: string, number, boolean, or json';
COMMENT ON COLUMN feature_stage_criteria.serve IS 'Variant control to serve when this criterion matches. References feature_variants.control. NULL means default boolean behavior.';
