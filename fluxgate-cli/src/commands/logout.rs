use super::App;
use crate::auth::session::SessionStore;
use crate::cli::LogoutArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;

pub async fn run(args: LogoutArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let settings = app.settings(&files)?;
    let store = SessionStore::new(app.paths.sessions.clone());
    let names = if args.all {
        store.names()?
    } else {
        vec![settings.session.clone().ok_or_else(|| {
            CliError::Usage(format!(
                "profile '{}' has no login session",
                settings.profile
            ))
        })?]
    };

    let mut warnings = Vec::new();
    let mut ended = Vec::new();
    for name in names {
        match store.load(&name) {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            // A damaged cache cannot be revoked; just remove it.
            Err(_) => {
                store.delete(&name)?;
                warnings.push(format!("warning: removed damaged session cache '{name}'"));
                continue;
            }
        }
        let url = files
            .session_value(&name, "url")
            .map(str::to_string)
            .unwrap_or_else(|| settings.url.value.clone());
        if let Err(err) = store
            .revoke(&name, &settings.profile, &url, Context::timeout(&settings))
            .await
        {
            warnings.push(format!(
                "warning: server logout failed for session '{name}': {err}"
            ));
        }
        store.delete(&name)?;
        ended.push(format!("'{name}'"));
    }

    let mut outcome = Outcome::message(if ended.is_empty() {
        "No active sessions".to_string()
    } else {
        format!("Logged out of {}", ended.join(", "))
    });
    outcome.warnings = warnings;
    Ok(outcome)
}
