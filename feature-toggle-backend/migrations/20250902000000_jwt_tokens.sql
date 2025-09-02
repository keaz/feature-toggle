-- Create JWT tokens table for token revocation
CREATE TABLE jwt_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(255) NOT NULL UNIQUE, -- Hash of the actual token for security
    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TIMESTAMP WITH TIME ZONE DEFAULT NULL,
    is_revoked BOOLEAN NOT NULL DEFAULT FALSE
);

-- Index for efficient lookups
CREATE INDEX idx_jwt_tokens_token_hash ON jwt_tokens(token_hash);
CREATE INDEX idx_jwt_tokens_user_id ON jwt_tokens(user_id);
CREATE INDEX idx_jwt_tokens_expires_at ON jwt_tokens(expires_at);
CREATE INDEX idx_jwt_tokens_is_revoked ON jwt_tokens(is_revoked);

-- Index for cleanup of expired tokens
CREATE INDEX idx_jwt_tokens_cleanup ON jwt_tokens(expires_at, is_revoked);
