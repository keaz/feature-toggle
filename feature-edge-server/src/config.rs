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

    /// gRPC request compression (none or gzip)
    #[serde(default)]
    pub compression: GrpcCompression,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum GrpcCompression {
    #[default]
    None,
    Gzip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushConfig {
    /// User assignment flush interval in seconds
    #[serde(default = "default_assignment_flush")]
    pub assignment_flush_secs: u64,

    /// Evaluation events flush interval in seconds
    #[serde(default = "default_evaluation_flush")]
    pub evaluation_flush_secs: u64,

    /// Evaluation event queue capacity (bounded channel)
    #[serde(default = "default_evaluation_event_queue_capacity")]
    pub evaluation_event_queue_capacity: usize,

    /// Max user assignment messages per gRPC stream flush
    #[serde(default = "default_assignment_flush_batch_size")]
    pub assignment_flush_batch_size: usize,

    /// Max sticky assignments queued for the backend; new assignments are
    /// dropped and counted while the queue is full
    #[serde(default = "default_assignment_queue_capacity")]
    pub assignment_queue_capacity: usize,

    /// Max evaluation events per gRPC request
    #[serde(default = "default_evaluation_flush_batch_size")]
    pub evaluation_flush_batch_size: usize,
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

    /// Client info cache TTL in seconds
    #[serde(default = "default_client_ttl")]
    pub client_ttl_secs: u64,

    /// Maximum number of client credentials whose info is cached
    #[serde(default = "default_client_max_capacity")]
    pub client_max_capacity: u64,

    /// Maximum number of sticky assignments (user, feature, environment)
    /// cached; the least recently used one is evicted when exceeded
    #[serde(default = "default_assignment_max_capacity")]
    pub assignment_max_capacity: u64,

    /// Seconds a cached assignment survives without being read or written.
    /// 0 disables idle expiry, leaving only the capacity bound
    #[serde(default = "default_assignment_time_to_idle")]
    pub assignment_time_to_idle_secs: u64,
}

fn default_max_capacity() -> u64 {
    10000
}

fn default_client_max_capacity() -> u64 {
    1000
}

fn default_client_ttl() -> u64 {
    300 // 5 minutes
}

fn default_assignment_max_capacity() -> u64 {
    50_000
}

fn default_assignment_time_to_idle() -> u64 {
    0 // disabled: evict only at capacity
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
fn default_evaluation_event_queue_capacity() -> usize {
    10_000
}
fn default_assignment_flush_batch_size() -> usize {
    1000
}
/// Default for `[flush] assignment_queue_capacity`.
pub const DEFAULT_ASSIGNMENT_QUEUE_CAPACITY: usize = 100_000;
fn default_assignment_queue_capacity() -> usize {
    DEFAULT_ASSIGNMENT_QUEUE_CAPACITY
}
fn default_evaluation_flush_batch_size() -> usize {
    500
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
            compression: GrpcCompression::default(),
        }
    }
}

impl Default for FlushConfig {
    fn default() -> Self {
        Self {
            assignment_flush_secs: default_assignment_flush(),
            evaluation_flush_secs: default_evaluation_flush(),
            evaluation_event_queue_capacity: default_evaluation_event_queue_capacity(),
            assignment_flush_batch_size: default_assignment_flush_batch_size(),
            assignment_queue_capacity: default_assignment_queue_capacity(),
            evaluation_flush_batch_size: default_evaluation_flush_batch_size(),
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
            client_ttl_secs: default_client_ttl(),
            client_max_capacity: default_client_max_capacity(),
            assignment_max_capacity: default_assignment_max_capacity(),
            assignment_time_to_idle_secs: default_assignment_time_to_idle(),
        }
    }
}

impl CacheConfig {
    pub fn client_ttl(&self) -> Duration {
        Duration::from_secs(self.client_ttl_secs)
    }

    /// Idle expiry for cached assignments; `None` when disabled.
    pub fn assignment_time_to_idle(&self) -> Option<Duration> {
        (self.assignment_time_to_idle_secs > 0)
            .then(|| Duration::from_secs(self.assignment_time_to_idle_secs))
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

    pub fn assignment_flush_batch_size(&self) -> usize {
        self.assignment_flush_batch_size.max(1)
    }

    pub fn assignment_queue_capacity(&self) -> usize {
        self.assignment_queue_capacity.max(1)
    }

    pub fn evaluation_flush_batch_size(&self) -> usize {
        self.evaluation_flush_batch_size.max(1)
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

/// Load configuration from file and environment variables.
///
/// Environment variables with the `EDGE_` prefix override file settings. A single
/// `_` follows the prefix and `__` separates nesting levels, so `EDGE_CLIENT_SECRET`
/// sets `client_secret` and `EDGE_GRPC__TIMEOUT_SECS` sets `grpc.timeout_secs`.
///
/// The legacy `EDGE_GRPC_COMPRESSION` is still honoured (with a deprecation warning)
/// unless `EDGE_GRPC__COMPRESSION` is also set, in which case the new form wins.
pub fn load_config() -> Result<EdgeConfig, config::ConfigError> {
    let config_file =
        std::env::var("EDGE_CONFIG_FILE").unwrap_or_else(|_| "config.toml".to_string());

    let mut builder = config::Config::builder()
        // Start with default config file
        .add_source(config::File::with_name(&config_file).required(false));

    // Back-compat: before `__` became the nesting separator, `EDGE_GRPC_COMPRESSION`
    // mapped to `grpc.compression`. Apply it explicitly, but only when the new
    // `EDGE_GRPC__COMPRESSION` is absent so the new form wins when both are set.
    if let Ok(legacy) = std::env::var("EDGE_GRPC_COMPRESSION") {
        if std::env::var_os("EDGE_GRPC__COMPRESSION").is_some() {
            tracing::warn!(
                "EDGE_GRPC_COMPRESSION is deprecated and ignored because EDGE_GRPC__COMPRESSION is set"
            );
        } else {
            tracing::warn!("EDGE_GRPC_COMPRESSION is deprecated; use EDGE_GRPC__COMPRESSION");
            builder = builder.set_override("grpc.compression", legacy)?;
        }
    }

    // `try_parsing` below would turn credentials like `0123` or `1e5` into numbers,
    // so read them verbatim. Overrides take precedence over the environment source.
    for (var, key) in [
        ("EDGE_CLIENT_ID", "client_id"),
        ("EDGE_CLIENT_SECRET", "client_secret"),
    ] {
        if let Ok(value) = std::env::var(var) {
            builder = builder.set_override(key, value)?;
        }
    }

    let settings = builder
        // Override with environment variables (EDGE_BACKEND_GRPC, EDGE_GRPC__TIMEOUT_SECS, etc.)
        .add_source(
            config::Environment::with_prefix("EDGE")
                .prefix_separator("_")
                .separator("__")
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
        assert_eq!(config.client_ttl_secs, 300);
    }

    #[test]
    fn test_cache_config_custom_values() {
        let config = CacheConfig {
            max_capacity: 5000,
            client_ttl_secs: 600,
            client_max_capacity: 200,
            assignment_max_capacity: 1000,
            assignment_time_to_idle_secs: 60,
        };
        assert_eq!(config.max_capacity, 5000);
        assert_eq!(config.client_ttl_secs, 600);
    }

    #[test]
    fn test_cache_config_client_max_capacity_default_and_override() {
        assert_eq!(CacheConfig::default().client_max_capacity, 1000);
        let parsed: CacheConfig = toml::from_str("client_max_capacity = 50").unwrap();
        assert_eq!(parsed.client_max_capacity, 50);
        assert_eq!(parsed.max_capacity, 10000);
    }

    #[test]
    fn test_cache_config_assignment_bounds_default_and_override() {
        let defaults = CacheConfig::default();
        assert_eq!(defaults.assignment_max_capacity, 50_000);
        // Idle expiry is off by default.
        assert_eq!(defaults.assignment_time_to_idle(), None);

        let parsed: CacheConfig =
            toml::from_str("assignment_max_capacity = 500\nassignment_time_to_idle_secs = 900")
                .unwrap();
        assert_eq!(parsed.assignment_max_capacity, 500);
        assert_eq!(
            parsed.assignment_time_to_idle(),
            Some(Duration::from_secs(900))
        );
        assert_eq!(parsed.max_capacity, 10000);
    }

    #[test]
    fn test_cache_config_duration_conversions() {
        let config = CacheConfig::default();
        assert_eq!(config.client_ttl(), Duration::from_secs(300));
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
        assert!(matches!(config.compression, GrpcCompression::None));
    }

    #[test]
    fn test_flush_config_defaults() {
        let config = FlushConfig::default();
        assert_eq!(config.assignment_flush_secs, 10);
        assert_eq!(config.evaluation_flush_secs, 30);
        assert_eq!(config.evaluation_event_queue_capacity, 10_000);
        assert_eq!(config.assignment_flush_batch_size, 1000);
        assert_eq!(config.assignment_queue_capacity, 100_000);
        assert_eq!(config.evaluation_flush_batch_size, 500);
    }

    #[test]
    fn test_flush_config_assignment_queue_capacity_override() {
        let parsed: FlushConfig = toml::from_str("assignment_queue_capacity = 2500").unwrap();
        assert_eq!(parsed.assignment_queue_capacity(), 2500);
        assert_eq!(parsed.assignment_flush_batch_size, 1000);

        // Values below 1 are treated as 1.
        let zero: FlushConfig = toml::from_str("assignment_queue_capacity = 0").unwrap();
        assert_eq!(zero.assignment_queue_capacity(), 1);
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
        assert_eq!(config.assignment_flush_batch_size(), 1000);
        assert_eq!(config.evaluation_flush_batch_size(), 500);
    }

    mod load_config_env {
        use super::super::*;
        use std::path::PathBuf;
        use std::sync::Mutex;

        /// Env vars are process-global, so every test that touches them holds this lock.
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        const BASE_CONFIG: &str = r#"
backend_grpc = "http://from-file:50051"
http_addr = "127.0.0.1:9999"
client_id = "file-client-id"
client_secret = "file-client-secret"

[grpc]
timeout_secs = 3
"#;

        /// Clears every `EDGE_*` var, sets the requested ones, and restores the
        /// original environment on drop (also when the test panics).
        struct EnvGuard {
            saved: Vec<(String, String)>,
            config_path: Option<PathBuf>,
            _lock: std::sync::MutexGuard<'static, ()>,
        }

        impl EnvGuard {
            fn new(config_contents: Option<&str>, vars: &[(&str, &str)]) -> Self {
                let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let saved: Vec<(String, String)> = std::env::vars()
                    .filter(|(k, _)| k.to_ascii_uppercase().starts_with("EDGE_"))
                    .collect();
                for (k, _) in &saved {
                    // SAFETY: ENV_LOCK serialises env access in these tests.
                    unsafe { std::env::remove_var(k) };
                }

                let unique = format!(
                    "edge-config-test-{}-{}.toml",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                );
                let path = std::env::temp_dir().join(unique);
                let config_path = match config_contents {
                    Some(contents) => {
                        std::fs::write(&path, contents).unwrap();
                        Some(path.clone())
                    }
                    None => None,
                };
                // Always point at the temp path so a stray ./config.toml is never read.
                unsafe { std::env::set_var("EDGE_CONFIG_FILE", &path) };
                for (k, v) in vars {
                    unsafe { std::env::set_var(k, v) };
                }

                Self {
                    saved,
                    config_path,
                    _lock: lock,
                }
            }
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                let current: Vec<String> = std::env::vars()
                    .map(|(k, _)| k)
                    .filter(|k| k.to_ascii_uppercase().starts_with("EDGE_"))
                    .collect();
                for k in current {
                    unsafe { std::env::remove_var(k) };
                }
                for (k, v) in &self.saved {
                    unsafe { std::env::set_var(k, v) };
                }
                if let Some(path) = &self.config_path {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        #[test]
        fn file_values_load_without_env_overrides() {
            let _env = EnvGuard::new(Some(BASE_CONFIG), &[]);
            let cfg = load_config().expect("config should load");
            assert_eq!(cfg.backend_grpc, "http://from-file:50051");
            assert_eq!(cfg.client_secret, "file-client-secret");
            assert_eq!(cfg.grpc.timeout_secs, 3);
            assert!(matches!(cfg.grpc.compression, GrpcCompression::None));
        }

        #[test]
        fn edge_client_secret_overrides_file() {
            let _env = EnvGuard::new(Some(BASE_CONFIG), &[("EDGE_CLIENT_SECRET", "x")]);
            let cfg = load_config().expect("config should load");
            assert_eq!(cfg.client_secret, "x");
            assert_eq!(cfg.client_id, "file-client-id");
        }

        #[test]
        fn credential_env_vars_are_not_parsed_as_numbers() {
            let _env = EnvGuard::new(
                Some(BASE_CONFIG),
                &[("EDGE_CLIENT_ID", "1e5"), ("EDGE_CLIENT_SECRET", "0123")],
            );
            let cfg = load_config().expect("config should load");
            assert_eq!(cfg.client_id, "1e5");
            assert_eq!(cfg.client_secret, "0123");
        }

        #[test]
        fn top_level_env_vars_override_file() {
            let _env = EnvGuard::new(
                Some(BASE_CONFIG),
                &[
                    ("EDGE_BACKEND_GRPC", "http://env-backend:50051"),
                    ("EDGE_HTTP_ADDR", "0.0.0.0:8081"),
                    ("EDGE_CLIENT_ID", "env-client-id"),
                ],
            );
            let cfg = load_config().expect("config should load");
            assert_eq!(cfg.backend_grpc, "http://env-backend:50051");
            assert_eq!(cfg.http_addr, "0.0.0.0:8081");
            assert_eq!(cfg.client_id, "env-client-id");
        }

        #[test]
        fn double_underscore_overrides_nested_grpc_timeout() {
            let _env = EnvGuard::new(Some(BASE_CONFIG), &[("EDGE_GRPC__TIMEOUT_SECS", "7")]);
            let cfg = load_config().expect("config should load");
            assert_eq!(cfg.grpc.timeout_secs, 7);
        }

        #[test]
        fn legacy_grpc_compression_still_applies() {
            let _env = EnvGuard::new(Some(BASE_CONFIG), &[("EDGE_GRPC_COMPRESSION", "gzip")]);
            let cfg = load_config().expect("config should load");
            assert!(matches!(cfg.grpc.compression, GrpcCompression::Gzip));
        }

        #[test]
        fn new_grpc_compression_form_applies() {
            let _env = EnvGuard::new(Some(BASE_CONFIG), &[("EDGE_GRPC__COMPRESSION", "gzip")]);
            let cfg = load_config().expect("config should load");
            assert!(matches!(cfg.grpc.compression, GrpcCompression::Gzip));
        }

        #[test]
        fn new_grpc_compression_form_wins_over_legacy() {
            let _env = EnvGuard::new(
                Some("[grpc]\ncompression = \"gzip\"\n"),
                &[
                    ("EDGE_BACKEND_GRPC", "http://env-backend:50051"),
                    ("EDGE_HTTP_ADDR", "0.0.0.0:8081"),
                    ("EDGE_CLIENT_ID", "env-client-id"),
                    ("EDGE_CLIENT_SECRET", "env-secret"),
                    ("EDGE_GRPC_COMPRESSION", "gzip"),
                    ("EDGE_GRPC__COMPRESSION", "none"),
                ],
            );
            let cfg = load_config().expect("config should load");
            assert!(matches!(cfg.grpc.compression, GrpcCompression::None));
        }

        #[test]
        fn docker_env_vars_work_without_config_file() {
            // Mirrors the `docker run` example in DOCKER.md: the image ships no config.toml.
            let _env = EnvGuard::new(
                None,
                &[
                    ("EDGE_BACKEND_GRPC", "http://backend-host:50051"),
                    ("EDGE_HTTP_ADDR", "0.0.0.0:8081"),
                    ("EDGE_CLIENT_ID", "your-client-id"),
                    ("EDGE_CLIENT_SECRET", "your-client-secret"),
                ],
            );
            let cfg = load_config().expect("config should load from env only");
            assert_eq!(cfg.backend_grpc, "http://backend-host:50051");
            assert_eq!(cfg.http_addr, "0.0.0.0:8081");
            assert_eq!(cfg.client_id, "your-client-id");
            assert_eq!(cfg.client_secret, "your-client-secret");
            assert_eq!(cfg.grpc.timeout_secs, 10);
        }
    }
}
