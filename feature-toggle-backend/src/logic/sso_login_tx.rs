//! Transactional part of the SSO callback: resolve (or link, or provision) the user
//! for validated IdP claims, record the login, run role sync, write the activity log
//! and create the one-time exchange code, all in the caller's transaction.

use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::SsoProvider;
use crate::database::sso_login_code::{NewSsoLoginCode, SsoLoginCodeRepositoryTx};
use crate::database::user::{CreateSsoUser, User, UserRepositoryTx};
use crate::database::user_identity::{CreateUserIdentity, UserIdentityRepositoryTx};
use crate::logic::oidc_client::{IdTokenClaims, random_token};
use crate::logic::sso_login::{
    LOGIN_CODE_TTL_SECONDS, SsoLoginError, USER_EMAIL_COLUMN_LIMIT, USER_NAME_COLUMN_LIMIT,
    derive_names, email_domain_allowed, truncate_chars, username_base, username_candidate,
};
use crate::logic::sso_role_sync::sync_roles_from_claims;
use crate::utils::activity_logger::{activity_types, entity_types};
use chrono::{Duration, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

/// Highest numeric suffix tried for a unique username before a random one is used.
const MAX_USERNAME_SUFFIX: u32 = 1000;

/// Result of a successful callback. `Debug` redacts the one-time code.
pub struct CompletedSsoLogin {
    pub user_id: Uuid,
    /// The one-time exchange code in clear; only its hash is stored.
    pub code: String,
    pub provisioned: bool,
    pub linked: bool,
}

impl std::fmt::Debug for CompletedSsoLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletedSsoLogin")
            .field("user_id", &self.user_id)
            .field("code", &"<redacted>")
            .field("provisioned", &self.provisioned)
            .field("linked", &self.linked)
            .finish()
    }
}

/// Repositories used by [`complete_sso_login_in_tx`].
pub struct SsoLoginRepos<'a, U, I, C> {
    pub users: &'a U,
    pub identities: &'a I,
    pub codes: &'a C,
    pub activity: &'a dyn ActivityLogRepository,
}

/// Resolves the user for `claims` and issues a one-time code.
///
/// Order: email present, email domain allowed (a non-empty domain list also needs
/// `email_verified: true`), then identity by `(provider, sub)`; else link by email
/// (only with `allowEmailLinking`, `email_verified: true`, and a user that is
/// neither a system admin nor a system-client user); else JIT provisioning (if
/// enabled). A disabled user is rejected after resolution. The user's `last_login`
/// is set later, when the exchange issues a session. Any error leaves the
/// transaction to be rolled back.
pub async fn complete_sso_login_in_tx<U, I, C>(
    conn: &mut PgConnection,
    repos: SsoLoginRepos<'_, U, I, C>,
    provider: &SsoProvider,
    claims: &IdTokenClaims,
) -> Result<CompletedSsoLogin, SsoLoginError>
where
    U: UserRepositoryTx,
    I: UserIdentityRepositoryTx,
    C: SsoLoginCodeRepositoryTx,
{
    let email = claims.email.as_deref().ok_or(SsoLoginError::EmailMissing)?;
    if email.chars().count() > USER_EMAIL_COLUMN_LIMIT || !email.contains('@') {
        return Err(SsoLoginError::ProviderError(
            "email claim is not a usable address".to_string(),
        ));
    }
    // An unverified email proves nothing about its domain: with a domain
    // restriction, only a verified email can satisfy it.
    if !provider.allowed_email_domains.is_empty()
        && (!claims.email_verified || !email_domain_allowed(email, &provider.allowed_email_domains))
    {
        return Err(SsoLoginError::EmailDomainNotAllowed);
    }

    let now = Utc::now();
    let mut provisioned = false;
    let mut linked = false;

    let user = match repos
        .identities
        .find_identity_tx(conn, provider.id, &claims.subject)
        .await?
    {
        Some(identity) => {
            let user = repos
                .users
                .get_user_by_id_tx(conn, identity.user_id)
                .await?;
            repos
                .identities
                .record_login_tx(conn, identity.id, Some(email.to_string()), now)
                .await?;
            user
        }
        None => {
            let user = match repos.users.find_user_by_email_ci_tx(conn, email).await? {
                Some(existing) => {
                    // Never link to a system admin: an IdP account must not be able
                    // to take over the break-glass admin by matching its email.
                    let linkable = provider.allow_email_linking
                        && claims.email_verified
                        && !existing.is_admin
                        && existing.auth_source != "system"
                        && !repos.users.is_system_client_tx(conn, existing.id).await?;
                    if !linkable {
                        return Err(SsoLoginError::LinkingNotAllowed);
                    }
                    linked = true;
                    existing
                }
                None => {
                    if !provider.jit_provisioning {
                        return Err(SsoLoginError::UserNotProvisioned);
                    }
                    provisioned = true;
                    provision_user(conn, repos.users, claims, email).await?
                }
            };
            repos
                .identities
                .create_identity_tx(
                    conn,
                    CreateUserIdentity {
                        user_id: user.id,
                        provider_id: provider.id,
                        subject: claims.subject.clone(),
                        email: Some(email.to_string()),
                        last_login: Some(now),
                    },
                )
                .await?;
            user
        }
    };

    if !user.enabled {
        return Err(SsoLoginError::AccountDisabled);
    }

    sync_roles_from_claims(conn, repos.activity, provider, user.id, &claims.raw).await?;

    let code = random_token();
    repos
        .codes
        .create_code_tx(
            conn,
            NewSsoLoginCode {
                code_hash: crate::middleware::jwt_guard::hash_token(&code),
                user_id: user.id,
                provider_id: provider.id,
                expires_at: now + Duration::seconds(LOGIN_CODE_TTL_SECONDS),
            },
        )
        .await?;

    let metadata = serde_json::json!({
        "provider_id": provider.id.to_string(),
        "provider_slug": provider.slug,
        "subject": claims.subject,
        "email": email,
    });
    if provisioned {
        log(
            conn,
            repos.activity,
            &user,
            activity_types::SSO_USER_PROVISIONED,
            format!(
                "User '{}' provisioned through SSO provider '{}'",
                user.username, provider.slug
            ),
            metadata.clone(),
        )
        .await?;
    }
    if linked {
        log(
            conn,
            repos.activity,
            &user,
            activity_types::SSO_IDENTITY_LINKED,
            format!(
                "SSO identity of provider '{}' linked to user '{}' by email",
                provider.slug, user.username
            ),
            metadata.clone(),
        )
        .await?;
    }
    log(
        conn,
        repos.activity,
        &user,
        activity_types::SSO_LOGIN,
        format!(
            "User '{}' signed in through SSO provider '{}'",
            user.username, provider.slug
        ),
        metadata,
    )
    .await?;

    Ok(CompletedSsoLogin {
        user_id: user.id,
        code,
        provisioned,
        linked,
    })
}

async fn provision_user<U: UserRepositoryTx>(
    conn: &mut PgConnection,
    users: &U,
    claims: &IdTokenClaims,
    email: &str,
) -> Result<User, SsoLoginError> {
    let base = username_base(claims.preferred_username.as_deref(), email);
    let mut username = None;
    for n in 1..=MAX_USERNAME_SUFFIX {
        let candidate = username_candidate(&base, n);
        if !users.username_exists_tx(conn, &candidate).await? {
            username = Some(candidate);
            break;
        }
    }
    let username = match username {
        Some(name) => name,
        None => {
            let suffix = &Uuid::new_v4().simple().to_string()[..8];
            let keep = USER_NAME_COLUMN_LIMIT - suffix.len() - 1;
            format!("{}-{suffix}", truncate_chars(&base, keep))
        }
    };
    let (first_name, last_name) = derive_names(
        claims.given_name.as_deref(),
        claims.family_name.as_deref(),
        claims.name.as_deref(),
        email,
    );
    Ok(users
        .create_sso_user_tx(
            conn,
            CreateSsoUser {
                username,
                first_name,
                last_name,
                email: email.to_string(),
                // Set when the exchange issues the session.
                last_login: None,
            },
        )
        .await?)
}

async fn log(
    conn: &mut PgConnection,
    activity: &dyn ActivityLogRepository,
    user: &User,
    activity_type: &str,
    description: String,
    metadata: serde_json::Value,
) -> Result<(), SsoLoginError> {
    activity
        .create_activity_tx(
            conn,
            CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: entity_types::USER.to_string(),
                entity_id: user.id.to_string(),
                actor_id: Some(user.id),
                actor_name: Some(user.username.clone()),
                description,
                metadata: Some(metadata),
            },
        )
        .await
        .map_err(|err| SsoLoginError::from(crate::Error::DatabaseError(err)))?;
    Ok(())
}
