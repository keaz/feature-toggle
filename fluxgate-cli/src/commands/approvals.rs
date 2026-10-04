use reqwest::Method;
use serde_json::json;

use super::{App, call, json_data, list_paged};
use crate::cli::{ApprovalsArgs, ApprovalsSubcommand};
use crate::error::CliError;
use crate::output::{APPROVAL_COLUMNS, Kind, Outcome};

pub async fn run(args: ApprovalsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let preview = match &args.command {
        ApprovalsSubcommand::Preview { data } => Some(json_data(data)?),
        _ => None,
    };
    let context = app.connect().await?;
    let vote = |id: String, verb: &'static str, comment: Option<String>| {
        let body = comment
            .map(|comment| json!({ "comment": comment }))
            .unwrap_or_else(|| json!({}));
        (id, verb, body)
    };
    let (id, verb, body) = match args.command {
        ApprovalsSubcommand::List { status, page } => {
            let team = context.team_id().await?;
            // The API parameter is `statuses` and takes a comma-separated list.
            let query = [("statuses", status.trim().to_string())];
            let value = list_paged(
                &context.api,
                &["teams", &team, "approval-requests"],
                &query,
                &page,
            )
            .await?;
            return Ok(Outcome::new(value, Kind::List(APPROVAL_COLUMNS)));
        }
        ApprovalsSubcommand::Preview { .. } => {
            let team = context.team_id().await?;
            return call(
                &context,
                Method::POST,
                &["teams", &team, "approval-policy-preview"],
                &[],
                preview,
            )
            .await;
        }
        ApprovalsSubcommand::Approve { id, comment } => vote(id, "approve", comment),
        ApprovalsSubcommand::Reject { id, comment } => vote(id, "reject", comment),
        ApprovalsSubcommand::Cancel { id, comment } => vote(id, "cancel", comment),
    };
    call(
        &context,
        Method::POST,
        &["approval-requests", &id, verb],
        &[],
        Some(body),
    )
    .await
}
