//! Which profile, settings and credentials a command uses.

use serde::Serialize;

use super::{ConfigFiles, Env};
use crate::error::CliError;
use crate::output::OutputFormat;

pub const DEFAULT_URL: &str = "http://localhost:8080/api/v1";
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Flag,
    Env,
    Profile,
    Session,
    Default,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Flag => "flag",
            Source::Env => "env",
            Source::Profile => "profile",
            Source::Session => "session",
            Source::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: Source,
}

/// Values given as global command line flags.
#[derive(Clone, Default)]
pub struct Overrides {
    pub profile: Option<String>,
    pub url: Option<String>,
    pub team: Option<String>,
    pub environment: Option<String>,
    pub output: Option<OutputFormat>,
    pub token: Option<String>,
    pub timeout: Option<u64>,
}

impl std::fmt::Debug for Overrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overrides")
            .field("profile", &self.profile)
            .field("url", &self.url)
            .field("team", &self.team)
            .field("environment", &self.environment)
            .field("output", &self.output)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// A bearer token from a flag, env var or the credentials file.
    Static {
        token: String,
        source: Source,
    },
    /// A cached login session.
    Session {
        name: String,
    },
    None,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Static { source, .. } => {
                write!(f, "Static {{ token: <redacted>, source: {source:?} }}")
            }
            Credential::Session { name } => write!(f, "Session {{ name: {name:?} }}"),
            Credential::None => write!(f, "None"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub profile: String,
    pub url: Resolved<String>,
    pub team: Option<Resolved<String>>,
    pub environment: Option<Resolved<String>>,
    pub output: Resolved<OutputFormat>,
    pub timeout: Resolved<u64>,
    pub session: Option<String>,
    /// The url of `session`; its tokens are valid only there.
    pub session_url: Option<String>,
    pub sso_provider: Option<String>,
    pub credential: Credential,
}

/// `--profile`, then `FLUXGATE_PROFILE`, then `default`.
pub fn selected_profile(overrides: &Overrides, env: &Env) -> String {
    overrides
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .or_else(|| env.get("FLUXGATE_PROFILE"))
        .unwrap_or("default")
        .to_string()
}

pub fn resolve(
    files: &ConfigFiles,
    env: &Env,
    overrides: &Overrides,
    is_tty: bool,
) -> Result<Settings, CliError> {
    let profile = selected_profile(overrides, env);
    // A named profile that does not exist must not fall back to default, which
    // could point at another environment.
    if profile != "default" && !files.has_profile(&profile) {
        return Err(CliError::Usage(format!(
            "profile '{profile}' not found: run fluxgate configure --profile {profile}"
        )));
    }
    let session = files.profile_value(&profile, "session").map(str::to_string);

    let pick = |flag: Option<String>, env_keys: &[&str], key: &str| -> Option<Resolved<String>> {
        if let Some(value) = flag.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            return Some(Resolved {
                value: value.to_string(),
                source: Source::Flag,
            });
        }
        if let Some(value) = env.first(env_keys) {
            return Some(Resolved {
                value: value.to_string(),
                source: Source::Env,
            });
        }
        files.profile_value(&profile, key).map(|value| Resolved {
            value: value.to_string(),
            source: Source::Profile,
        })
    };

    let url = pick(overrides.url.clone(), &["FLUXGATE_URL"], "url")
        .or_else(|| {
            session
                .as_deref()
                .and_then(|name| files.session_value(name, "url"))
                .map(|value| Resolved {
                    value: value.to_string(),
                    source: Source::Session,
                })
        })
        .unwrap_or(Resolved {
            value: DEFAULT_URL.to_string(),
            source: Source::Default,
        });
    let url = Resolved {
        value: url.value.trim_end_matches('/').to_string(),
        source: url.source,
    };

    let team = pick(
        overrides.team.clone(),
        &["FLUXGATE_TEAM", "FLUXGATE_TEAM_ID"],
        "team",
    );
    let environment = pick(
        overrides.environment.clone(),
        &["FLUXGATE_ENVIRONMENT", "FLUXGATE_ENVIRONMENT_ID"],
        "environment",
    );

    let output = match pick(
        overrides.output.map(|o| o.as_str().to_string()),
        &["FLUXGATE_OUTPUT"],
        "output",
    ) {
        Some(raw) => Resolved {
            value: raw.value.parse::<OutputFormat>().map_err(CliError::Usage)?,
            source: raw.source,
        },
        None => Resolved {
            value: if is_tty {
                OutputFormat::Table
            } else {
                OutputFormat::Json
            },
            source: Source::Default,
        },
    };

    let timeout = match pick(
        overrides.timeout.map(|t| t.to_string()),
        &["FLUXGATE_TIMEOUT"],
        "timeout",
    ) {
        Some(raw) => Resolved {
            value: raw.value.parse::<u64>().map_err(|_| {
                CliError::Usage(format!("invalid timeout '{}': expected seconds", raw.value))
            })?,
            source: raw.source,
        },
        None => Resolved {
            value: DEFAULT_TIMEOUT_SECS,
            source: Source::Default,
        },
    };

    let credential = if let Some(token) = overrides
        .token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Credential::Static {
            token: token.to_string(),
            source: Source::Flag,
        }
    } else if let Some(token) = env.get("FLUXGATE_TOKEN") {
        Credential::Static {
            token: token.to_string(),
            source: Source::Env,
        }
    } else if let Some(token) = files.credential_token(&profile) {
        Credential::Static {
            token: token.to_string(),
            source: Source::Profile,
        }
    } else if let Some(name) = &session {
        Credential::Session { name: name.clone() }
    } else {
        Credential::None
    };

    let session_url = session
        .as_deref()
        .and_then(|name| files.session_value(name, "url"))
        .map(|url| url.trim_end_matches('/').to_string());
    let sso_provider = session
        .as_deref()
        .and_then(|name| files.session_value(name, "sso_provider"))
        .map(str::to_string);

    Ok(Settings {
        profile,
        url,
        team,
        environment,
        output,
        timeout,
        session,
        session_url,
        sso_provider,
        credential,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ini::Ini;

    const CONFIG: &str = "[default]\nsession = corp\nteam = payments\noutput = text\n\n[profile prod]\nsession = corp\nteam = checkout\nenvironment = production\n\n[profile ci]\nurl = https://ci.example.com/api/v1/\n\n[session corp]\nurl = https://fg.example.com/api/v1\nsso_provider = okta\n";
    const CREDS: &str = "[ci]\ntoken = static-ci\n";

    fn files() -> ConfigFiles {
        ConfigFiles {
            config: Ini::load_from_str(CONFIG).unwrap(),
            credentials: Ini::load_from_str(CREDS).unwrap(),
            warnings: Vec::new(),
        }
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn defaults_without_files() {
        let s = resolve(
            &ConfigFiles::empty(),
            &Env::default(),
            &Overrides::default(),
            false,
        )
        .unwrap();
        assert_eq!(s.profile, "default");
        assert_eq!(
            s.url,
            Resolved {
                value: DEFAULT_URL.to_string(),
                source: Source::Default
            }
        );
        assert_eq!(s.team, None);
        assert_eq!(s.output.value, OutputFormat::Json);
        assert_eq!(s.timeout.value, 30);
        assert_eq!(s.credential, Credential::None);
        let tty = resolve(
            &ConfigFiles::empty(),
            &Env::default(),
            &Overrides::default(),
            true,
        )
        .unwrap();
        assert_eq!(tty.output.value, OutputFormat::Table);
    }

    #[test]
    fn profile_values_and_session_url() {
        let s = resolve(&files(), &Env::default(), &Overrides::default(), false).unwrap();
        assert_eq!(
            s.url,
            Resolved {
                value: "https://fg.example.com/api/v1".into(),
                source: Source::Session
            }
        );
        assert_eq!(
            s.team,
            Some(Resolved {
                value: "payments".into(),
                source: Source::Profile
            })
        );
        assert_eq!(
            s.output,
            Resolved {
                value: OutputFormat::Text,
                source: Source::Profile
            }
        );
        assert_eq!(
            s.credential,
            Credential::Session {
                name: "corp".into()
            }
        );
        assert_eq!(s.session.as_deref(), Some("corp"));
        assert_eq!(s.sso_provider.as_deref(), Some("okta"));
    }

    #[test]
    fn flag_beats_env_beats_profile() {
        let overrides = Overrides {
            team: Some("flag-team".into()),
            ..Overrides::default()
        };
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_TEAM", "env-team")]),
            &overrides,
            false,
        )
        .unwrap();
        assert_eq!(
            s.team,
            Some(Resolved {
                value: "flag-team".into(),
                source: Source::Flag
            })
        );
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_TEAM", "env-team")]),
            &Overrides::default(),
            false,
        )
        .unwrap();
        assert_eq!(
            s.team,
            Some(Resolved {
                value: "env-team".into(),
                source: Source::Env
            })
        );
    }

    #[test]
    fn legacy_env_names_still_work_and_new_names_win() {
        let s = resolve(
            &ConfigFiles::empty(),
            &env(&[
                ("FLUXGATE_TEAM_ID", "t-old"),
                ("FLUXGATE_ENVIRONMENT_ID", "e-old"),
            ]),
            &Overrides::default(),
            false,
        )
        .unwrap();
        assert_eq!(s.team.unwrap().value, "t-old");
        assert_eq!(s.environment.unwrap().value, "e-old");
        let s = resolve(
            &ConfigFiles::empty(),
            &env(&[("FLUXGATE_TEAM_ID", "t-old"), ("FLUXGATE_TEAM", "t-new")]),
            &Overrides::default(),
            false,
        )
        .unwrap();
        assert_eq!(s.team.unwrap().value, "t-new");
    }

    #[test]
    fn profile_comes_from_flag_then_env() {
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_PROFILE", "prod")]),
            &Overrides::default(),
            false,
        )
        .unwrap();
        assert_eq!(s.profile, "prod");
        assert_eq!(s.team.unwrap().value, "checkout");
        assert_eq!(s.environment.unwrap().value, "production");
        let overrides = Overrides {
            profile: Some("ci".into()),
            ..Overrides::default()
        };
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_PROFILE", "prod")]),
            &overrides,
            false,
        )
        .unwrap();
        assert_eq!(s.profile, "ci");
    }

    #[test]
    fn missing_profile_is_a_usage_error_naming_it() {
        let err = resolve(
            &files(),
            &env(&[("FLUXGATE_PROFILE", "nope")]),
            &Overrides::default(),
            false,
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        assert!(err.to_string().contains("'nope'"));
    }

    #[test]
    fn credential_order_is_flag_env_file_session() {
        let ci = Overrides {
            profile: Some("ci".into()),
            ..Overrides::default()
        };
        let s = resolve(&files(), &Env::default(), &ci, false).unwrap();
        assert_eq!(
            s.credential,
            Credential::Static {
                token: "static-ci".into(),
                source: Source::Profile
            }
        );
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_TOKEN", "from-env")]),
            &ci,
            false,
        )
        .unwrap();
        assert_eq!(
            s.credential,
            Credential::Static {
                token: "from-env".into(),
                source: Source::Env
            }
        );
        let flag = Overrides {
            token: Some("from-flag".into()),
            ..ci
        };
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_TOKEN", "from-env")]),
            &flag,
            false,
        )
        .unwrap();
        assert_eq!(
            s.credential,
            Credential::Static {
                token: "from-flag".into(),
                source: Source::Flag
            }
        );
    }

    #[test]
    fn url_trailing_slash_is_trimmed() {
        let ci = Overrides {
            profile: Some("ci".into()),
            ..Overrides::default()
        };
        let s = resolve(&files(), &Env::default(), &ci, false).unwrap();
        assert_eq!(s.url.value, "https://ci.example.com/api/v1");
    }

    #[test]
    fn invalid_output_and_timeout_are_usage_errors() {
        let err = resolve(
            &ConfigFiles::empty(),
            &env(&[("FLUXGATE_OUTPUT", "yaml")]),
            &Overrides::default(),
            false,
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        let err = resolve(
            &ConfigFiles::empty(),
            &env(&[("FLUXGATE_TIMEOUT", "soon")]),
            &Overrides::default(),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid timeout 'soon'"));
    }

    #[test]
    fn credential_debug_hides_the_token() {
        let shown = format!(
            "{:?}",
            Credential::Static {
                token: "secret-token".into(),
                source: Source::Env
            }
        );
        assert!(!shown.contains("secret-token"));
    }

    #[test]
    fn selected_profile_defaults_to_default() {
        assert_eq!(
            selected_profile(&Overrides::default(), &Env::default()),
            "default"
        );
        assert_eq!(
            selected_profile(&Overrides::default(), &env(&[("FLUXGATE_PROFILE", "prod")])),
            "prod"
        );
    }

    #[test]
    fn overrides_debug_hides_the_token() {
        let overrides = Overrides {
            token: Some("secret-token".into()),
            ..Overrides::default()
        };
        assert!(!format!("{overrides:?}").contains("secret-token"));
    }

    #[test]
    fn settings_carry_the_session_url() {
        let s = resolve(
            &files(),
            &env(&[("FLUXGATE_URL", "https://other.example.com/api/v1")]),
            &Overrides::default(),
            false,
        )
        .unwrap();
        assert_eq!(
            s.session_url.as_deref(),
            Some("https://fg.example.com/api/v1")
        );
        assert_eq!(s.url.value, "https://other.example.com/api/v1");
    }
}
