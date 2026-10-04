use super::{App, list_paged};
use crate::cli::{FlagsArgs, FlagsSubcommand};
use crate::context::is_uuid;
use crate::error::CliError;
use crate::output::{FEATURE_COLUMNS, Kind, Outcome};

pub async fn run(args: FlagsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    match args.command {
        FlagsSubcommand::List { page } => {
            let team = context.team_id().await?;
            let value = list_paged(&context.api, &["teams", &team, "features"], &[], &page).await?;
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
    }
}
