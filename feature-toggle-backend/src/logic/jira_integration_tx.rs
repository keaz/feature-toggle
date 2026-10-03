//! Writes for Jira integrations, each with its activity row, inside one transaction.
//! Activity rows never contain the secret or its hash.

use std::collections::{BTreeMap, HashSet};

use sqlx::PgConnection;
use uuid::Uuid;

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::{JiraIntegrationRow, JiraStatusRuleRow};
use crate::database::jira_integration::{
    CreateJiraIntegration, JiraIntegrationRepositoryTx, UpdateJiraIntegration,
};
use crate::logic::ActorContext;
use crate::logic::external_link::validate_url;
use crate::logic::jira_integration::{
    JiraStatusRuleInput, generate_secret, hash_secret, validate_environment_aliases,
    validate_environment_ids, validate_field_name, validate_integration_name, validate_rules,
};
use crate::utils::activity_logger::activity_types::{
    JIRA_INTEGRATION_CREATED, JIRA_INTEGRATION_DELETED, JIRA_INTEGRATION_RULES_REPLACED,
    JIRA_INTEGRATION_SECRET_ROTATED, JIRA_INTEGRATION_UPDATED,
};

const ENTITY_TYPE: &str = "jira_integration";

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
pub async fn update_jira_integration_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    id: Uuid,
    patch: JiraIntegrationPatch,
    actor: ActorContext,
) -> Result<JiraIntegrationRow, Error>
where
    R: JiraIntegrationRepositoryTx + ?Sized,
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
    let changed_fields = changed_field_names(&update);
    let updated = repo
        .update_tx(conn, id, update)
        .await?
        .ok_or(Error::NotFound(id))?;
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
