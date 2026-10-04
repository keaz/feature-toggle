use reqwest::Method;
use serde_json::{Value, json};

use super::{App, call, feature_id, json_data, list_paged, optional_team};
use crate::cli::{FlagsArgs, FlagsSubcommand};
use crate::context::{Context, is_uuid};
use crate::error::CliError;
use crate::output::{FEATURE_COLUMNS, Kind, Outcome};

pub async fn run(args: FlagsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    // Check bodies and confirmations before any request.
    let data = match &args.command {
        FlagsSubcommand::Create { data }
        | FlagsSubcommand::Update { data, .. }
        | FlagsSubcommand::ImpactPreview { data, .. } => Some(json_data(data)?),
        FlagsSubcommand::Archive { yes: false, .. } => {
            return Err(CliError::Usage("archiving needs --yes".into()));
        }
        _ => None,
    };
    let context = app.connect().await?;
    match args.command {
        FlagsSubcommand::List { page, filters } => {
            let team = context.team_id().await?;
            let value = list_paged(
                &context.api,
                &["teams", &team, "features"],
                &filters.query(),
                &page,
            )
            .await?;
            Ok(Outcome::new(value, Kind::List(FEATURE_COLUMNS)))
        }
        FlagsSubcommand::Get { id_or_key } => {
            let value = if is_uuid(&id_or_key) {
                context
                    .api
                    .get(&["features", id_or_key.trim()], &[])
                    .await?
            } else {
                let team = context.team_id().await?;
                context
                    .api
                    .get(&["teams", &team, "features", "by-key", &id_or_key], &[])
                    .await?
            };
            Ok(Outcome::new(value, Kind::Object))
        }
        FlagsSubcommand::Create { .. } => {
            let team = context.team_id().await?;
            call(
                &context,
                Method::POST,
                &["teams", &team, "features"],
                &[],
                data,
            )
            .await
        }
        FlagsSubcommand::Update { id_or_key, .. } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(&context, Method::PATCH, &["features", &id], &[], data).await
        }
        FlagsSubcommand::ImpactPreview { id_or_key, .. } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::POST,
                &["features", &id, "impact-preview"],
                &[],
                data,
            )
            .await
        }
        FlagsSubcommand::Archive { flags, .. } => {
            let body = json!({ "action": "archive", "archiveConfirmation": true });
            bulk(&context, &flags, body).await
        }
        FlagsSubcommand::Bulk {
            action,
            flags,
            owner,
            tags,
            lifecycle_stage,
            yes,
        } => {
            let mut body = json!({ "action": action.api_name() });
            if let Some(owner) = owner {
                body["owner"] = json!(owner);
            }
            if !tags.is_empty() {
                body["tags"] = json!(tags);
            }
            if let Some(stage) = lifecycle_stage {
                body["lifecycleStage"] = json!(stage);
            }
            if yes {
                body["archiveConfirmation"] = json!(true);
            }
            bulk(&context, &flags, body).await
        }
        FlagsSubcommand::Versions { id_or_key } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::GET,
                &["features", &id, "versions"],
                &[],
                None,
            )
            .await
        }
        FlagsSubcommand::Diff {
            id_or_key,
            version_id,
        } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::GET,
                &["features", &id, "versions", &version_id, "diff"],
                &[],
                None,
            )
            .await
        }
        FlagsSubcommand::Rollback {
            id_or_key,
            version_id,
            yes,
        } => {
            let id = feature_id(&context, &id_or_key).await?;
            let body = if yes {
                json!({ "archiveConfirmation": true })
            } else {
                json!({})
            };
            let segments = [
                "features",
                id.as_str(),
                "versions",
                version_id.as_str(),
                "rollback",
            ];
            call(&context, Method::POST, &segments, &[], Some(body)).await
        }
        FlagsSubcommand::Impact { id_or_key } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::GET,
                &["features", &id, "dependency-impact"],
                &[],
                None,
            )
            .await
        }
        FlagsSubcommand::Kill {
            id_or_key,
            reason,
            rollback_in,
            expires_at,
        } => {
            let id = feature_id(&context, &id_or_key).await?;
            let mut body = json!({ "reason": reason });
            if let Some(minutes) = rollback_in {
                body["rollbackInMinutes"] = json!(minutes);
            }
            if let Some(at) = expires_at {
                body["expiresAt"] = json!(at);
            }
            call(
                &context,
                Method::POST,
                &["features", &id, "emergency-disable"],
                &[],
                Some(body),
            )
            .await
        }
        FlagsSubcommand::Unkill { id_or_key, reason } => {
            let id = feature_id(&context, &id_or_key).await?;
            let body = json!({ "reason": reason });
            call(
                &context,
                Method::POST,
                &["features", &id, "emergency-enable"],
                &[],
                Some(body),
            )
            .await
        }
        FlagsSubcommand::KillSwitches => {
            let query: Vec<(&str, String)> = optional_team(&context)
                .await?
                .map(|team| ("teamId", team))
                .into_iter()
                .collect();
            call(
                &context,
                Method::GET,
                &["features", "active-kill-switches"],
                &query,
                None,
            )
            .await
        }
        FlagsSubcommand::Schedules { id_or_key } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::GET,
                &["features", &id, "scheduled-changes"],
                &[],
                None,
            )
            .await
        }
        FlagsSubcommand::Schedule {
            id_or_key,
            action,
            at,
            reason,
            stage,
            requested_status,
            payload,
            timezone,
        } => {
            let mut body =
                json!({ "action": action.api_name(), "scheduledAt": at, "reason": reason });
            if let Some(stage) = stage {
                body["stageId"] = json!(stage);
            }
            if let Some(status) = requested_status {
                body["requestedStatus"] = json!(status);
            }
            if let Some(payload) = payload {
                body["payload"] = json_data(&payload)?;
            }
            if let Some(timezone) = timezone {
                body["timezone"] = json!(timezone);
            }
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::POST,
                &["features", &id, "scheduled-changes"],
                &[],
                Some(body),
            )
            .await
        }
        FlagsSubcommand::Unschedule { change_id, reason } => {
            let body = reason
                .map(|reason| json!({ "reason": reason }))
                .unwrap_or_else(|| json!({}));
            call(
                &context,
                Method::PATCH,
                &["scheduled-changes", &change_id, "cancel"],
                &[],
                Some(body),
            )
            .await
        }
        FlagsSubcommand::Reschedule {
            change_id,
            at,
            reason,
            timezone,
        } => {
            let mut body = json!({ "scheduledAt": at });
            if let Some(reason) = reason {
                body["reason"] = json!(reason);
            }
            if let Some(timezone) = timezone {
                body["timezone"] = json!(timezone);
            }
            call(
                &context,
                Method::PATCH,
                &["scheduled-changes", &change_id, "reschedule"],
                &[],
                Some(body),
            )
            .await
        }
        FlagsSubcommand::Links { id_or_key } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::GET,
                &["features", &id, "external-links"],
                &[],
                None,
            )
            .await
        }
        FlagsSubcommand::Link {
            id_or_key,
            issue,
            issue_url: url,
        } => {
            let id = feature_id(&context, &id_or_key).await?;
            let mut body = json!({ "system": "jira", "externalKey": issue });
            if let Some(url) = url {
                body["url"] = json!(url);
            }
            call(
                &context,
                Method::POST,
                &["features", &id, "external-links"],
                &[],
                Some(body),
            )
            .await
        }
        FlagsSubcommand::Unlink { id_or_key, link_id } => {
            let id = feature_id(&context, &id_or_key).await?;
            call(
                &context,
                Method::DELETE,
                &["features", &id, "external-links", &link_id],
                &[],
                None,
            )
            .await
        }
        FlagsSubcommand::Search { query, limit } => {
            let team = context.team_id().await?;
            let mut body = json!({ "query": query });
            if let Some(limit) = limit {
                body["limit"] = json!(limit);
            }
            call(
                &context,
                Method::POST,
                &["teams", &team, "features", "nl-search"],
                &[],
                Some(body),
            )
            .await
        }
    }
}

/// `POST /teams/{team}/features/bulk-actions` for flags given by id or key.
async fn bulk(context: &Context, flags: &[String], mut body: Value) -> Result<Outcome, CliError> {
    let team = context.team_id().await?;
    let mut ids = Vec::with_capacity(flags.len());
    for flag in flags {
        ids.push(feature_id(context, flag).await?);
    }
    body["featureIds"] = json!(ids);
    call(
        context,
        Method::POST,
        &["teams", &team, "features", "bulk-actions"],
        &[],
        Some(body),
    )
    .await
}
