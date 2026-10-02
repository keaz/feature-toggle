//! TypeSafe Jev judgments: typed questions about application state, answered
//! with probabilities. See `docs/ai-judgments/design.md`.

pub mod client;
pub mod types;

use std::sync::Arc;

use crate::config::TypesafeConfig;

pub use client::{HttpJudgmentClient, JudgmentClient, JudgmentError};

/// The only source of the API key. Never read it from TOML, never log it.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// Trims the key and treats an empty value as missing.
pub fn normalize_api_key(raw: Option<String>) -> Option<String> {
    raw.map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

pub fn api_key_from_env() -> Option<String> {
    normalize_api_key(std::env::var(API_KEY_ENV).ok())
}

/// `None` when no key is set: every AI path is then skipped.
pub fn build_client(config: &TypesafeConfig) -> Option<Arc<dyn JudgmentClient>> {
    let api_key = api_key_from_env()?;
    match HttpJudgmentClient::new(config, api_key) {
        Ok(client) => Some(Arc::new(client)),
        Err(error) => {
            log::error!("TypeSafe client could not be built: {}", error.log_label());
            None
        }
    }
}

/// Shared with REST handlers through `web::Data<AiRuntime>`.
#[derive(Clone)]
pub struct AiRuntime {
    pub client: Option<Arc<dyn JudgmentClient>>,
    pub model: String,
}

impl AiRuntime {
    pub fn new(client: Option<Arc<dyn JudgmentClient>>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    pub fn available(&self) -> bool {
        self.client.is_some()
    }

    /// The model id for status responses; `None` when the subsystem is off.
    pub fn status_model(&self) -> Option<String> {
        self.available().then(|| self.model.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn normalize_api_key_treats_blank_as_missing() {
        assert_eq!(normalize_api_key(None), None);
        assert_eq!(normalize_api_key(Some(String::new())), None);
        assert_eq!(normalize_api_key(Some("   ".into())), None);
        assert_eq!(normalize_api_key(Some(" k1 ".into())), Some("k1".into()));
    }

    #[test]
    #[serial]
    fn build_client_is_none_without_key() {
        let original = std::env::var(API_KEY_ENV).ok();
        // SAFETY: serialized with every other env-mutating test; restored below.
        unsafe { std::env::remove_var(API_KEY_ENV) };
        assert!(build_client(&TypesafeConfig::default()).is_none());
        unsafe { std::env::set_var(API_KEY_ENV, "  ") };
        assert!(build_client(&TypesafeConfig::default()).is_none());
        unsafe { std::env::set_var(API_KEY_ENV, "test-key") };
        assert!(build_client(&TypesafeConfig::default()).is_some());
        match original {
            Some(value) => unsafe { std::env::set_var(API_KEY_ENV, value) },
            None => unsafe { std::env::remove_var(API_KEY_ENV) },
        }
    }

    #[test]
    fn runtime_reports_model_only_when_available() {
        let off = AiRuntime::new(None, "jev-1.13.0");
        assert!(!off.available());
        assert_eq!(off.status_model(), None);
        let client: Arc<dyn JudgmentClient> = Arc::new(client::MockJudgmentClient::new());
        let on = AiRuntime::new(Some(client), "jev-1.13.0");
        assert!(on.available());
        assert_eq!(on.status_model().as_deref(), Some("jev-1.13.0"));
    }
}
