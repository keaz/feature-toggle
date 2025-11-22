-- Add lifecycle tracking fields to features table

ALTER TABLE features
ADD COLUMN lifecycle_stage VARCHAR(50) NOT NULL DEFAULT 'active' CHECK (lifecycle_stage IN ('active', 'deprecated', 'archived', 'permanent'));

ALTER TABLE features
ADD COLUMN deprecated_at TIMESTAMPTZ NULL;

ALTER TABLE features
ADD COLUMN deprecation_notice TEXT NULL;

ALTER TABLE features
ADD COLUMN last_evaluated_at TIMESTAMPTZ NULL;

ALTER TABLE features
ADD COLUMN evaluation_count_7d BIGINT NOT NULL DEFAULT 0;

ALTER TABLE features
ADD COLUMN evaluation_count_30d BIGINT NOT NULL DEFAULT 0;

ALTER TABLE features
ADD COLUMN evaluation_count_90d BIGINT NOT NULL DEFAULT 0;

CREATE INDEX idx_features_lifecycle ON features (lifecycle_stage, last_evaluated_at);
