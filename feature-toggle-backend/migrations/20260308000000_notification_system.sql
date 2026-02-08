ALTER TABLE users
    ADD COLUMN IF NOT EXISTS mobile_number VARCHAR(32);

CREATE TABLE IF NOT EXISTS notification_channel_configs (
    channel VARCHAR(16) PRIMARY KEY,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    provider VARCHAR(64) NOT NULL,
    settings JSONB NOT NULL DEFAULT '{}'::jsonb,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT notification_channel_configs_channel_check
        CHECK (channel IN ('email', 'sms'))
);

CREATE TABLE IF NOT EXISTS notification_preferences (
    notification_type VARCHAR(64) PRIMARY KEY,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    email_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    sms_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS notification_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    notification_type VARCHAR(64) NOT NULL,
    channel VARCHAR(16) NOT NULL,
    team_id UUID REFERENCES teams(id) ON DELETE SET NULL,
    recipient_user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    recipient_email VARCHAR(255),
    recipient_mobile VARCHAR(32),
    subject TEXT NOT NULL,
    message TEXT NOT NULL,
    status VARCHAR(32) NOT NULL,
    failure_reason TEXT,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sent_at TIMESTAMPTZ,
    CONSTRAINT notification_deliveries_channel_check
        CHECK (channel IN ('email', 'sms'))
);

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_type
    ON notification_deliveries(notification_type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_notification_deliveries_team
    ON notification_deliveries(team_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_notification_deliveries_recipient
    ON notification_deliveries(recipient_user_id, created_at DESC);

INSERT INTO notification_channel_configs (channel, enabled, provider, settings)
VALUES
    (
        'email',
        FALSE,
        'smtp',
        jsonb_build_object(
            'host', 'smtp.gmail.com',
            'port', 587,
            'secure', false,
            'startTls', true,
            'username', 'no-reply@yourdomain.com',
            'password', 'replace-with-app-password',
            'fromEmail', 'no-reply@yourdomain.com',
            'fromName', 'FluxGate Notifications'
        )
    ),
    (
        'sms',
        FALSE,
        'twilio',
        jsonb_build_object(
            'providerAccountSid', 'ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx',
            'providerAuthToken', 'replace-with-auth-token',
            'fromNumber', '+15551234567'
        )
    )
ON CONFLICT (channel) DO NOTHING;

INSERT INTO notification_preferences (notification_type, enabled, email_enabled, sms_enabled)
VALUES
    ('feature_created', TRUE, TRUE, FALSE),
    ('stage_change_requested', TRUE, TRUE, FALSE),
    ('stage_change_approved', TRUE, TRUE, FALSE),
    ('feature_deployed', TRUE, TRUE, FALSE),
    ('team_created', TRUE, TRUE, FALSE),
    ('user_added_to_team', TRUE, TRUE, FALSE),
    ('kill_switch_activated', TRUE, TRUE, FALSE)
ON CONFLICT (notification_type) DO NOTHING;
