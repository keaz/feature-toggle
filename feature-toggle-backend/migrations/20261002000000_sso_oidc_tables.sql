-- OIDC single sign-on: provider configuration, linked identities, group
-- mappings, short-lived login state, one-time exchange codes and global settings.
CREATE TABLE sso_providers (
    id UUID PRIMARY KEY,
    slug VARCHAR(50) NOT NULL UNIQUE,
    display_name VARCHAR(100) NOT NULL,
    issuer_url TEXT NOT NULL,
    client_id TEXT NOT NULL,
    -- AES-256-GCM, stored as base64(nonce || ciphertext). NULL when no secret is
    -- stored in the database (public client or secret supplied by environment).
    client_secret_enc TEXT NULL,
    scopes TEXT[] NOT NULL DEFAULT '{openid,email,profile}',
    groups_claim TEXT NOT NULL DEFAULT 'groups',
    allowed_email_domains TEXT[] NOT NULL DEFAULT '{}',
    jit_provisioning BOOLEAN NOT NULL DEFAULT TRUE,
    allow_email_linking BOOLEAN NOT NULL DEFAULT FALSE,
    role_sync_mode VARCHAR(20) NOT NULL DEFAULT 'authoritative'
        CHECK (role_sync_mode IN ('authoritative', 'additive', 'off')),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE user_identities (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider_id UUID NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    email TEXT NULL,
    last_login TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider_id, subject)
);

CREATE INDEX idx_user_identities_user_id ON user_identities(user_id);

CREATE TABLE sso_group_mappings (
    id UUID PRIMARY KEY,
    provider_id UUID NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
    group_value TEXT NOT NULL,
    target_type VARCHAR(10) NOT NULL CHECK (target_type IN ('role', 'team', 'admin')),
    target_id UUID NULL,
    CHECK ((target_type = 'admin') = (target_id IS NULL)),
    UNIQUE (provider_id, group_value, target_type, target_id)
);

CREATE INDEX idx_sso_group_mappings_provider_id ON sso_group_mappings(provider_id);

-- UNIQUE treats NULL target_id values as distinct, so it cannot stop duplicate
-- 'admin' mappings for the same group. This partial index does.
CREATE UNIQUE INDEX uq_sso_group_mappings_admin
    ON sso_group_mappings(provider_id, group_value, target_type)
    WHERE target_id IS NULL;

-- Authorization requests in flight. The state is stored as a SHA-256 hash; the
-- nonce and PKCE verifier are needed in clear for the callback and live at most
-- 10 minutes. A row is single use: it is deleted when the callback consumes it.
CREATE TABLE sso_login_states (
    id UUID PRIMARY KEY,
    state_hash TEXT NOT NULL UNIQUE,
    provider_id UUID NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    redirect_path TEXT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_sso_login_states_expires_at ON sso_login_states(expires_at);

-- One-time codes handed to the UI after a successful callback. Only the SHA-256
-- hash is stored; tokens are issued at exchange and never stored here.
CREATE TABLE sso_login_codes (
    id UUID PRIMARY KEY,
    code_hash TEXT NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider_id UUID NOT NULL REFERENCES sso_providers(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_sso_login_codes_expires_at ON sso_login_codes(expires_at);

-- Singleton settings row (the PRIMARY KEY CHECK keeps it to one row).
CREATE TABLE sso_settings (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    enforce_sso BOOLEAN NOT NULL DEFAULT FALSE
);

INSERT INTO sso_settings (id, enforce_sso) VALUES (TRUE, FALSE);
