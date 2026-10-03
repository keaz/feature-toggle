//! Writes for Jira integrations, each with its activity row, inside one transaction.
//! Activity rows never contain the secret or its hash.

use std::collections::{BTreeMap, HashSet};

use sqlx::PgConnection;
use uuid::Uuid;

use crate::Error;
use crate::config::JiraConfig;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::{JiraIntegrationRow, JiraStatusRuleRow};
use crate::database::jira_integration::{
    CreateJiraIntegration, JiraIntegrationRepositoryTx, JiraWritebackColumns, UpdateJiraIntegration,
};
use crate::database::jira_outbound_job::JiraOutboundJobRepositoryTx;
use crate::logic::ActorContext;
use crate::logic::external_link::validate_url;
use crate::logic::jira_client::JiraEdition;
use crate::logic::jira_integration::{
    JiraStatusRuleInput, generate_secret, hash_secret, validate_environment_aliases,
    validate_environment_ids, validate_field_name, validate_integration_name, validate_rules,
};
use crate::logic::secret_box;
use crate::utils::activity_logger::activity_types::{
    JIRA_INTEGRATION_CREATED, JIRA_INTEGRATION_DELETED, JIRA_INTEGRATION_RULES_REPLACED,
    JIRA_INTEGRATION_SECRET_ROTATED, JIRA_INTEGRATION_UPDATED,
};

const ENTITY_TYPE: &str = "jira_integration";
/// `last_error` of jobs cancelled because write-back went off.
const WRITEBACK_DISABLED: &str = "write-back disabled";

/// A new integration as sent by a client. Environment ids are strings, checked
/// against the team.
#[derive(Debug, Clone, Default)]
pub struct JiraIntegrationInput {
    pub name: String,
    pub jira_base_url: Option<String>,
    pub environment_field: String,
    pub environment_aliases: BTreeMap<String, String>,
    pub jira_approved_environment_ids: Vec<String>,
    pub feature_key_field: Option<String>,
    pub enabled: bool,
}

/// Changes to an integration. `None` keeps the stored value; a blank
/// `jira_base_url` or `feature_key_field` clears it.
#[derive(Debug, Clone, Default)]
pub struct JiraIntegrationPatch {
    pub name: Option<String>,
    pub jira_base_url: Option<String>,
    pub environment_field: Option<String>,
    pub environment_aliases: Option<BTreeMap<String, String>>,
    pub jira_approved_environment_ids: Option<Vec<String>>,
    pub feature_key_field: Option<String>,
    pub enabled: Option<bool>,
}

/// An integration and its inbound secret. The secret is returned only here, once.
#[derive(Debug, Clone)]
pub struct JiraIntegrationWithSecret {
    pub integration: JiraIntegrationRow,
    pub secret: String,
}

fn optional_field_name(value: Option<&str>, label: &str) -> Result<Option<String>, Error> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => validate_field_name(value, label).map(Some),
        None => Ok(None),
    }
}

/// Environment ids of the team; `Error::NotFound(team_id)` when the team does not exist.
async fn team_environment_ids<R>(
    conn: &mut PgConnection,
    repo: &R,
    team_id: Uuid,
) -> Result<HashSet<Uuid>, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    Ok(repo
        .team_environment_ids_tx(conn, team_id)
        .await?
        .ok_or(Error::NotFound(team_id))?
        .into_iter()
        .collect())
}

async fn existing<R>(
    conn: &mut PgConnection,
    repo: &R,
    id: Uuid,
) -> Result<JiraIntegrationRow, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    repo.get_tx(conn, id).await?.ok_or(Error::NotFound(id))
}

/// Creates the integration, its shadow user and secret, and writes a
/// `jira_integration_created` activity row. `Error::NotFound(team_id)` when the
/// team does not exist.
pub async fn create_jira_integration_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    team_id: Uuid,
    input: JiraIntegrationInput,
    actor: ActorContext,
) -> Result<JiraIntegrationWithSecret, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    let team_environments = team_environment_ids(conn, repo, team_id).await?;
    let name = validate_integration_name(&input.name)?;
    let jira_base_url = validate_url(input.jira_base_url.as_deref())?;
    let environment_field = validate_field_name(&input.environment_field, "environmentField")?;
    let feature_key_field =
        optional_field_name(input.feature_key_field.as_deref(), "featureKeyField")?;
    let environment_aliases =
        validate_environment_aliases(&input.environment_aliases, &team_environments)?;
    let jira_approved_environment_ids = validate_environment_ids(
        &input.jira_approved_environment_ids,
        &team_environments,
        "jiraApprovedEnvironmentIds",
    )?;

    let secret = generate_secret();
    let integration = repo
        .create_tx(
            conn,
            CreateJiraIntegration {
                team_id,
                name,
                jira_base_url,
                secret_hash: hash_secret(&secret),
                environment_field,
                environment_aliases,
                jira_approved_environment_ids,
                feature_key_field,
                enabled: input.enabled,
            },
        )
        .await?;
    write_activity(
        conn,
        activity_repo,
        JIRA_INTEGRATION_CREATED,
        format!("Created Jira integration '{}'", integration.name),
        &integration,
        Some(config_metadata(&integration)),
        actor,
    )
    .await?;
    Ok(JiraIntegrationWithSecret {
        integration,
        secret,
    })
}

/// Applies `patch` and writes a `jira_integration_updated` activity row.
/// `Error::NotFound(id)` when the integration does not exist.
pub async fn update_jira_integration_in_tx<R, O>(
    conn: &mut PgConnection,
    repo: &R,
    outbound_repo: &O,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    patch: JiraIntegrationPatch,
    jira_config: &JiraConfig,
    actor: ActorContext,
) -> Result<JiraIntegrationRow, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
    O: JiraOutboundJobRepositoryTx + ?Sized,
{
    let current = existing(conn, repo, id).await?;
    let team_environments = team_environment_ids(conn, repo, current.team_id).await?;
    let update = UpdateJiraIntegration {
        name: patch
            .name
            .as_deref()
            .map(validate_integration_name)
            .transpose()?,
        jira_base_url: patch
            .jira_base_url
            .as_deref()
            .map(|url| validate_url(Some(url)))
            .transpose()?,
        environment_field: patch
            .environment_field
            .as_deref()
            .map(|field| validate_field_name(field, "environmentField"))
            .transpose()?,
        environment_aliases: patch
            .environment_aliases
            .as_ref()
            .map(|aliases| validate_environment_aliases(aliases, &team_environments))
            .transpose()?,
        jira_approved_environment_ids: patch
            .jira_approved_environment_ids
            .as_deref()
            .map(|ids| {
                validate_environment_ids(ids, &team_environments, "jiraApprovedEnvironmentIds")
            })
            .transpose()?,
        feature_key_field: patch
            .feature_key_field
            .as_deref()
            .map(|field| optional_field_name(Some(field), "featureKeyField"))
            .transpose()?,
        enabled: patch.enabled,
    };
    let mut changed_fields = changed_field_names(&update);
    let host_changed = match &update.jira_base_url {
        Some(new_url) => {
            if current.writeback_enabled {
                validate_writeback_base_url(new_url.as_deref(), jira_config)?;
            }
            origin_of(new_url.as_deref()) != origin_of(current.jira_base_url.as_deref())
        }
        None => false,
    };
    let mut updated = repo
        .update_tx(conn, id, update)
        .await?
        .ok_or(Error::NotFound(id))?;
    if host_changed && updated.jira_credential_enc.is_some() {
        // The stored token must not follow the integration to another host.
        updated = repo
            .set_writeback_tx(
                conn,
                id,
                JiraWritebackColumns {
                    enabled: false,
                    comments: updated.writeback_comments,
                    remote_link: updated.writeback_remote_link,
                    auth_kind: updated.jira_auth_kind.clone(),
                    account_email: updated.jira_account_email.clone(),
                    credential_enc: None,
                    clear_paused_reason: false,
                },
            )
            .await?
            .ok_or(Error::NotFound(id))?;
        changed_fields.push("jira_credential");
        if current.writeback_enabled {
            changed_fields.push("writeback_enabled");
            outbound_repo
                .cancel_pending_tx(conn, id, WRITEBACK_DISABLED)
                .await?;
        }
    }
    let mut metadata = config_metadata(&updated);
    metadata["changed_fields"] = serde_json::json!(changed_fields);
    write_activity(
        conn,
        activity_repo,
        JIRA_INTEGRATION_UPDATED,
        format!("Updated Jira integration '{}'", updated.name),
        &updated,
        Some(metadata),
        actor,
    )
    .await?;
    Ok(updated)
}

/// A write-back configuration as sent by a client.
#[derive(Clone, Default)]
pub struct WritebackPatch {
    pub enabled: bool,
    pub comments: bool,
    pub remote_link: bool,
    /// `None` keeps the stored value; blank clears it.
    pub auth_kind: Option<String>,
    /// `None` keeps the stored value; blank clears it.
    pub account_email: Option<String>,
    /// `None` keeps the stored credential; blank clears it (only with `enabled = false`).
    pub credential: Option<String>,
}

impl std::fmt::Debug for WritebackPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritebackPatch")
            .field("enabled", &self.enabled)
            .field("comments", &self.comments)
            .field("remote_link", &self.remote_link)
            .field("auth_kind", &self.auth_kind)
            .field("account_email", &self.account_email)
            .field("credential", &self.credential.as_ref().map(|_| "***"))
            .finish()
    }
}

const MAX_CREDENTIAL_CHARS: usize = 1000;
const MAX_EMAIL_CHARS: usize = 254;

fn invalid(message: &str) -> Error {
    Error::InvalidInput(message.to_string())
}

fn validate_account_email(email: &str) -> Result<(), Error> {
    if email.chars().count() > MAX_EMAIL_CHARS || !email.contains('@') {
        return Err(invalid(
            "accountEmail must contain '@' and be at most 254 characters",
        ));
    }
    Ok(())
}

/// Scheme, host and port of a base URL; `None` for no URL or an unparsable one.
fn origin_of(base_url: Option<&str>) -> Option<String> {
    let url = reqwest::Url::parse(base_url?).ok()?;
    Some(url.origin().ascii_serialization())
}

fn validate_writeback_base_url(base_url: Option<&str>, config: &JiraConfig) -> Result<(), Error> {
    let base_url =
        base_url.ok_or_else(|| invalid("jira base URL is required while write-back is enabled"))?;
    let parsed =
        reqwest::Url::parse(base_url).map_err(|_| invalid("jira base URL must be a valid URL"))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if config.allow_insecure_http => Ok(()),
        _ => Err(invalid("jira base URL must be an https URL")),
    }
}

/// Stores the write-back configuration and writes a `jira_integration_updated`
/// activity row whose `changed_fields` name the columns, never the credential.
/// Rules apply only when `patch.enabled`. Turning write-back off cancels the pending
/// outbound jobs. `Error::NotFound(id)` when the integration does not exist.
#[allow(clippy::too_many_arguments)]
pub async fn update_jira_writeback_in_tx<R, O>(
    conn: &mut PgConnection,
    repo: &R,
    outbound_repo: &O,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    patch: WritebackPatch,
    jira_config: &JiraConfig,
    ui_base_url: Option<&str>,
    actor: ActorContext,
) -> Result<JiraIntegrationRow, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
    O: JiraOutboundJobRepositoryTx + ?Sized,
{
    let current = existing(conn, repo, id).await?;

    let auth_kind = match patch.auth_kind.as_deref().map(str::trim) {
        None => current.jira_auth_kind.clone(),
        Some("") => None,
        Some(kind) if JiraEdition::from_auth_kind(kind).is_some() => Some(kind.to_string()),
        Some(_) => return Err(invalid("authKind must be cloud_basic or dc_pat")),
    };
    let account_email = match patch.account_email.as_deref().map(str::trim) {
        None => current.jira_account_email.clone(),
        Some("") => None,
        Some(email) => {
            validate_account_email(email)?;
            Some(email.to_string())
        }
    };
    let new_credential = match patch.credential.as_deref().map(str::trim) {
        None => None,
        Some("") => {
            if patch.enabled {
                return Err(invalid(
                    "credential cannot be cleared while write-back is enabled",
                ));
            }
            None
        }
        Some(value) if value.chars().count() > MAX_CREDENTIAL_CHARS => {
            return Err(invalid("credential must be at most 1000 characters"));
        }
        // Tokens are sent in an Authorization header: no spaces, controls or non-ASCII.
        Some(value) if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) => {
            return Err(invalid("credential contains invalid characters"));
        }
        Some(value) => Some(value),
    };
    let clears_credential = matches!(patch.credential.as_deref().map(str::trim), Some(""));
    let credential_enc =
        match new_credential {
            Some(value) => Some(secret_box::encrypt_with_aad(value, id.as_bytes()).map_err(
                |_| invalid("FLUXGATE_ENCRYPTION_KEY must be set to store a credential"),
            )?),
            None if clears_credential => None,
            None => current.jira_credential_enc.clone(),
        };
    let clear_paused_reason = new_credential.is_some();

    if patch.enabled {
        validate_writeback_base_url(current.jira_base_url.as_deref(), jira_config)?;
        let kind = auth_kind
            .as_deref()
            .ok_or_else(|| invalid("authKind is required for write-back"))?;
        if credential_enc.is_none() {
            return Err(invalid("a credential is required for write-back"));
        }
        if JiraEdition::from_auth_kind(kind) == Some(JiraEdition::Cloud) && account_email.is_none()
        {
            return Err(invalid("accountEmail is required for cloud_basic"));
        }
        if patch.remote_link && ui_base_url.is_none() {
            return Err(invalid("ui base URL is not configured"));
        }
    }

    let columns = JiraWritebackColumns {
        enabled: patch.enabled,
        comments: patch.comments,
        remote_link: patch.remote_link,
        auth_kind,
        account_email,
        credential_enc,
        clear_paused_reason,
    };
    let changed_fields: Vec<&'static str> = [
        (
            "writeback_enabled",
            columns.enabled != current.writeback_enabled,
        ),
        (
            "writeback_comments",
            columns.comments != current.writeback_comments,
        ),
        (
            "writeback_remote_link",
            columns.remote_link != current.writeback_remote_link,
        ),
        (
            "jira_auth_kind",
            columns.auth_kind != current.jira_auth_kind,
        ),
        (
            "jira_account_email",
            columns.account_email != current.jira_account_email,
        ),
        ("jira_credential", patch.credential.is_some()),
        (
            "writeback_paused_reason",
            columns.clear_paused_reason && current.writeback_paused_reason.is_some(),
        ),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect();

    let updated = repo
        .set_writeback_tx(conn, id, columns)
        .await?
        .ok_or(Error::NotFound(id))?;
    if current.writeback_enabled && !updated.writeback_enabled {
        // Jobs queued while write-back was on must not go out after it is turned off.
        outbound_repo
            .cancel_pending_tx(conn, id, WRITEBACK_DISABLED)
            .await?;
    }
    write_writeback_activity(
        conn,
        activity_repo,
        &updated,
        "Updated write-back of Jira integration",
        changed_fields,
        actor,
    )
    .await?;
    Ok(updated)
}

/// Clears the paused reason and writes a `jira_integration_updated` activity row.
/// `Error::NotFound(id)` when the integration does not exist.
pub async fn resume_jira_writeback_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    actor: ActorContext,
) -> Result<JiraIntegrationRow, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    let updated = repo
        .set_writeback_paused_tx(conn, id, None)
        .await?
        .ok_or(Error::NotFound(id))?;
    write_writeback_activity(
        conn,
        activity_repo,
        &updated,
        "Resumed write-back of Jira integration",
        vec!["writeback_paused_reason"],
        actor,
    )
    .await?;
    Ok(updated)
}

/// Activity row for a write-back change. Carries flags and field names only, never
/// the credential or its sealed form.
async fn write_writeback_activity(
    conn: &mut PgConnection,
    activity_repo: &dyn ActivityLogRepository,
    integration: &JiraIntegrationRow,
    description: &str,
    changed_fields: Vec<&'static str>,
    actor: ActorContext,
) -> Result<(), Error> {
    let mut metadata = config_metadata(integration);
    metadata["changed_fields"] = serde_json::json!(changed_fields);
    metadata["writeback_enabled"] = integration.writeback_enabled.into();
    metadata["writeback_comments"] = integration.writeback_comments.into();
    metadata["writeback_remote_link"] = integration.writeback_remote_link.into();
    metadata["jira_auth_kind"] = serde_json::json!(integration.jira_auth_kind);
    metadata["has_credential"] = integration.jira_credential_enc.is_some().into();
    write_activity(
        conn,
        activity_repo,
        JIRA_INTEGRATION_UPDATED,
        format!("{description} '{}'", integration.name),
        integration,
        Some(metadata),
        actor,
    )
    .await
}

/// Replaces the secret and writes a `jira_integration_secret_rotated` activity row.
/// `Error::NotFound(id)` when the integration does not exist.
pub async fn rotate_jira_integration_secret_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    actor: ActorContext,
) -> Result<JiraIntegrationWithSecret, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    let secret = generate_secret();
    let integration = repo
        .set_secret_hash_tx(conn, id, hash_secret(&secret))
        .await?
        .ok_or(Error::NotFound(id))?;
    write_activity(
        conn,
        activity_repo,
        JIRA_INTEGRATION_SECRET_ROTATED,
        format!(
            "Rotated the secret of Jira integration '{}'",
            integration.name
        ),
        &integration,
        None,
        actor,
    )
    .await?;
    Ok(JiraIntegrationWithSecret {
        integration,
        secret,
    })
}

/// Validates and replaces every status rule, and writes a
/// `jira_integration_rules_replaced` activity row.
/// `Error::NotFound(id)` when the integration does not exist.
pub async fn replace_jira_status_rules_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    rules: Vec<JiraStatusRuleInput>,
    actor: ActorContext,
) -> Result<Vec<JiraStatusRuleRow>, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    let integration = existing(conn, repo, id).await?;
    let team_environments = team_environment_ids(conn, repo, integration.team_id).await?;
    let rules = validate_rules(&rules, &team_environments)?;
    let stored = repo.replace_rules_tx(conn, id, rules).await?;
    write_activity(
        conn,
        activity_repo,
        JIRA_INTEGRATION_RULES_REPLACED,
        format!(
            "Set {} status rule(s) of Jira integration '{}'",
            stored.len(),
            integration.name
        ),
        &integration,
        Some(serde_json::json!({ "rules": stored })),
        actor,
    )
    .await?;
    Ok(stored)
}

/// Deletes the integration (rules cascade), disables its shadow user and writes a
/// `jira_integration_deleted` activity row.
/// `Error::NotFound(id)` when the integration does not exist.
pub async fn delete_jira_integration_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    actor: ActorContext,
) -> Result<(), Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
{
    let deleted = repo.delete_tx(conn, id).await?.ok_or(Error::NotFound(id))?;
    write_activity(
        conn,
        activity_repo,
        JIRA_INTEGRATION_DELETED,
        format!("Deleted Jira integration '{}'", deleted.name),
        &deleted,
        None,
        actor,
    )
    .await
}

fn changed_field_names(update: &UpdateJiraIntegration) -> Vec<&'static str> {
    [
        ("name", update.name.is_some()),
        ("jira_base_url", update.jira_base_url.is_some()),
        ("environment_field", update.environment_field.is_some()),
        ("environment_aliases", update.environment_aliases.is_some()),
        (
            "jira_approved_environment_ids",
            update.jira_approved_environment_ids.is_some(),
        ),
        ("feature_key_field", update.feature_key_field.is_some()),
        ("enabled", update.enabled.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect()
}

/// The configuration of the integration, without its secret hash.
fn config_metadata(integration: &JiraIntegrationRow) -> serde_json::Value {
    serde_json::json!({
        "jira_base_url": integration.jira_base_url,
        "environment_field": integration.environment_field,
        "environment_aliases": integration.environment_aliases.0,
        "jira_approved_environment_ids": integration.jira_approved_environment_ids,
        "feature_key_field": integration.feature_key_field,
        "enabled": integration.enabled,
    })
}

async fn write_activity(
    conn: &mut PgConnection,
    activity_repo: &dyn ActivityLogRepository,
    activity_type: &str,
    description: String,
    integration: &JiraIntegrationRow,
    details: Option<serde_json::Value>,
    actor: ActorContext,
) -> Result<(), Error> {
    let mut metadata = serde_json::json!({
        "integration_id": integration.id.to_string(),
        "team_id": integration.team_id.to_string(),
        "name": integration.name,
        "actor_user_id": integration.actor_user_id.to_string(),
    });
    if let Some(serde_json::Value::Object(details)) = details {
        metadata
            .as_object_mut()
            .expect("metadata is an object")
            .extend(details);
    }
    let (actor_id, actor_name) = actor.as_option();
    activity_repo
        .create_activity_tx(
            conn,
            CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: ENTITY_TYPE.to_string(),
                entity_id: integration.id.to_string(),
                actor_id,
                actor_name,
                description,
                metadata: Some(metadata),
            },
        )
        .await
        .map_err(Error::DatabaseError)?;
    Ok(())
}
