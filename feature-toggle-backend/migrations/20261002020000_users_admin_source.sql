-- Records how a user got is_admin so SSO group sync only revokes admin it granted.
-- 'manual' (granted through the users API, bootstrap or seed) is never revoked by
-- sync; 'sso' was granted by an admin group mapping. NULL when not an admin. A
-- NULL on an admin row is treated as manual by sync.
ALTER TABLE users
    ADD COLUMN admin_source VARCHAR(10) NULL
        CHECK (admin_source IN ('manual', 'sso'));

UPDATE users SET admin_source = 'manual' WHERE is_admin = TRUE;
