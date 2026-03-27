-- Persist a stable idempotency fingerprint for feature evaluation ingestion.
-- This lets retrying the same logical event remain safe across process restarts
-- and transient response losses.

ALTER TABLE feature_evaluations
    ADD COLUMN IF NOT EXISTS ingest_fingerprint TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_feature_evaluations_ingest_fingerprint
    ON feature_evaluations(ingest_fingerprint);
