//! Transactional SSO admin writes: providers, group mappings and settings, each with
//! its activity log entry in the same transaction.

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::{SsoGroupMapping, SsoProvider};
use crate::database::role::RoleRepository;
use crate::database::sso_group_mapping::{NewSsoGroupMapping, SsoGroupMappingRepositoryTx};
use crate::database::sso_provider::{
    CreateSsoProvider, SsoProviderRepositoryTx, UpdateSsoProvider,
};
use crate::database::sso_settings::SsoSettingsRepositoryTx;
use crate::database::team::TeamRepositoryTx;
use crate::database::user_identity::UserIdentityRepositoryTx;
use crate::logic::ActorContext;
use crate::logic::sso_provider::{
    ProviderFields, SsoAdminError, SsoSecrets, validate_create, validate_patch,
};
use crate::utils::activity_logger::{activity_types, entity_types};
use sqlx::PgConnection;
use uuid::Uuid;

pub const TARGET_ROLE: &str = "role";
pub const TARGET_TEAM: &str = "team";
pub const TARGET_ADMIN: &str = "admin";

const MAX_MAPPINGS: usize = 1000;
const MAX_GROUP_VALUE_LEN: usize = 255;

fn actor_details(actor: &Option<ActorContext>) -> (Option<Uuid>, Option<String>) {
    actor
        .as_ref()
        .map(|a| a.as_option())
        .unwrap_or((None, None))
}

async fn log_activity(
    conn: &mut PgConnection,
    activity_repo: &dyn ActivityLogRepository,
    activity_type: &str,
    entity_type: &str,
    entity_id: String,
    description: String,
    metadata: serde_json::Value,
    actor: &Option<ActorContext>,
) -> Result<(), Error> {
    let (actor_id, actor_name) = actor_details(actor);
    activity_repo
        .create_activity_tx(
            conn,
            CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: entity_type.to_string(),
                entity_id,
                actor_id,
                actor_name,
                description,
                // Never include the client secret, plain or sealed.
                metadata: Some(metadata),
            },
        )
        .await
        .map_err(Error::DatabaseError)?;
    Ok(())
}

/// Creates a provider. The client secret, if any, is sealed with the new provider id as
/// associated data, so the row is inserted first and the secret set in the same
/// transaction.
pub async fn create_provider_in_tx<P>(
    conn: &mut PgConnection,
    repo: &P,
    activity_repo: &dyn ActivityLogRepository,
    secrets: &SsoSecrets,
    fields: ProviderFields,
    client_secret: Option<String>,
    actor: Option<ActorContext>,
) -> Result<SsoProvider, SsoAdminError>
where
    P: SsoProviderRepositoryTx,
{
    let validated = validate_create(fields)?;
    let secret = client_secret.filter(|s| !s.is_empty());
    if secret.is_some() {
        // Fail before touching the database when no key is configured.
        secrets.seal(Uuid::nil(), "probe")?;
    }

    let mut provider = repo
        .create_provider_tx(
            conn,
            CreateSsoProvider {
                slug: validated.slug,
                display_name: validated.display_name,
                issuer_url: validated.issuer_url,
                client_id: validated.client_id,
                client_secret_enc: None,
                scopes: validated.scopes,
                groups_claim: validated.groups_claim,
                allowed_email_domains: validated.allowed_email_domains,
                jit_provisioning: validated.jit_provisioning,
                allow_email_linking: validated.allow_email_linking,
                role_sync_mode: validated.role_sync_mode,
                enabled: validated.enabled,
            },
        )
        .await?;

    if let Some(secret) = secret.as_deref() {
        let sealed = secrets.seal(provider.id, secret)?;
        provider = repo
            .update_provider_tx(
                conn,
                provider.id,
                UpdateSsoProvider {
                    client_secret_enc: Some(Some(sealed)),
                    ..Default::default()
                },
            )
            .await?;
    }

    log_activity(
        conn,
        activity_repo,
        activity_types::SSO_PROVIDER_CREATED,
        entity_types::SSO_PROVIDER,
        provider.id.to_string(),
        format!("Created SSO provider '{}'", provider.slug),
        serde_json::json!({
            "provider_id": provider.id.to_string(),
            "slug": provider.slug,
            "enabled": provider.enabled,
            "has_client_secret": provider.client_secret_enc.is_some(),
        }),
        &actor,
    )
    .await?;

    Ok(provider)
}

/// Partial update. `client_secret`: `None` leaves the stored secret, `Some("")` clears
/// it, any other value replaces it.
///
/// A changed issuer URL deletes the provider's user identities: a subject is only
/// unique per issuer, so links made against the old issuer must not match accounts
/// at the new one. Users keep their accounts and are linked again (or provisioned)
/// on their next SSO login.
pub async fn update_provider_in_tx<P, I>(
    conn: &mut PgConnection,
    repo: &P,
    identities: &I,
    activity_repo: &dyn ActivityLogRepository,
    secrets: &SsoSecrets,
    id: Uuid,
    fields: ProviderFields,
    client_secret: Option<String>,
    actor: Option<ActorContext>,
) -> Result<SsoProvider, SsoAdminError>
where
    P: SsoProviderRepositoryTx,
    I: UserIdentityRepositoryTx,
{
    let patch = validate_patch(fields)?;
    // Existence check first so an unknown id is 404 even for an invalid body.
    let before = repo.get_provider_by_id_tx(conn, id).await?;

    let client_secret_enc = match client_secret.as_deref() {
        None => None,
        Some("") => Some(None),
        Some(secret) => Some(Some(secrets.seal(id, secret)?)),
    };
    let secret_changed = client_secret_enc.is_some();

    let mut changed_fields: Vec<&str> = Vec::new();
    let mut note = |name: &'static str, present: bool| {
        if present {
            changed_fields.push(name);
        }
    };
    note("slug", patch.slug.is_some());
    note("displayName", patch.display_name.is_some());
    note("issuerUrl", patch.issuer_url.is_some());
    note("clientId", patch.client_id.is_some());
    note("clientSecret", secret_changed);
    note("scopes", patch.scopes.is_some());
    note("groupsClaim", patch.groups_claim.is_some());
    note("allowedEmailDomains", patch.allowed_email_domains.is_some());
    note("jitProvisioning", patch.jit_provisioning.is_some());
    note("allowEmailLinking", patch.allow_email_linking.is_some());
    note("roleSyncMode", patch.role_sync_mode.is_some());
    note("enabled", patch.enabled.is_some());

    let provider = repo
        .update_provider_tx(
            conn,
            id,
            UpdateSsoProvider {
                slug: patch.slug,
                display_name: patch.display_name,
                issuer_url: patch.issuer_url,
                client_id: patch.client_id,
                client_secret_enc,
                scopes: patch.scopes,
                groups_claim: patch.groups_claim,
                allowed_email_domains: patch.allowed_email_domains,
                jit_provisioning: patch.jit_provisioning,
                allow_email_linking: patch.allow_email_linking,
                role_sync_mode: patch.role_sync_mode,
                enabled: patch.enabled,
            },
        )
        .await?;

    let issuer_changed = provider.issuer_url != before.issuer_url;
    let identities_cleared = if issuer_changed {
        identities
            .delete_identities_for_provider_tx(conn, provider.id)
            .await?
    } else {
        0
    };

    log_activity(
        conn,
        activity_repo,
        activity_types::SSO_PROVIDER_UPDATED,
        entity_types::SSO_PROVIDER,
        provider.id.to_string(),
        format!("Updated SSO provider '{}'", provider.slug),
        serde_json::json!({
            "provider_id": provider.id.to_string(),
            "slug": provider.slug,
            "previous_slug": before.slug,
            "changed_fields": changed_fields,
            "issuer_changed": issuer_changed,
            "identities_cleared": identities_cleared,
        }),
        &actor,
    )
    .await?;

    Ok(provider)
}

/// Deletes a provider; its identities, mappings, login states and codes cascade.
/// Users remain.
pub async fn delete_provider_in_tx<P>(
    conn: &mut PgConnection,
    repo: &P,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    P: SsoProviderRepositoryTx,
{
    let provider = repo.get_provider_by_id_tx(conn, id).await?;
    repo.delete_provider_tx(conn, id).await?;
    log_activity(
        conn,
        activity_repo,
        activity_types::SSO_PROVIDER_DELETED,
        entity_types::SSO_PROVIDER,
        id.to_string(),
        format!("Deleted SSO provider '{}'", provider.slug),
        serde_json::json!({
            "provider_id": id.to_string(),
            "slug": provider.slug,
        }),
        &actor,
    )
    .await
}

/// One requested mapping, as received from the API.
#[derive(Debug, Clone)]
pub struct MappingInput {
    pub group_value: String,
    pub target_type: String,
    pub target_id: Option<Uuid>,
}

/// Replaces every mapping of the provider. Validation: group value non-empty,
/// `targetType` one of role/team/admin, `admin` has no `targetId`, role/team require
/// an existing target, no duplicates.
pub async fn replace_mappings_in_tx<P, M, T>(
    conn: &mut PgConnection,
    provider_repo: &P,
    mapping_repo: &M,
    role_repo: &dyn RoleRepository,
    team_repo: &T,
    activity_repo: &dyn ActivityLogRepository,
    provider_id: Uuid,
    inputs: Vec<MappingInput>,
    actor: Option<ActorContext>,
) -> Result<Vec<SsoGroupMapping>, SsoAdminError>
where
    P: SsoProviderRepositoryTx,
    M: SsoGroupMappingRepositoryTx,
    T: TeamRepositoryTx,
{
    let provider = provider_repo
        .get_provider_by_id_tx(conn, provider_id)
        .await?;

    if inputs.len() > MAX_MAPPINGS {
        return Err(SsoAdminError::invalid(format!(
            "at most {MAX_MAPPINGS} mappings are allowed"
        )));
    }

    let mut accepted: Vec<NewSsoGroupMapping> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let group_value = input.group_value.trim().to_string();
        if group_value.is_empty() {
            return Err(SsoAdminError::invalid("groupValue is required"));
        }
        if group_value.chars().count() > MAX_GROUP_VALUE_LEN {
            return Err(SsoAdminError::invalid(format!(
                "groupValue must be at most {MAX_GROUP_VALUE_LEN} characters"
            )));
        }
        match input.target_type.as_str() {
            TARGET_ADMIN => {
                if input.target_id.is_some() {
                    return Err(SsoAdminError::invalid(
                        "targetId must be null for an admin mapping",
                    ));
                }
            }
            TARGET_ROLE | TARGET_TEAM => {
                let Some(target_id) = input.target_id else {
                    return Err(SsoAdminError::invalid(format!(
                        "targetId is required for a {} mapping",
                        input.target_type
                    )));
                };
                let exists = if input.target_type == TARGET_ROLE {
                    match role_repo.get_role_by_id(target_id).await {
                        Ok(_) => true,
                        Err(Error::NotFound(_)) => false,
                        Err(err) => return Err(err.into()),
                    }
                } else {
                    match team_repo.get_team_by_id_tx(conn, target_id).await {
                        Ok(_) => true,
                        Err(Error::NotFound(_)) => false,
                        Err(err) => return Err(err.into()),
                    }
                };
                if !exists {
                    return Err(SsoAdminError::invalid(format!(
                        "{} {target_id} does not exist",
                        input.target_type
                    )));
                }
            }
            _ => {
                return Err(SsoAdminError::invalid(
                    "targetType must be one of role, team, admin",
                ));
            }
        }

        let mapping = NewSsoGroupMapping {
            group_value,
            target_type: input.target_type,
            target_id: input.target_id,
        };
        if accepted.contains(&mapping) {
            return Err(SsoAdminError::invalid(format!(
                "duplicate mapping for group '{}'",
                mapping.group_value
            )));
        }
        accepted.push(mapping);
    }

    let count = accepted.len();
    let mappings = mapping_repo
        .replace_mappings_tx(conn, provider_id, accepted)
        .await?;

    log_activity(
        conn,
        activity_repo,
        activity_types::SSO_MAPPINGS_UPDATED,
        entity_types::SSO_PROVIDER,
        provider_id.to_string(),
        format!(
            "Replaced group mappings of SSO provider '{}'",
            provider.slug
        ),
        serde_json::json!({
            "provider_id": provider_id.to_string(),
            "slug": provider.slug,
            "mapping_count": count,
        }),
        &actor,
    )
    .await?;

    Ok(mappings)
}

/// Sets the enforce-SSO flag. Turning it on fails with
/// `EnforceSsoRequiresLocalAdmin` unless a break-glass admin exists.
pub async fn set_settings_in_tx<S>(
    conn: &mut PgConnection,
    repo: &S,
    activity_repo: &dyn ActivityLogRepository,
    enforce_sso: bool,
    actor: Option<ActorContext>,
) -> Result<bool, Error>
where
    S: SsoSettingsRepositoryTx,
{
    // Refuse to lock everyone out: with SSO enforced, only break-glass admins
    // can still use a password, so at least one must exist.
    if enforce_sso && !repo.has_break_glass_admin_tx(conn).await? {
        return Err(Error::EnforceSsoRequiresLocalAdmin);
    }
    let previous = repo.get_enforce_sso_tx(conn).await?;
    let current = repo.set_enforce_sso_tx(conn, enforce_sso).await?;
    log_activity(
        conn,
        activity_repo,
        activity_types::SSO_SETTINGS_UPDATED,
        entity_types::SSO_SETTINGS,
        "sso".to_string(),
        format!("SSO enforcement set to {current}"),
        serde_json::json!({
            "enforce_sso": current,
            "previous_enforce_sso": previous,
        }),
        &actor,
    )
    .await?;
    Ok(current)
}
