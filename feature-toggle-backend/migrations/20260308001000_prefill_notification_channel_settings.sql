UPDATE notification_channel_configs
SET settings = jsonb_build_object(
    'host', 'smtp.gmail.com',
    'port', 587,
    'secure', false,
    'startTls', true,
    'username', 'no-reply@yourdomain.com',
    'password', 'replace-with-app-password',
    'fromEmail', 'no-reply@yourdomain.com',
    'fromName', 'FluxGate Notifications'
),
    updated_at = NOW()
WHERE channel = 'email'
  AND (settings IS NULL OR settings = '{}'::jsonb);

UPDATE notification_channel_configs
SET settings = jsonb_build_object(
    'providerAccountSid', 'ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx',
    'providerAuthToken', 'replace-with-auth-token',
    'fromNumber', '+15551234567'
),
    updated_at = NOW()
WHERE channel = 'sms'
  AND (settings IS NULL OR settings = '{}'::jsonb);
