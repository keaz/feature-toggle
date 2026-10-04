use super::{App, list_paged};
use crate::cli::{ApprovalsArgs, ApprovalsSubcommand};
use crate::error::CliError;
use crate::output::{APPROVAL_COLUMNS, Kind, Outcome};

pub async fn run(args: ApprovalsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    match args.command {
        ApprovalsSubcommand::List { status, page } => {
            let team = context.team_id().await?;
            // The API parameter is `statuses` and takes a comma-separated list.
            let query = [("statuses", status.trim().to_string())];
            let value = list_paged(&context.api, &["teams", &team, "approval-requests"], &query, &page).await?;
            Ok(Outcome::new(value, Kind::List(APPROVAL_COLUMNS)))
        }
    }
}
