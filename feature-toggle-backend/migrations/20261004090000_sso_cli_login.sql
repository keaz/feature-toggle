-- SSO login for the fluxgate CLI: the callback can hand the one-time code to a
-- loopback address, and such codes are bound to a PKCE challenge that the CLI
-- must answer at exchange.
ALTER TABLE sso_login_states
    ADD COLUMN cli_redirect_uri TEXT NULL,
    ADD COLUMN cli_code_challenge TEXT NULL;

ALTER TABLE sso_login_codes
    ADD COLUMN code_challenge TEXT NULL;
