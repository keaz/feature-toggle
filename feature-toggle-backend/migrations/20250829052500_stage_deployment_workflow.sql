-- Add deployment workflow fields to features_pipeline_stages
-- 1) Add status column with restricted values and default
ALTER TABLE IF EXISTS features_pipeline_stages
ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'NOT_DEPLOYED' CHECK (status IN (
    'NOT_DEPLOYED', 'DEPLOYMENT_REQUESTED', 'DEPLOYMENT_REJECTED', 'DEPLOYED', 'ROLLBACK_REQUESTED', 'ROLLBACK_REJECTED','ROLLBACKED'
));

-- 2) Add request/approval audit columns
ALTER TABLE IF EXISTS features_pipeline_stages
ADD COLUMN IF NOT EXISTS requested_user UUID REFERENCES users(id),
ADD COLUMN IF NOT EXISTS requested_time TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS approved_user UUID REFERENCES users(id),
ADD COLUMN IF NOT EXISTS approved_time TIMESTAMPTZ;
