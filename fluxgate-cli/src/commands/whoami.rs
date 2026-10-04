use chrono::{DateTime, Utc};
use serde_json::json;

use super::App;
use crate::config::Credential;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    let claims = context.claims.clone();
    let kind = match &claims {
        Some(claims) if claims.is_system_client() => "system_client",
        Some(_) => "user",
        None => "unknown",
    };
    let credential = match &context.settings.credential {
        Credential::Static { source, .. } => format!("static token ({})", source.as_str()),
        Credential::Session { name } => format!("session '{name}'"),
        Credential::None => "none".to_string(),
    };
    let mut value = json!({
        "profile": context.settings.profile,
        "session": context.settings.session,
        "credential": credential,
        "kind": kind,
        "username": claims.as_ref().map(|c| c.username.clone()),
        "id": claims.as_ref().map(|c| c.sub.clone()),
        "isAdmin": claims.as_ref().map(|c| c.is_admin),
        "tokenExpiresAt": claims
            .as_ref()
            .and_then(|c| DateTime::<Utc>::from_timestamp(c.exp, 0))
            .map(|expires| expires.to_rfc3339()),
    });
    if kind == "system_client" {
        // System clients may not list teams; the token names its team.
        value["team"] = json!(context.token_team());
    } else {
        let teams = context.teams().await?;
        value["teams"] = json!(
            teams
                .iter()
                .map(|team| team.name.clone())
                .collect::<Vec<_>>()
        );
        value["team"] = match context.team_id().await {
            Ok(id) => json!(
                teams
                    .iter()
                    .find(|team| team.id.eq_ignore_ascii_case(&id))
                    .map(|team| format!("{} ({})", team.name, team.id))
                    .unwrap_or(id)
            ),
            Err(CliError::Usage(message)) => json!(format!("unresolved: {message}")),
            Err(err) => return Err(err),
        };
    }
    Ok(Outcome::new(value, Kind::Object))
}
