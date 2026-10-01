-- Key rotation for JWT signing secrets. Tokens carry the signing secret's id as
-- their `kid` header. A secret deactivated by rotation keeps verifying the tokens
-- it signed for one access-token lifetime after `deactivated_at`. Emergency
-- deactivation (POST /api/v1/auth/jwt-secrets/deactivate-all) sets `revoked_at`,
-- which ends that grace immediately. Secrets deactivated before this migration
-- have neither value and verify nothing.
ALTER TABLE jwt_secrets ADD COLUMN IF NOT EXISTS deactivated_at TIMESTAMPTZ NULL;
ALTER TABLE jwt_secrets ADD COLUMN IF NOT EXISTS revoked_at TIMESTAMPTZ NULL;
