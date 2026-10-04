use chrono::Utc;

use super::App;
use crate::auth::device::device_login;
use crate::auth::password::password_login;
use crate::auth::session::{SessionCache, SessionStore};
use crate::auth::sso_loopback::{LOGIN_WAIT, sso_login};
use crate::cli::LoginArgs;
use crate::config::{Credential, Source};
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
    let sso_slug = match (&args.sso, args.password) {
        (_, true) => None,
        (Some(slug), _) if !slug.trim().is_empty() => Some(slug.trim().to_string()),
        (Some(_), _) => Some(settings.sso_provider.clone().ok_or_else(|| {
            CliError::Usage("pass --sso <slug>, or set sso_provider for the session".into())
        })?),
        (None, false) => settings.sso_provider.clone(),
    };
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
    let timeout = Context::timeout(&settings);
    // Without a usable browser (SSH, containers) SSO falls back to the device code.
    let device = args.use_device_code || (sso_slug.is_some() && args.no_browser);
    let response = match &sso_slug {
        _ if device => device_login(&mut *app.prompter, &url, timeout, !args.no_browser).await?,
        Some(slug) => sso_login(&mut *app.prompter, &url, slug, timeout, LOGIN_WAIT).await?,
        None => {
            let username = match args.username {
                Some(username) => username,
                None => app.prompter.input("Username", None)?,
            };
            password_login(&mut *app.prompter, &url, &username, timeout).await?
        }
    };

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
    if let Credential::Static {
        source: Source::Profile,
        ..
    } = &settings.credential
    {
        outcome.warnings.push(format!(
            "warning: profile '{}' has a static token in the credentials file, which commands use before this session; remove it to use the login",
            settings.profile
        ));
    }

    // A profile url would shadow the session's url.
    let shadowed = settings.url.source == Source::Profile;
    if shadowed {
        files.remove_profile_value(&settings.profile, "url");
    }
    // `--sso <slug>` makes SSO the session's default login.
    let new_provider = args
        .sso
        .as_deref()
        .map(str::trim)
        .filter(|slug| !slug.is_empty() && settings.sso_provider.as_deref() != Some(*slug));
    if let Some(slug) = new_provider {
        files.set_session_value(&session, "sso_provider", slug);
    }
    if new_provider.is_some()
        || shadowed
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
