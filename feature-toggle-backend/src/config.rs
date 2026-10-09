use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use log::{error, info, warn};
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
    /// TypeSafe Jev judgments. The subsystem is on only when env
    /// `TYPESAFE_API_KEY` is set; the key is never read from this file.
    #[serde(default)]
    pub typesafe: TypesafeConfig,
    /// Jira write-back settings. A missing `[jira]` section uses the defaults.
    #[serde(default)]
    pub jira: JiraConfig,
    /// Rate limits of the public CLI device-login routes.
    #[serde(default)]
    pub device_login: DeviceLoginConfig,
    /// Folder with the built admin UI (`index.html` and its assets). When set,
    /// the backend serves the UI on its HTTP port next to `/api/v1`, with a
    /// `/config.js` that points the UI at the same origin. Unset: no UI routes.
    #[serde(default)]
    pub ui_dir: Option<String>,
}

/// Limits of `POST /auth/device/authorize` and `/auth/device/token`
/// (`[device_login]` section).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DeviceLoginConfig {
    /// `authorize` calls per minute and client address (default 10).
    pub authorize_per_minute: u32,
    /// `token` polls per minute and client address (default 120; a CLI polls
    /// every 5 seconds).
    pub token_per_minute: u32,
    /// Total per route, as a multiple of the per-client limit (default 30).
    pub total_factor: u32,
    /// Addresses of reverse proxies in front of the backend. For requests
    /// from them the client address is read from `Forwarded` /
    /// `X-Forwarded-For`; other requests use the connection's address, so
    /// clients cannot pick their own budget.
    pub trusted_proxies: Vec<std::net::IpAddr>,
}

pub const DEFAULT_DEVICE_AUTHORIZE_PER_MINUTE: u32 = 10;
pub const DEFAULT_DEVICE_TOKEN_PER_MINUTE: u32 = 120;
pub const DEFAULT_DEVICE_TOTAL_FACTOR: u32 = 30;

impl Default for DeviceLoginConfig {
    fn default() -> Self {
        Self {
            authorize_per_minute: DEFAULT_DEVICE_AUTHORIZE_PER_MINUTE,
            token_per_minute: DEFAULT_DEVICE_TOKEN_PER_MINUTE,
            total_factor: DEFAULT_DEVICE_TOTAL_FACTOR,
            trusted_proxies: Vec::new(),
        }
    }
}

impl DeviceLoginConfig {
    /// Raises limits of 0 to the defaults, with a warning: a zero limit would
    /// refuse every CLI device login.
    pub fn sanitized(mut self) -> Self {
        if self.authorize_per_minute == 0 {
            warn!(
                "device_login.authorize_per_minute = 0; using {DEFAULT_DEVICE_AUTHORIZE_PER_MINUTE}"
            );
            self.authorize_per_minute = DEFAULT_DEVICE_AUTHORIZE_PER_MINUTE;
        }
        if self.token_per_minute == 0 {
            warn!("device_login.token_per_minute = 0; using {DEFAULT_DEVICE_TOKEN_PER_MINUTE}");
            self.token_per_minute = DEFAULT_DEVICE_TOKEN_PER_MINUTE;
        }
        if self.total_factor == 0 {
            warn!("device_login.total_factor = 0; using {DEFAULT_DEVICE_TOTAL_FACTOR}");
            self.total_factor = DEFAULT_DEVICE_TOTAL_FACTOR;
        }
        self
    }
}

/// Jira write-back settings (`[jira]` section).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct JiraConfig {
    /// Allow `http://` Jira base URLs for write-back (default false).
    pub allow_insecure_http: bool,
    /// UI origin for remote links; falls back to `allowed_origin`.
    pub ui_base_url: Option<String>,
    /// Sustained inbound events per minute, per integration (default 120).
    pub inbound_per_minute: u32,
    /// Inbound events one integration may send at once (default 60).
    pub inbound_burst: u32,
}

pub const DEFAULT_JIRA_INBOUND_PER_MINUTE: u32 = 120;
pub const DEFAULT_JIRA_INBOUND_BURST: u32 = 60;

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            allow_insecure_http: false,
            ui_base_url: None,
            inbound_per_minute: DEFAULT_JIRA_INBOUND_PER_MINUTE,
            inbound_burst: DEFAULT_JIRA_INBOUND_BURST,
        }
    }
}

impl JiraConfig {
    /// Raises inbound limits of 0 to the defaults, with a warning: a zero
    /// limit would answer 429 to every Jira event.
    pub fn sanitized(mut self) -> Self {
        if self.inbound_per_minute == 0 {
            warn!("jira.inbound_per_minute = 0; using {DEFAULT_JIRA_INBOUND_PER_MINUTE}");
            self.inbound_per_minute = DEFAULT_JIRA_INBOUND_PER_MINUTE;
        }
        if self.inbound_burst == 0 {
            warn!("jira.inbound_burst = 0; using {DEFAULT_JIRA_INBOUND_BURST}");
            self.inbound_burst = DEFAULT_JIRA_INBOUND_BURST;
        }
        self
    }
}

/// [`Config::jira_ui_base_url`] resolved at startup, shared as `web::Data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraUiBaseUrl(pub Option<String>);

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

pub const DEFAULT_TYPESAFE_BASE_URL: &str = "https://api.typesafe.ai";
/// Pinned on purpose: aliases such as `jev-latest` move and change answers.
pub const DEFAULT_TYPESAFE_MODEL: &str = "jev-1.13.0";
const DEFAULT_TYPESAFE_TIMEOUT_MS: u64 = 3000;
const DEFAULT_TYPESAFE_MAX_IN_FLIGHT: usize = 16;

/// TypeSafe System One client settings (`[typesafe]` section).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TypesafeConfig {
    pub base_url: String,
    pub model: String,
    /// Timeout per HTTP attempt.
    pub timeout_ms: u64,
    /// Concurrent requests allowed across all callers. The async judgment
    /// pipeline may use half of them (`judgment::service::async_in_flight`).
    pub max_in_flight: usize,
}

impl Default for TypesafeConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_TYPESAFE_BASE_URL.to_string(),
            model: DEFAULT_TYPESAFE_MODEL.to_string(),
            timeout_ms: DEFAULT_TYPESAFE_TIMEOUT_MS,
            max_in_flight: DEFAULT_TYPESAFE_MAX_IN_FLIGHT,
        }
    }
}

impl TypesafeConfig {
    /// Trims the base URL and replaces values that would break every call
    /// (empty URL or model, zero timeout, zero permits) with the defaults.
    pub fn sanitized(self) -> Self {
        let mut cfg = self;
        cfg.base_url = cfg.base_url.trim().trim_end_matches('/').to_string();
        if cfg.base_url.is_empty() {
            warn!("typesafe.base_url is empty; using {DEFAULT_TYPESAFE_BASE_URL}");
            cfg.base_url = DEFAULT_TYPESAFE_BASE_URL.to_string();
        }
        cfg.model = cfg.model.trim().to_string();
        if cfg.model.is_empty() {
            warn!("typesafe.model is empty; using {DEFAULT_TYPESAFE_MODEL}");
            cfg.model = DEFAULT_TYPESAFE_MODEL.to_string();
        }
        if cfg.timeout_ms == 0 {
            warn!("typesafe.timeout_ms = 0; using {DEFAULT_TYPESAFE_TIMEOUT_MS}");
            cfg.timeout_ms = DEFAULT_TYPESAFE_TIMEOUT_MS;
        }
        if cfg.max_in_flight == 0 {
            warn!("typesafe.max_in_flight = 0; using {DEFAULT_TYPESAFE_MAX_IN_FLIGHT}");
            cfg.max_in_flight = DEFAULT_TYPESAFE_MAX_IN_FLIGHT;
        }
        cfg
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
            typesafe: TypesafeConfig::default(),
            jira: JiraConfig::default(),
            device_login: DeviceLoginConfig::default(),
            ui_dir: None,
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
                            error!(
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
        cfg.typesafe = cfg.typesafe.sanitized();
        cfg.jira = cfg.jira.sanitized();
        cfg.device_login = cfg.device_login.sanitized();
        cfg.jira.ui_base_url = cfg
            .jira
            .ui_base_url
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty());
        cfg.public_base_url = cfg
            .public_base_url
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty());
        cfg.ui_dir = cfg
            .ui_dir
            .map(|dir| dir.trim().to_string())
            .filter(|dir| !dir.is_empty());
        Ok(cfg)
    }

    /// UI base URL for Jira remote links: `[jira] ui_base_url`, else `allowed_origin`
    /// when it is one absolute `http(s)` URL. Trailing `/` trimmed.
    pub fn jira_ui_base_url(&self) -> Option<String> {
        let candidate = self
            .jira
            .ui_base_url
            .as_deref()
            .unwrap_or(&self.allowed_origin);
        let candidate = candidate.trim().trim_end_matches('/');
        let url = reqwest::Url::parse(candidate).ok()?;
        (matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
            .then(|| candidate.to_string())
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
    fn ui_dir_is_optional_and_blank_means_unset() {
        assert_eq!(Config::from_toml(BASE).unwrap().ui_dir, None);
        let cfg = Config::from_toml(&format!("ui_dir = \" ui \"\n{BASE}")).unwrap();
        assert_eq!(cfg.ui_dir.as_deref(), Some("ui"));
        let cfg = Config::from_toml(&format!("ui_dir = \"  \"\n{BASE}")).unwrap();
        assert_eq!(cfg.ui_dir, None);
    }

    #[test]
    fn shipped_config_file_parses_with_default_auth_values() {
        let content = include_str!("../config.toml");
        let cfg = Config::from_toml(content).unwrap();
        assert_eq!(cfg.auth, AuthConfig::default());
    }

    #[test]
    fn missing_typesafe_section_uses_defaults() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert_eq!(cfg.typesafe, TypesafeConfig::default());
        assert_eq!(cfg.typesafe.base_url, "https://api.typesafe.ai");
        assert_eq!(cfg.typesafe.model, "jev-1.13.0");
        assert_eq!(cfg.typesafe.timeout_ms, 3000);
        assert_eq!(cfg.typesafe.max_in_flight, 16);
    }

    #[test]
    fn typesafe_section_overrides_defaults_per_key() {
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[typesafe]\nbase_url = \"https://example.test/\"\ntimeout_ms = 1500\n"
        ))
        .unwrap();
        assert_eq!(cfg.typesafe.base_url, "https://example.test");
        assert_eq!(cfg.typesafe.timeout_ms, 1500);
        assert_eq!(cfg.typesafe.model, "jev-1.13.0");
        assert_eq!(cfg.typesafe.max_in_flight, 16);
    }

    #[test]
    fn typesafe_zero_values_use_defaults() {
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[typesafe]\nbase_url = \"  \"\ntimeout_ms = 0\nmax_in_flight = 0\n"
        ))
        .unwrap();
        assert_eq!(cfg.typesafe, TypesafeConfig::default());
    }

    #[test]
    fn shipped_config_file_has_default_typesafe_values() {
        let content = include_str!("../config.toml");
        let cfg = Config::from_toml(content).unwrap();
        assert_eq!(cfg.typesafe, TypesafeConfig::default());
    }

    #[test]
    fn jira_section_defaults() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert!(!cfg.jira.allow_insecure_http);
        assert_eq!(cfg.jira.ui_base_url, None);
        let cfg =
            Config::from_toml(&format!("{BASE}\n[jira]\nallow_insecure_http = true\n")).unwrap();
        assert!(cfg.jira.allow_insecure_http);
    }

    #[test]
    fn jira_inbound_defaults() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert_eq!(cfg.jira.inbound_per_minute, 120);
        assert_eq!(cfg.jira.inbound_burst, 60);
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[jira]\ninbound_per_minute = 300\ninbound_burst = 20\n"
        ))
        .unwrap();
        assert_eq!(cfg.jira.inbound_per_minute, 300);
        assert_eq!(cfg.jira.inbound_burst, 20);
    }

    #[test]
    fn device_login_defaults_and_overrides() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert_eq!(cfg.device_login.authorize_per_minute, 10);
        assert_eq!(cfg.device_login.token_per_minute, 120);
        assert_eq!(cfg.device_login.total_factor, 30);
        assert!(cfg.device_login.trusted_proxies.is_empty());
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[device_login]\nauthorize_per_minute = 60\ntoken_per_minute = 600\ntotal_factor = 0\ntrusted_proxies = [\"10.0.0.5\", \"::1\"]\n"
        ))
        .unwrap();
        assert_eq!(cfg.device_login.authorize_per_minute, 60);
        assert_eq!(cfg.device_login.token_per_minute, 600);
        assert_eq!(
            cfg.device_login.total_factor, 30,
            "0 is raised to the default"
        );
        assert_eq!(
            cfg.device_login.trusted_proxies,
            vec![
                "10.0.0.5".parse::<std::net::IpAddr>().unwrap(),
                "::1".parse().unwrap()
            ]
        );
        assert!(
            Config::from_toml(&format!(
                "{BASE}\n[device_login]\ntrusted_proxies = [\"proxy\"]\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn zero_inbound_limit_is_raised_to_default() {
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[jira]\ninbound_per_minute = 0\ninbound_burst = 0\n"
        ))
        .unwrap();
        assert_eq!(cfg.jira.inbound_per_minute, 120);
        assert_eq!(cfg.jira.inbound_burst, 60);
    }

    #[test]
    fn jira_ui_base_url_falls_back_to_allowed_origin() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert_eq!(
            cfg.jira_ui_base_url().as_deref(),
            Some("http://localhost:8090")
        );
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[jira]\nui_base_url = \"https://ui.example.com/\"\n"
        ))
        .unwrap();
        assert_eq!(
            cfg.jira_ui_base_url().as_deref(),
            Some("https://ui.example.com")
        );
        let cfg = Config::from_toml(&format!("{BASE}\n[jira]\nui_base_url = \"  \"\n")).unwrap();
        assert_eq!(
            cfg.jira_ui_base_url().as_deref(),
            Some("http://localhost:8090")
        );
    }

    #[test]
    fn jira_ui_base_url_none_for_a_non_url_origin() {
        for origin in ["*", "a.com,b.com", "localhost:5173", ""] {
            let cfg = Config {
                allowed_origin: origin.to_string(),
                ..Config::default()
            };
            assert_eq!(cfg.jira_ui_base_url(), None, "{origin:?}");
        }
    }
}
