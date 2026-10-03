//! Writes for feature external links, each with its activity row, inside one transaction.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::Error;
use crate::database::activity_log::{ActivityLogRepository, CreateActivityLog};
use crate::database::entity::ExternalLinkRow;
use crate::database::external_link::{CreateExternalLink, ExternalLinkRepositoryTx, FeatureScope};
use crate::logic::ActorContext;
use crate::logic::external_link::NewExternalLink;
use crate::utils::activity_logger::activity_types::{EXTERNAL_LINK_ADDED, EXTERNAL_LINK_REMOVED};

/// Links `link` to the feature and writes an `external_link_added` activity row.
/// `Error::NotFound(feature_id)` when the feature does not exist,
/// `Error::RecordAlreadyExists` when the feature already links the key.
pub async fn create_external_link_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    feature_id: Uuid,
    link: NewExternalLink,
    actor: Option<ActorContext>,
) -> Result<ExternalLinkRow, Error>
where
    R: ExternalLinkRepositoryTx + ?Sized,
{
    let feature = repo
        .feature_scope_tx(conn, feature_id)
        .await?
        .ok_or(Error::NotFound(feature_id))?;
    let created = repo
        .create_tx(
            conn,
            CreateExternalLink {
                feature_id,
                system: link.system,
                external_key: link.external_key,
                url: link.url,
                created_by: actor.as_ref().map(|actor| actor.id),
            },
        )
        .await?;
    write_activity(
        conn,
        activity_repo,
        EXTERNAL_LINK_ADDED,
        format!(
            "Linked {} to feature '{}'",
            created.external_key, feature.key
        ),
        &feature,
        &created,
        actor,
    )
    .await?;
    Ok(created)
}

/// Removes a link of the feature and writes an `external_link_removed` activity row.
/// `Error::NotFound(feature_id)` when the feature does not exist,
/// `Error::NotFound(link_id)` when the link is not on that feature.
pub async fn delete_external_link_in_tx<R>(
    conn: &mut PgConnection,
    repo: &R,
    activity_repo: &dyn ActivityLogRepository,
    feature_id: Uuid,
    link_id: Uuid,
    actor: Option<ActorContext>,
) -> Result<(), Error>
where
    R: ExternalLinkRepositoryTx + ?Sized,
{
    let feature = repo
        .feature_scope_tx(conn, feature_id)
        .await?
        .ok_or(Error::NotFound(feature_id))?;
    let deleted = repo
        .delete_tx(conn, feature_id, link_id)
        .await?
        .ok_or(Error::NotFound(link_id))?;
    write_activity(
        conn,
        activity_repo,
        EXTERNAL_LINK_REMOVED,
        format!(
            "Unlinked {} from feature '{}'",
            deleted.external_key, feature.key
        ),
        &feature,
        &deleted,
        actor,
    )
    .await
}

async fn write_activity(
    conn: &mut PgConnection,
    activity_repo: &dyn ActivityLogRepository,
    activity_type: &str,
    description: String,
    feature: &FeatureScope,
    link: &ExternalLinkRow,
    actor: Option<ActorContext>,
) -> Result<(), Error> {
    let (actor_id, actor_name) = actor.map(|actor| actor.as_option()).unwrap_or((None, None));
    activity_repo
        .create_activity_tx(
            conn,
            CreateActivityLog {
                activity_type: activity_type.to_string(),
                entity_type: "feature".to_string(),
                entity_id: link.feature_id.to_string(),
                actor_id,
                actor_name,
                description,
                metadata: Some(serde_json::json!({
                    "feature_id": link.feature_id.to_string(),
                    "feature_key": feature.key,
                    "team_id": feature.team_id.to_string(),
                    "link_id": link.id.to_string(),
                    "system": link.system,
                    "external_key": link.external_key,
                    "url": link.url,
                })),
            },
        )
        .await
        .map_err(Error::DatabaseError)?;
    Ok(())
}
