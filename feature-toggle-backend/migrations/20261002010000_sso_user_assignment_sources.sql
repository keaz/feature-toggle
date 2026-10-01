-- Users created through SSO have no password. auth_source records how the account
-- is managed: 'local' (password), 'sso' (JIT provisioned) or 'system' (system
-- client shadow user).
ALTER TABLE users ALTER COLUMN password_hash DROP NOT NULL;

ALTER TABLE users
    ADD COLUMN auth_source VARCHAR(20) NOT NULL DEFAULT 'local'
        CHECK (auth_source IN ('local', 'sso', 'system'));

UPDATE users SET auth_source = 'system' WHERE id IN (SELECT id FROM system_clients);

-- Role and team assignments get a source so SSO group sync and manual edits do
-- not overwrite each other. Existing rows are manual.
--
-- user_roles is UNIQUE (user_id, role_id) and user_teams has PRIMARY KEY
-- (user_id, team_id), so an assignment is a single row. When sync wants an
-- assignment the user already has manually, the row stays 'manual' (manual
-- wins). In the other direction, manual "replace the whole set" edits only delete
-- and insert 'manual' rows; an assignment currently held through SSO is left as
-- an 'sso' row.
ALTER TABLE user_roles
    ADD COLUMN source VARCHAR(10) NOT NULL DEFAULT 'manual'
        CHECK (source IN ('manual', 'sso'));

ALTER TABLE user_teams
    ADD COLUMN source VARCHAR(10) NOT NULL DEFAULT 'manual'
        CHECK (source IN ('manual', 'sso'));
