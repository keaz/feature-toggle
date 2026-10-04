use chrono::Utc;

use super::App;
use crate::auth::password::password_login;
use crate::auth::session::{SessionCache, SessionStore};
use crate::cli::LoginArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;

pub async fn run(args: LoginArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let mut files = app.files()?;
    let settings = app.settings(&files)?;
    if let Some(provider) = &settings.sso_provider
        && !args.password
    {
        return Err(CliError::Usage(format!(
            "this session uses SSO provider '{provider}', which this version cannot log in to: run fluxgate login --password"
        )));
    }
    // A profile without a session gets one named after the profile.
    let session = settings
        .session
        .clone()
        .unwrap_or_else(|| settings.profile.clone());
    let username = match args.username {
        Some(username) => username,
        None => app.prompter.input("Username", None)?,
    };
    let response = password_login(
        &mut *app.prompter,
        &settings.url.value,
        &username,
        Context::timeout(&settings),
    )
    .await?;
    SessionStore::new(app.paths.sessions.clone())
        .save(&session, &SessionCache::from_login(&response, Utc::now()))?;
    if settings.session.is_none() {
        files.set_profile_value(&settings.profile, "session", &session);
        files.set_session_value(&session, "url", &settings.url.value);
        files.save_config(&app.paths)?;
    }
    Ok(Outcome::message(format!(
        "Logged in as {} (session '{session}')",
        response.user.username
    )))
}
