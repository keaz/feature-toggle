//! Role, team and admin synchronisation from IdP group claims.
//!
//! Called inside the SSO callback transaction after the user is resolved. This is a
//! no-op for now; group mapping sync fills it in.

use crate::Error;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::entity::SsoProvider;
use sqlx::PgConnection;
use uuid::Uuid;

/// Applies the provider's group mappings to the user from the validated claims
/// (`claims` holds every id_token claim, merged with userinfo). Currently a no-op.
pub async fn sync_roles_from_claims(
    _conn: &mut PgConnection,
    _activity_repo: &dyn ActivityLogRepository,
    _provider: &SsoProvider,
    _user_id: Uuid,
    _claims: &serde_json::Value,
) -> Result<(), Error> {
    Ok(())
}
