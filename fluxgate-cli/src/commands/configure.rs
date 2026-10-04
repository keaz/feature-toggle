use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};

use super::App;
use crate::api::ApiClient;
use crate::auth::device::device_login;
use crate::auth::password::password_login;
use crate::auth::session::{SessionCache, SessionStore};
use crate::auth::sso_loopback::{LOGIN_WAIT, sso_login};
use crate::cli::{ConfigureArgs, ConfigureSubcommand};
use crate::config::files::PROFILE_KEYS;
use crate::config::resolve::{DEFAULT_TIMEOUT_SECS, DEFAULT_URL};
use crate::config::{Credential, Env, Overrides, Resolved, Source, mask_token, resolve};
use crate::context::Context;
use crate::error::CliError;
use crate::output::{CONFIG_COLUMNS, Kind, Outcome, OutputFormat, PROFILE_COLUMNS};

pub const LOGIN_PASSWORD: &str = "Log in with username and password";
pub const LOGIN_SSO: &str = "Single sign-on (SSO)";
pub const LOGIN_DEVICE: &str = "Approve in a browser (device code)";
pub const LOGIN_TOKEN: &str = "Static token (system client)";

pub async fn run(args: ConfigureArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    match args.command {
        None => interactive(app).await,
        Some(ConfigureSubcommand::Set { key, value }) => set(app, &key, &value),
        Some(ConfigureSubcommand::Get { key }) => get(app, &key),
        Some(ConfigureSubcommand::List) => list(app),
        Some(ConfigureSubcommand::ListProfiles) => list_profiles(app),
    }
}

fn set(app: &App<'_>, key: &str, value: &str) -> Result<Outcome, CliError> {
    let profile = app.profile();
    let mut files = app.files()?;
    let value = value.trim();
    match key {
        "token" | "edge_key" => {
            files.set_credential_value(&profile, key, value);
            files.save_credentials(&app.paths)?;
        }
        _ if PROFILE_KEYS.contains(&key) => {
            if key == "url" {
                ApiClient::new(value, None, Duration::from_secs(DEFAULT_TIMEOUT_SECS))?;
            }
            if key == "output" {
                value.parse::<OutputFormat>().map_err(CliError::Usage)?;
            }
            if key == "timeout" {
                value
                    .parse::<u64>()
                    .map_err(|_| CliError::Usage("timeout must be a number of seconds".into()))?;
            }
            files.set_profile_value(&profile, key, value);
            files.save_config(&app.paths)?;
        }
        other => {
            return Err(CliError::Usage(format!(
                "unknown key '{other}'; expected one of: {}, token, edge_key",
                PROFILE_KEYS.join(", ")
            )));
        }
    }
    Ok(Outcome::message(format!(
        "Set {key} for profile '{profile}'"
    )))
}

fn get(app: &App<'_>, key: &str) -> Result<Outcome, CliError> {
    let profile = app.profile();
    let files = app.files()?;
    let value = if key == "token" || key == "edge_key" {
        files.credential_value(&profile, key).map(mask_token)
    } else {
        files.profile_value(&profile, key).map(str::to_string)
    };
    value
        .map(Outcome::message)
        .ok_or_else(|| CliError::Other(format!("{key} is not set for profile '{profile}'")))
}

fn row(name: &str, value: &str, source: &str) -> Value {
    json!({ "name": name, "value": value, "source": source })
}

fn optional_row(name: &str, value: Option<&Resolved<String>>) -> Value {
    match value {
        Some(resolved) => row(name, &resolved.value, resolved.source.as_str()),
        None => row(name, "-", "unset"),
    }
}

fn list(app: &App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let settings = app.settings(&files)?;
    let profile_source = if app.overrides.profile.is_some() {
        Source::Flag
    } else if app.env.get("FLUXGATE_PROFILE").is_some() {
        Source::Env
    } else {
        Source::Default
    };
    let token = match &settings.credential {
        Credential::Static { token, source } => row(
            "token",
            &format!("{} (static)", mask_token(token)),
            source.as_str(),
        ),
        Credential::Session { name } => {
            let status = match SessionStore::new(app.paths.sessions.clone()).load(name) {
                Ok(Some(cache)) => format!(
                    "session '{name}', expires {}",
                    cache.expires_at.to_rfc3339()
                ),
                Ok(None) => format!("session '{name}', not logged in"),
                Err(_) => format!("session '{name}', cache damaged"),
            };
            row("token", &status, "session")
        }
        Credential::None => row("token", "-", "unset"),
    };
    let items = vec![
        row("profile", &settings.profile, profile_source.as_str()),
        row("url", &settings.url.value, settings.url.source.as_str()),
        optional_row("team", settings.team.as_ref()),
        optional_row("environment", settings.environment.as_ref()),
        row(
            "output",
            settings.output.value.as_str(),
            settings.output.source.as_str(),
        ),
        row(
            "timeout",
            &settings.timeout.value.to_string(),
            settings.timeout.source.as_str(),
        ),
        match &settings.session {
            Some(session) => row("session", session, "profile"),
            None => row("session", "-", "unset"),
        },
        token,
    ];
    Ok(Outcome::new(
        json!({ "items": items }),
        Kind::List(CONFIG_COLUMNS),
    ))
}

fn list_profiles(app: &App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let active = app.profile();
    let items: Vec<Value> = files
        .profile_names()
        .into_iter()
        .map(|name| {
            let session = files.profile_value(&name, "session");
            let url = files
                .profile_value(&name, "url")
                .or_else(|| session.and_then(|s| files.session_value(s, "url")));
            json!({
                "active": name == active,
                "name": name,
                "session": session,
                "team": files.profile_value(&name, "team"),
                "url": url,
            })
        })
        .collect();
    Ok(Outcome::new(
        json!({ "items": items }),
        Kind::List(PROFILE_COLUMNS),
    ))
}

async fn interactive(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let profile = app.profile();
    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    let mut files = app.files()?;
    let current_url = files
        .profile_value(&profile, "url")
        .or_else(|| {
            files
                .profile_value(&profile, "session")
                .and_then(|s| files.session_value(s, "url"))
        })
        .unwrap_or(DEFAULT_URL)
        .to_string();
    let url = app
        .prompter
        .input("FluxGate API URL", Some(&current_url))?
        .trim()
        .trim_end_matches('/')
        .to_string();
    ApiClient::new(&url, None, timeout)?;

    let mut warnings = Vec::new();
    let methods = vec![
        LOGIN_PASSWORD.to_string(),
        LOGIN_SSO.to_string(),
        LOGIN_DEVICE.to_string(),
        LOGIN_TOKEN.to_string(),
    ];
    let method = app.prompter.select("How do you sign in", &methods)?;
    if method < 3 {
        let session = files
            .profile_value(&profile, "session")
            .unwrap_or(profile.as_str())
            .to_string();
        // Log in before writing anything, so a failed login changes nothing.
        let (response, provider) = if method == 0 {
            let username = app.prompter.input("Username", None)?;
            let response = password_login(&mut *app.prompter, &url, &username, timeout).await?;
            (response, None)
        } else if method == 1 {
            let slug = choose_sso_provider(app, &url, timeout).await?;
            let response = sso_login(&mut *app.prompter, &url, &slug, timeout, LOGIN_WAIT).await?;
            (response, Some(slug))
        } else {
            (
                device_login(&mut *app.prompter, &url, timeout, true).await?,
                None,
            )
        };
        if files
            .session_value(&session, "url")
            .is_some_and(|old| old.trim_end_matches('/') != url)
        {
            let others: Vec<String> = files
                .profile_names()
                .into_iter()
                .filter(|name| {
                    name != &profile
                        && files.profile_value(name, "session") == Some(session.as_str())
                })
                .map(|name| format!("'{name}'"))
                .collect();
            if !others.is_empty() {
                warnings.push(format!(
                    "warning: session '{session}' now points at {url}; it is also used by profile {}",
                    others.join(", ")
                ));
            }
        }
        files.set_profile_value(&profile, "session", &session);
        files.set_session_value(&session, "url", &url);
        match &provider {
            Some(slug) => files.set_session_value(&session, "sso_provider", slug),
            None => files.remove_session_value(&session, "sso_provider"),
        }
        // The session holds the url; a profile url would shadow it.
        files.remove_profile_value(&profile, "url");
        files.save_config(&app.paths)?;
        SessionStore::new(app.paths.sessions.clone())
            .save(&session, &SessionCache::from_login(&response, Utc::now()))?;
    } else {
        let token = app.prompter.secret("Token")?;
        files.set_credential_token(&profile, token.trim());
        files.set_profile_value(&profile, "url", &url);
        files.save_credentials(&app.paths)?;
        files.save_config(&app.paths)?;
    }

    // Check the profile as written: env and flag credentials or URLs (such as
    // an exported FLUXGATE_TOKEN) must not decide its team or environment.
    let written = Overrides {
        profile: Some(profile.clone()),
        ..Overrides::default()
    };
    let settings = resolve(&app.files()?, &Env::default(), &written, app.is_tty)?;
    let context = Context::connect(settings, app.paths.clone()).await?;
    let mut files = app.files()?;
    let team_id = match context.token_team() {
        Some(token_team) => {
            files.set_profile_value(&profile, "team", token_team);
            Some(token_team.to_string())
        }
        None => {
            let teams = context.teams().await?;
            if teams.is_empty() {
                None
            } else {
                let names: Vec<String> = teams.iter().map(|team| team.name.clone()).collect();
                let chosen = &teams[app.prompter.select("Team", &names)?];
                // Store the name unless another team has the same name.
                let duplicate = teams
                    .iter()
                    .filter(|t| t.name.eq_ignore_ascii_case(&chosen.name))
                    .count()
                    > 1;
                files.set_profile_value(
                    &profile,
                    "team",
                    if duplicate { &chosen.id } else { &chosen.name },
                );
                Some(chosen.id.clone())
            }
        }
    };
    if let Some(team_id) = team_id {
        let environments = context.environments(&team_id).await?;
        if !environments.is_empty() {
            let names: Vec<String> = environments.iter().map(|e| e.name.clone()).collect();
            let chosen = app.prompter.select("Default environment", &names)?;
            files.set_profile_value(&profile, "environment", &environments[chosen].name);
        }
    }
    let outputs = vec!["table".to_string(), "json".to_string(), "text".to_string()];
    let output = app.prompter.select("Default output", &outputs)?;
    files.set_profile_value(&profile, "output", &outputs[output]);
    files.save_config(&app.paths)?;
    let mut outcome = Outcome::message(format!(
        "Profile '{profile}' saved to {}",
        app.paths.config.display()
    ));
    outcome.warnings = warnings;
    Ok(outcome)
}

/// Asks which of the backend's enabled SSO providers to use; returns its slug.
async fn choose_sso_provider(
    app: &mut App<'_>,
    url: &str,
    timeout: Duration,
) -> Result<String, CliError> {
    let value = ApiClient::new(url, None, timeout)?
        .get(&["auth", "sso", "providers"], &[])
        .await?;
    let providers: Vec<(String, String)> = value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let slug = item.get("slug")?.as_str()?.to_string();
                    let name = item
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or(&slug)
                        .to_string();
                    Some((slug, name))
                })
                .collect()
        })
        .unwrap_or_default();
    if providers.is_empty() {
        return Err(CliError::Usage(format!(
            "{url} has no SSO providers enabled"
        )));
    }
    let names: Vec<String> = providers.iter().map(|(_, name)| name.clone()).collect();
    let chosen = app.prompter.select("SSO provider", &names)?;
    Ok(providers[chosen].0.clone())
}
