-- Device-code login for the fluxgate CLI (fluxgate login --use-device-code).
-- The CLI polls with a device code (only its SHA-256 hash is stored); a
-- signed-in user approves the short user code in the web UI.
CREATE TABLE cli_device_authorizations (
    id UUID PRIMARY KEY,
    device_code_hash TEXT NOT NULL UNIQUE,
    user_code TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied', 'consumed')),
    user_id UUID NULL REFERENCES users(id) ON DELETE CASCADE,
    interval_secs INTEGER NOT NULL,
    last_polled_at TIMESTAMPTZ NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_cli_device_authorizations_expires_at ON cli_device_authorizations(expires_at);
