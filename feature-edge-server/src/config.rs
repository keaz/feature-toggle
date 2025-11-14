use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConfig {
    /// Backend gRPC server address
    pub backend_grpc: String,

    /// HTTP server bind address
    pub http_addr: String,

    /// Client ID for backend authentication
    pub client_id: String,

    /// Client secret for backend authentication
    pub client_secret: String,

    /// gRPC connection settings
    #[serde(default)]
    pub grpc: GrpcConfig,

    /// Flush interval settings
    #[serde(default)]
    pub flush: FlushConfig,

    /// Retry settings
    #[serde(default)]
    pub retry: RetryConfig,

    /// Cache settings
    #[serde(default)]
    pub cache: CacheConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcConfig {
    /// Connection timeout in seconds
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,

    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// TCP keepalive interval in seconds
    #[serde(default = "default_tcp_keepalive")]
    pub tcp_keepalive_secs: u64,

    /// HTTP/2 keepalive interval in seconds
    #[serde(default = "default_http2_keepalive")]
    pub http2_keepalive_secs: u64,

    /// Keep connection alive even when idle
    #[serde(default = "default_true")]
    pub keep_alive_while_idle: bool,

    /// Maximum concurrent requests
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: usize,

    /// Enable TCP_NODELAY
    #[serde(default = "default_true")]
    pub tcp_nodelay: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushConfig {
    /// User assignment flush interval in seconds
    #[serde(default = "default_assignment_flush")]
    pub assignment_flush_secs: u64,

    /// Evaluation events flush interval in seconds
    #[serde(default = "default_evaluation_flush")]
    pub evaluation_flush_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Base delay for exponential backoff in milliseconds
    #[serde(default = "default_base_delay")]
    pub base_delay_ms: u64,

    /// Maximum retry attempts for direct gRPC calls
    #[serde(default = "default_max_attempts")]
    pub max_attempts: usize,

    /// Initial delay for stream reconnection in seconds
    #[serde(default = "default_stream_initial_delay")]
    pub stream_initial_delay_secs: u64,

    /// Maximum delay for stream reconnection in seconds
    #[serde(default = "default_stream_max_delay")]
    pub stream_max_delay_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum number of features to cache (LRU eviction when exceeded)
    #[serde(default = "default_max_capacity")]
    pub max_capacity: u64,
}

fn default_max_capacity() -> u64 {
    10000
}

// Default value functions
fn default_connect_timeout() -> u64 {
    5
}
fn default_timeout() -> u64 {
    10
}
fn default_tcp_keepalive() -> u64 {
    30
}
fn default_http2_keepalive() -> u64 {
    20
}
fn default_true() -> bool {
    true
}
fn default_concurrency_limit() -> usize {
    256
}
fn default_assignment_flush() -> u64 {
    10
}
fn default_evaluation_flush() -> u64 {
    30
}
fn default_base_delay() -> u64 {
    500
}
fn default_max_attempts() -> usize {
    3
}
fn default_stream_initial_delay() -> u64 {
    1
}
fn default_stream_max_delay() -> u64 {
    30
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_connect_timeout(),
            timeout_secs: default_timeout(),
            tcp_keepalive_secs: default_tcp_keepalive(),
            http2_keepalive_secs: default_http2_keepalive(),
            keep_alive_while_idle: default_true(),
            concurrency_limit: default_concurrency_limit(),
            tcp_nodelay: default_true(),
        }
    }
}

impl Default for FlushConfig {
    fn default() -> Self {
        Self {
            assignment_flush_secs: default_assignment_flush(),
            evaluation_flush_secs: default_evaluation_flush(),
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            base_delay_ms: default_base_delay(),
            max_attempts: default_max_attempts(),
            stream_initial_delay_secs: default_stream_initial_delay(),
            stream_max_delay_secs: default_stream_max_delay(),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: default_max_capacity(),
        }
    }
}

impl GrpcConfig {
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_secs)
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    pub fn tcp_keepalive(&self) -> Option<Duration> {
        Some(Duration::from_secs(self.tcp_keepalive_secs))
    }

    pub fn http2_keepalive(&self) -> Duration {
        Duration::from_secs(self.http2_keepalive_secs)
    }
}

impl FlushConfig {
    pub fn assignment_flush_interval(&self) -> Duration {
        Duration::from_secs(self.assignment_flush_secs)
    }

    pub fn evaluation_flush_interval(&self) -> Duration {
        Duration::from_secs(self.evaluation_flush_secs)
    }
}

impl RetryConfig {
    pub fn stream_initial_delay(&self) -> Duration {
        Duration::from_secs(self.stream_initial_delay_secs)
    }

    pub fn stream_max_delay(&self) -> Duration {
        Duration::from_secs(self.stream_max_delay_secs)
    }
}

/// Load configuration from file and environment variables
/// Environment variables override file settings with EDGE_ prefix
pub fn load_config() -> Result<EdgeConfig, config::ConfigError> {
    let config_file =
        std::env::var("EDGE_CONFIG_FILE").unwrap_or_else(|_| "config.toml".to_string());

    let settings = config::Config::builder()
        // Start with default config file
        .add_source(config::File::with_name(&config_file).required(false))
        // Override with environment variables (EDGE_BACKEND_GRPC, etc.)
        .add_source(
            config::Environment::with_prefix("EDGE")
                .separator("_")
                .try_parsing(true),
        )
        .build()?;

    settings.try_deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_config_default_values() {
        let config = CacheConfig::default();
        assert_eq!(config.max_capacity, 10000);
    }

    #[test]
    fn test_cache_config_custom_values() {
        let config = CacheConfig {
            max_capacity: 5000,
        };
        assert_eq!(config.max_capacity, 5000);
    }

    #[test]
    fn test_grpc_config_defaults() {
        let config = GrpcConfig::default();
        assert_eq!(config.connect_timeout_secs, 5);
        assert_eq!(config.timeout_secs, 10);
        assert_eq!(config.tcp_keepalive_secs, 30);
        assert_eq!(config.http2_keepalive_secs, 20);
        assert!(config.keep_alive_while_idle);
        assert_eq!(config.concurrency_limit, 256);
        assert!(config.tcp_nodelay);
    }

    #[test]
    fn test_flush_config_defaults() {
        let config = FlushConfig::default();
        assert_eq!(config.assignment_flush_secs, 10);
        assert_eq!(config.evaluation_flush_secs, 30);
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.base_delay_ms, 500);
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.stream_initial_delay_secs, 1);
        assert_eq!(config.stream_max_delay_secs, 30);
    }

    #[test]
    fn test_retry_config_duration_conversions() {
        let config = RetryConfig::default();
        assert_eq!(config.stream_initial_delay(), Duration::from_secs(1));
        assert_eq!(config.stream_max_delay(), Duration::from_secs(30));
    }

    #[test]
    fn test_grpc_config_duration_conversions() {
        let config = GrpcConfig::default();
        assert_eq!(config.connect_timeout(), Duration::from_secs(5));
        assert_eq!(config.timeout(), Duration::from_secs(10));
        assert_eq!(config.tcp_keepalive(), Some(Duration::from_secs(30)));
        assert_eq!(config.http2_keepalive(), Duration::from_secs(20));
    }

    #[test]
    fn test_flush_config_duration_conversions() {
        let config = FlushConfig::default();
        assert_eq!(config.assignment_flush_interval(), Duration::from_secs(10));
        assert_eq!(config.evaluation_flush_interval(), Duration::from_secs(30));
    }
}
