//! Transactional JWT secret operations: emergency deactivation.

use crate::Error;
use crate::database::jwt_secret::revoke_all_secrets_tx;
use crate::database::jwt_token::revoke_all_tokens_tx;
use crate::database::refresh_token::revoke_all_refresh_tokens_tx;
use sqlx::PgConnection;

/// Counts of what an emergency deactivation revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeactivateAllOutcome {
    pub secrets: u64,
    pub access_tokens: u64,
    pub refresh_tokens: u64,
}

/// Invalidates every session immediately, inside the caller's transaction:
/// every signing secret is revoked (no rotation grace, so no token signed by any
/// of them verifies, including system-client tokens), and every access token
/// and refresh token row is revoked.
pub async fn deactivate_all_secrets_in_tx(
    conn: &mut PgConnection,
) -> Result<DeactivateAllOutcome, Error> {
    let secrets = revoke_all_secrets_tx(conn).await?;
    let access_tokens = revoke_all_tokens_tx(conn).await?;
    let refresh_tokens = revoke_all_refresh_tokens_tx(conn).await?;
    log::warn!(
        "Deactivated all JWT secrets: revoked {} secret(s), {} access token(s), {} refresh token(s)",
        secrets,
        access_tokens,
        refresh_tokens
    );
    Ok(DeactivateAllOutcome {
        secrets,
        access_tokens,
        refresh_tokens,
    })
}
