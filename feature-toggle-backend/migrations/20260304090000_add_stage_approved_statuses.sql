-- Expand stage status workflow to include approved states for deployment and rollback
ALTER TABLE IF EXISTS features_pipeline_stages
DROP CONSTRAINT IF EXISTS features_pipeline_stages_status_check;

ALTER TABLE IF EXISTS features_pipeline_stages
ADD CONSTRAINT features_pipeline_stages_status_check CHECK (status IN (
    'NOT_DEPLOYED',
    'DEPLOYMENT_REQUESTED',
    'DEPLOYMENT_APPROVED',
    'DEPLOYMENT_REJECTED',
    'DEPLOYED',
    'ROLLBACK_REQUESTED',
    'ROLLBACK_APPROVED',
    'ROLLBACK_REJECTED',
    'ROLLBACKED'
));
