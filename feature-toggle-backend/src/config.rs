use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use log::{info, warn};
use serde::Deserialize;

use crate::cluster::ClusterConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub allowed_origin: String,
    /// Address for Actix-Web HTTP server, e.g., "127.0.0.1:8080"
    pub http_addr: String,
    /// Address for gRPC server, e.g., "0.0.0.0:50051"
    pub grpc_addr: String,
    /// Optional configuration for multi-node replication.
    #[serde(default)]
    pub cluster: ClusterConfig,
    /// Session token lifetimes. A missing `[auth]` section uses the defaults.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Public base URL of this backend as browsers and identity providers reach it,
    /// e.g. `https://fluxgate.example.com`. Used to build the SSO callback URL
    /// (`<public_base_url>/api/v1/auth/sso/<slug>/callback`). When unset, it is
    /// derived from each request (scheme and host, honouring `Forwarded` /
    /// `X-Forwarded-*` headers); set it in production behind a proxy.
    #[serde(default)]
    pub public_base_url: Option<String>,
}

/// Lifetimes of user session tokens (`[auth]` section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Lifetime of an access token (JWT) in minutes.
    pub access_token_ttl_minutes: u32,
    /// Lifetime of a refresh token in days.
    pub refresh_token_ttl_days: u32,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            access_token_ttl_minutes: 30,
            refresh_token_ttl_days: 7,
        }
    }
}

impl AuthConfig {
    /// Raises lifetimes below 1 to 1, with a warning: a zero access-token
    /// lifetime would issue already-expired tokens (and give rotated JWT
    /// secrets no grace), a zero refresh-token lifetime already-expired
    /// refresh tokens.
    pub fn sanitized(self) -> Self {
        let mut auth = self;
        if auth.access_token_ttl_minutes < 1 {
            warn!(
                "auth.access_token_ttl_minutes = {} is below 1; using 1",
                auth.access_token_ttl_minutes
            );
            auth.access_token_ttl_minutes = 1;
        }
        if auth.refresh_token_ttl_days < 1 {
            warn!(
                "auth.refresh_token_ttl_days = {} is below 1; using 1",
                auth.refresh_token_ttl_days
            );
            auth.refresh_token_ttl_days = 1;
        }
        auth
    }

    pub fn access_token_ttl(&self) -> chrono::Duration {
        chrono::Duration::minutes(i64::from(self.access_token_ttl_minutes))
    }

    pub fn refresh_token_ttl(&self) -> chrono::Duration {
        chrono::Duration::days(i64::from(self.refresh_token_ttl_days))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            allowed_origin: "http://localhost:5173".to_string(),
            http_addr: "0.0.0.0:8080".to_string(),
            grpc_addr: "0.0.0.0:50051".to_string(),
            cluster: ClusterConfig::default(),
            auth: AuthConfig::default(),
            public_base_url: None,
        }
    }
}

impl Config {
    /// Load configuration from a TOML file. If not found or invalid, fall back to defaults.
    /// Search order:
    /// 1) Path from env var FEATURE_TOGGLE_CONFIG
    /// 2) feature-toggle-backend/config.toml (relative to workspace root)
    /// 3) config.toml (current working directory)
    pub fn load() -> Self {
        let default = Self::default();

        let candidates = [
            std::env::var("FEATURE_TOGGLE_CONFIG").ok(),
            Some("feature-toggle-backend/config.toml".to_string()),
            Some("config.toml".to_string()),
        ];

        for path_str in candidates.into_iter().flatten() {
            let path = Path::new(&path_str);
            if path.exists() {
                match fs::read_to_string(path) {
                    Ok(content) => match Self::from_toml(&content) {
                        Ok(cfg) => {
                            info!("Loaded configuration from {}", path_str);
                            return cfg;
                        }
                        Err(e) => {
                            warn!(
                                "Failed to parse TOML configuration at {}: {}. Falling back to defaults.",
                                path_str, e
                            );
                        }
                    },
                    Err(e) => {
                        warn!(
                            "Failed to read configuration file {}: {}. Falling back to defaults.",
                            path_str, e
                        );
                    }
                }
            }
        }

        warn!("Using default configuration values.");
        default
    }

    /// Parses a TOML configuration and sanitizes values that must be positive.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(content)?;
        cfg.auth = cfg.auth.sanitized();
        cfg.public_base_url = cfg
            .public_base_url
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty());
        Ok(cfg)
    }

    pub fn grpc_socket_addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        self.grpc_addr.parse::<SocketAddr>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
allowed_origin = "http://localhost:8090"
http_addr = "0.0.0.0:8080"
grpc_addr = "0.0.0.0:50051"
"#;

    #[test]
    fn missing_auth_section_uses_defaults() {
        let cfg: Config = toml::from_str(BASE).unwrap();
        assert_eq!(cfg.auth, AuthConfig::default());
        assert_eq!(cfg.auth.access_token_ttl_minutes, 30);
        assert_eq!(cfg.auth.refresh_token_ttl_days, 7);
        assert_eq!(cfg.auth.access_token_ttl(), chrono::Duration::minutes(30));
        assert_eq!(cfg.auth.refresh_token_ttl(), chrono::Duration::days(7));
    }

    #[test]
    fn auth_section_overrides_defaults_per_key() {
        let cfg: Config =
            toml::from_str(&format!("{BASE}\n[auth]\naccess_token_ttl_minutes = 5\n")).unwrap();
        assert_eq!(cfg.auth.access_token_ttl_minutes, 5);
        assert_eq!(cfg.auth.refresh_token_ttl_days, 7);

        let cfg: Config = toml::from_str(&format!(
            "{BASE}\n[auth]\naccess_token_ttl_minutes = 15\nrefresh_token_ttl_days = 14\n"
        ))
        .unwrap();
        assert_eq!(cfg.auth.access_token_ttl(), chrono::Duration::minutes(15));
        assert_eq!(cfg.auth.refresh_token_ttl(), chrono::Duration::days(14));
    }

    #[test]
    fn auth_lifetimes_below_one_are_raised_to_one() {
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[auth]\naccess_token_ttl_minutes = 0\nrefresh_token_ttl_days = 0\n"
        ))
        .unwrap();
        assert_eq!(cfg.auth.access_token_ttl_minutes, 1);
        assert_eq!(cfg.auth.refresh_token_ttl_days, 1);
        assert_eq!(cfg.auth.access_token_ttl(), chrono::Duration::minutes(1));

        // Valid values are kept as they are.
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[auth]\naccess_token_ttl_minutes = 1\nrefresh_token_ttl_days = 3\n"
        ))
        .unwrap();
        assert_eq!(cfg.auth.access_token_ttl_minutes, 1);
        assert_eq!(cfg.auth.refresh_token_ttl_days, 3);
    }

    #[test]
    fn public_base_url_is_optional_and_normalized() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert_eq!(cfg.public_base_url, None);
        let cfg = Config::from_toml(&format!(
            "public_base_url = \"https://flux.example.com/\"\n{BASE}"
        ))
        .unwrap();
        assert_eq!(
            cfg.public_base_url.as_deref(),
            Some("https://flux.example.com")
        );
        let cfg = Config::from_toml(&format!("public_base_url = \"  \"\n{BASE}")).unwrap();
        assert_eq!(cfg.public_base_url, None);
    }

    #[test]
    fn shipped_config_file_parses_with_default_auth_values() {
        let content = include_str!("../config.toml");
        let cfg = Config::from_toml(content).unwrap();
        assert_eq!(cfg.auth, AuthConfig::default());
    }
}
