-- Add temporary password flag to users table
ALTER TABLE users ADD COLUMN is_temporary_password BOOLEAN NOT NULL DEFAULT FALSE;
