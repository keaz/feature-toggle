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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            allowed_origin: "http://localhost:5173".to_string(),
            http_addr: "0.0.0.0:8080".to_string(),
            grpc_addr: "0.0.0.0:50051".to_string(),
            cluster: ClusterConfig::default(),
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
                    Ok(content) => match toml::from_str::<Config>(&content) {
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

    pub fn grpc_socket_addr(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        self.grpc_addr.parse::<SocketAddr>()
    }
}
