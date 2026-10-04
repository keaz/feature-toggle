use serde_json::json;

use super::App;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    let team_id = context.team_id().await?;
    // System-client tokens see no teams in /teams; the id alone is still useful.
    let team = context
        .teams()
        .await
        .ok()
        .and_then(|teams| {
            teams
                .into_iter()
                .find(|team| team.id.eq_ignore_ascii_case(&team_id))
        })
        .map(|team| json!(team))
        .unwrap_or_else(|| json!({ "id": team_id }));
    let environments = context
        .api
        .get_all_pages(&["teams", &team_id, "environments"], &[])
        .await?;
    let features = context
        .api
        .get_all_pages(&["teams", &team_id, "features"], &[])
        .await?;
    Ok(Outcome::new(
        json!({ "team": team, "environments": environments, "features": features }),
        Kind::Document,
    ))
}
