-- Add enabled flag to users with default true
ALTER TABLE IF EXISTS users
    ADD COLUMN IF NOT EXISTS enabled BOOLEAN NOT NULL DEFAULT TRUE;
