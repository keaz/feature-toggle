//! Locations of the config file, credentials file and session cache.

use std::path::PathBuf;

use super::Env;
use crate::error::CliError;

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub credentials: PathBuf,
    /// Session caches; always next to the config file.
    pub sessions: PathBuf,
}

impl Paths {
    pub fn from_env(env: &Env) -> Result<Self, CliError> {
        let base = || {
            dirs::home_dir()
                .map(|home| home.join(".fluxgate"))
                .ok_or_else(|| {
                    CliError::Usage(
                        "cannot find the home directory; set FLUXGATE_CONFIG_FILE".into(),
                    )
                })
        };
        let config = match env.get("FLUXGATE_CONFIG_FILE") {
            Some(path) => PathBuf::from(path),
            None => base()?.join("config"),
        };
        let credentials = match env.get("FLUXGATE_SHARED_CREDENTIALS_FILE") {
            Some(path) => PathBuf::from(path),
            None => base()?.join("credentials"),
        };
        let sessions = config
            .parent()
            .map(|dir| dir.join("sessions"))
            .unwrap_or_else(|| PathBuf::from("sessions"));
        Ok(Self { config, credentials, sessions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn env_overrides_file_locations_and_sessions_follow_the_config_file() {
        let env = Env::from_pairs([
            ("FLUXGATE_CONFIG_FILE", "/x/conf"),
            ("FLUXGATE_SHARED_CREDENTIALS_FILE", "/y/creds"),
        ]);
        let paths = Paths::from_env(&env).unwrap();
        assert_eq!(paths.config, Path::new("/x/conf"));
        assert_eq!(paths.credentials, Path::new("/y/creds"));
        assert_eq!(paths.sessions, Path::new("/x/sessions"));
    }

    #[test]
    fn defaults_live_under_dot_fluxgate() {
        let paths = Paths::from_env(&Env::default()).unwrap();
        assert!(paths.config.ends_with(".fluxgate/config"));
        assert!(paths.credentials.ends_with(".fluxgate/credentials"));
        assert!(paths.sessions.ends_with(".fluxgate/sessions"));
    }
}
