use chrono::Utc;

use super::App;
use crate::auth::password::password_login;
use crate::auth::session::{SessionCache, SessionStore};
use crate::cli::LoginArgs;
use crate::config::Source;
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;

pub async fn run(args: LoginArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let mut files = app.files()?;
    let profile = app.profile();
    // `login --profile new --url ...` creates the profile, like configure.
    let explicit_url = app.overrides.url.is_some() || app.env.get("FLUXGATE_URL").is_some();
    if profile != "default" && !files.has_profile(&profile) && explicit_url {
        files.set_profile_value(&profile, "session", &profile);
    }
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
    // A url from a flag or env var moves the session to that server; otherwise
    // the session's own url is used.
    let url = match (&settings.session_url, settings.url.source) {
        (Some(session_url), Source::Profile | Source::Session | Source::Default) => {
            session_url.clone()
        }
        _ => settings.url.value.clone(),
    };
    let username = match args.username {
        Some(username) => username,
        None => app.prompter.input("Username", None)?,
    };
    let timeout = Context::timeout(&settings);
    let response = password_login(&mut *app.prompter, &url, &username, timeout).await?;

    let store = SessionStore::new(app.paths.sessions.clone());
    let mut outcome = Outcome::message(format!(
        "Logged in as {} (session '{session}')",
        response.user.username
    ));
    // End the session being replaced, so its refresh tokens stop working.
    if matches!(store.load(&session), Ok(Some(_))) {
        let old_url = settings.session_url.clone().unwrap_or_else(|| url.clone());
        let _ = store
            .revoke(&session, &settings.profile, &old_url, timeout)
            .await;
    }
    store.save(&session, &SessionCache::from_login(&response, Utc::now()))?;

    // A profile url would shadow the session's url.
    let shadowed = settings.url.source == Source::Profile;
    if shadowed {
        files.remove_profile_value(&settings.profile, "url");
    }
    if shadowed
        || settings.session.is_none()
        || settings.session_url.as_deref() != Some(url.as_str())
    {
        if let Some(old) = &settings.session_url
            && old != &url
        {
            outcome.warnings.push(format!(
                "warning: session '{session}' now points at {url} (was {old}); profiles that use it follow"
            ));
        }
        files.set_profile_value(&settings.profile, "session", &session);
        files.set_session_value(&session, "url", &url);
        files.save_config(&app.paths)?;
    }
    Ok(outcome)
}
