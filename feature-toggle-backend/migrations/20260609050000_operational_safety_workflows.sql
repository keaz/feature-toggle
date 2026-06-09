CREATE TABLE IF NOT EXISTS change_freeze_windows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    environment_id UUID REFERENCES environments(id) ON DELETE CASCADE,
    environment_type TEXT,
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    recurrence TEXT NOT NULL DEFAULT 'NONE' CHECK (recurrence IN ('NONE', 'DAILY', 'WEEKLY')),
    reason TEXT,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (ends_at > starts_at),
    CHECK (environment_id IS NOT NULL OR environment_type IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_change_freeze_windows_team_active
    ON change_freeze_windows(team_id, active);

CREATE INDEX IF NOT EXISTS idx_change_freeze_windows_environment
    ON change_freeze_windows(environment_id)
    WHERE environment_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS scheduled_feature_changes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    feature_id UUID NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    stage_id UUID REFERENCES features_pipeline_stages(id) ON DELETE CASCADE,
    environment_id UUID REFERENCES environments(id) ON DELETE SET NULL,
    action TEXT NOT NULL CHECK (action IN ('ENABLE_FEATURE', 'DISABLE_FEATURE', 'STAGE_CHANGE', 'ARCHIVE_FEATURE')),
    requested_status TEXT,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    reason TEXT NOT NULL,
    scheduled_at TIMESTAMPTZ NOT NULL,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    status TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING', 'EXECUTING', 'EXECUTED', 'CANCELLED', 'FAILED', 'BLOCKED')),
    requested_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    executed_at TIMESTAMPTZ,
    cancelled_at TIMESTAMPTZ,
    result_message TEXT,
    failure_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_scheduled_feature_changes_due
    ON scheduled_feature_changes(status, scheduled_at)
    WHERE status = 'PENDING';

CREATE INDEX IF NOT EXISTS idx_scheduled_feature_changes_feature
    ON scheduled_feature_changes(feature_id, status, scheduled_at);
