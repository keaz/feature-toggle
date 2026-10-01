//! Transactional session operations: refresh-token rotation.

use crate::Error;
use crate::config::AuthConfig;
use crate::database::jwt_token::store_token_tx;
use crate::database::refresh_token::{
    RefreshToken, create_token_tx, family_has_active_token_tx, find_by_hash_for_update_tx,
    mark_rotated_tx, revoke_family_tx,
};
use crate::database::role::RoleRepositoryTx;
use crate::database::user::UserRepositoryTx;
use crate::logic::jwt_secret::SigningKey;
use crate::logic::jwt_token::{LoginResult, generate_refresh_token, issue_access_token};
use crate::logic::user::ApiUser;
use chrono::{Duration, Utc};
use sqlx::PgConnection;

/// How long after a rotation the rotated token is still accepted, so that
/// concurrent refreshes (several tabs waking up together) do not count as
/// reuse. Each such late refresh gets its own new token in the same family.
pub const REFRESH_REUSE_GRACE_SECONDS: i64 = 10;

/// Why a refresh token was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRejection {
    /// Unknown or expired token, or its user is missing or disabled.
    Invalid,
    /// The token had already been revoked; its whole family is now revoked.
    Reused,
}

/// Result of a refresh attempt. Both variants must be committed: a rejection
/// for reuse carries the revocation of the token family.
#[derive(Debug)]
pub enum RefreshOutcome {
    Rotated(LoginResult),
    Rejected(RefreshRejection),
}

/// Rotates `refresh_token` inside the caller's transaction.
///
/// The presented token's row is locked (`SELECT ... FOR UPDATE`), so concurrent
/// refreshes with the same token are serialized: the first rotates it, and a
/// later one sees it revoked. A revoked token is accepted only within the reuse
/// grace window (see [`within_reuse_grace`]); otherwise it counts as reuse and
/// the whole family is revoked.
pub async fn refresh_session_in_tx<U, R>(
    conn: &mut PgConnection,
    user_repo: &U,
    role_repo: &R,
    signing_key: &SigningKey,
    auth: &AuthConfig,
    refresh_token: &str,
) -> Result<RefreshOutcome, Error>
where
    U: UserRepositoryTx,
    R: RoleRepositoryTx,
{
    let token_hash = crate::middleware::jwt_guard::hash_token(refresh_token);
    let Some(stored) = find_by_hash_for_update_tx(conn, &token_hash).await? else {
        return Ok(RefreshOutcome::Rejected(RefreshRejection::Invalid));
    };

    // Checked before revocation: disabling a user also revokes their refresh
    // tokens, and that must read as an invalid token, not as reuse.
    let user = match user_repo.get_user_by_id_tx(conn, stored.user_id).await {
        Ok(user) => user,
        Err(Error::NotFound(_)) => return Ok(RefreshOutcome::Rejected(RefreshRejection::Invalid)),
        Err(err) => return Err(err),
    };
    if !user.enabled {
        return Ok(RefreshOutcome::Rejected(RefreshRejection::Invalid));
    }

    let in_grace = stored.revoked_at.is_some() && within_reuse_grace(conn, &stored).await?;
    if stored.revoked_at.is_some() && !in_grace {
        let revoked = revoke_family_tx(conn, stored.family_id).await?;
        log::warn!(
            "Refresh token reuse detected for user {}; revoked {} token(s) of family {}",
            stored.user_id,
            revoked,
            stored.family_id
        );
        return Ok(RefreshOutcome::Rejected(RefreshRejection::Reused));
    }

    if stored.expires_at <= Utc::now() {
        return Ok(RefreshOutcome::Rejected(RefreshRejection::Invalid));
    }

    // Roles and admin flag come from the database, not from the old session.
    let role_names: Vec<String> = role_repo
        .get_user_roles_tx(conn, user.id)
        .await?
        .into_iter()
        .map(|role| role.name)
        .collect();

    let (new_refresh_token, new_refresh_hash) = generate_refresh_token();
    let child = create_token_tx(
        conn,
        user.id,
        stored.family_id,
        new_refresh_hash,
        Utc::now() + auth.refresh_token_ttl(),
    )
    .await?;
    // A token accepted within the grace window stays revoked and keeps pointing
    // at the token it was first rotated into.
    if !in_grace && !mark_rotated_tx(conn, stored.id, child.id).await? {
        // Unreachable while the row lock is held; fail closed regardless.
        revoke_family_tx(conn, stored.family_id).await?;
        return Ok(RefreshOutcome::Rejected(RefreshRejection::Reused));
    }

    let access = issue_access_token(
        user.id,
        &user.username,
        user.is_admin,
        role_names,
        signing_key,
        auth,
    )?;
    store_token_tx(conn, user.id, access.token_hash, access.expires_at).await?;

    let is_temporary = user.is_temporary_password;
    Ok(RefreshOutcome::Rotated(LoginResult {
        user: ApiUser::from(user),
        token: access.token,
        is_temporary,
        refresh_token: new_refresh_token,
        expires_in: auth.access_token_ttl().num_seconds(),
    }))
}

/// A revoked token is a benign concurrent refresh only if it was revoked by
/// rotation (it has `replaced_by`), less than [`REFRESH_REUSE_GRACE_SECONDS`]
/// ago, and its family has not since been ended (logout, disable, password
/// reset or reuse detection revoke every active token of the family).
async fn within_reuse_grace(
    conn: &mut PgConnection,
    stored: &RefreshToken,
) -> Result<bool, crate::Error> {
    let (Some(revoked_at), Some(_)) = (stored.revoked_at, stored.replaced_by) else {
        return Ok(false);
    };
    if Utc::now() - revoked_at > Duration::seconds(REFRESH_REUSE_GRACE_SECONDS) {
        return Ok(false);
    }
    family_has_active_token_tx(conn, stored.family_id).await
}
