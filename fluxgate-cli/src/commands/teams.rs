use serde_json::{Value, json};

use super::App;
use crate::cli::{TeamsArgs, TeamsSubcommand};
use crate::context::{find_team, is_uuid, team_names};
use crate::error::CliError;
use crate::output::{Kind, Outcome, TEAM_COLUMNS};

pub async fn run(args: TeamsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    let teams = context.teams().await?;
    match args.command {
        TeamsSubcommand::List => {
            let active = context
                .settings
                .team
                .as_ref()
                .map(|team| team.value.clone());
            let items: Vec<Value> = teams
                .iter()
                .map(|team| {
                    let is_active = active.as_deref().is_some_and(|a| {
                        a.eq_ignore_ascii_case(&team.id) || a.eq_ignore_ascii_case(&team.name)
                    });
                    json!({ "active": is_active, "name": team.name, "id": team.id })
                })
                .collect();
            Ok(Outcome::new(
                json!({ "items": items }),
                Kind::List(TEAM_COLUMNS),
            ))
        }
        TeamsSubcommand::Use { team } => {
            let wanted = team.trim();
            let chosen = if is_uuid(wanted) {
                teams
                    .iter()
                    .find(|t| t.id.eq_ignore_ascii_case(wanted))
                    .cloned()
                    .ok_or_else(|| {
                        CliError::Usage(format!(
                            "team {wanted} not found; available: {}",
                            team_names(&teams)
                        ))
                    })?
            } else {
                find_team(&teams, wanted)?
            };
            let profile = context.settings.profile.clone();
            let mut files = app.files()?;
            files.set_profile_value(&profile, "team", wanted);
            files.save_config(&app.paths)?;
            Ok(Outcome::message(format!(
                "Profile '{profile}' now uses team {} ({})",
                chosen.name, chosen.id
            )))
        }
    }
}
