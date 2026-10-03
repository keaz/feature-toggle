-- Jira write-back configuration (JI-41). The credential and the native webhook
-- secret are sealed with secret_box (AES-256-GCM, AAD = integration id).
ALTER TABLE jira_integrations
    ADD COLUMN writeback_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN writeback_comments BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN writeback_remote_link BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN jira_auth_kind VARCHAR(20) NULL
        CHECK (jira_auth_kind IN ('cloud_basic', 'dc_pat')),
    ADD COLUMN jira_account_email TEXT NULL,
    ADD COLUMN jira_credential_enc TEXT NULL,
    ADD COLUMN writeback_paused_reason TEXT NULL,
    ADD COLUMN native_webhook_secret_enc TEXT NULL;
