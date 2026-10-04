//! CLI errors and the process exit codes they map to.

use serde_json::{Value, json};

pub const EXIT_OK: i32 = 0;
pub const EXIT_OTHER: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_AUTH: i32 = 3;
pub const EXIT_FORBIDDEN: i32 = 4;
pub const EXIT_NOT_FOUND: i32 = 5;
pub const EXIT_CONFLICT: i32 = 6;
pub const EXIT_SERVER: i32 = 7;
/// `evaluate --exit-code` when the flag evaluated to `false`.
pub const EXIT_FLAG_OFF: i32 = 10;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Bad arguments or configuration.
    #[error("{0}")]
    Usage(String),
    /// No credentials, or the session cannot be used.
    #[error("{0}")]
    Auth(String),
    /// The backend answered with a non-success status.
    #[error("{message} (code {code}, HTTP {status})")]
    Api {
        status: u16,
        code: String,
        message: String,
        body: Value,
    },
    /// Timeout, connection or transport failure.
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Other(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => EXIT_USAGE,
            CliError::Auth(_) => EXIT_AUTH,
            CliError::Network(_) => EXIT_SERVER,
            CliError::Other(_) => EXIT_OTHER,
            CliError::Api { status, .. } => match *status {
                401 => EXIT_AUTH,
                403 => EXIT_FORBIDDEN,
                404 => EXIT_NOT_FOUND,
                409 => EXIT_CONFLICT,
                500..=599 => EXIT_SERVER,
                _ => EXIT_OTHER,
            },
        }
    }

    /// Form printed on stderr when the output format is json: the server's
    /// error body for API errors.
    pub fn to_json(&self) -> Value {
        match self {
            CliError::Api { body, .. } => body.clone(),
            other => json!({ "error": other.kind(), "message": other.to_string() }),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            CliError::Usage(_) => "usage",
            CliError::Auth(_) => "auth",
            CliError::Api { .. } => "api",
            CliError::Network(_) => "network",
            CliError::Other(_) => "error",
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(err: std::io::Error) -> Self {
        CliError::Other(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn api(status: u16) -> CliError {
        CliError::Api {
            status,
            code: "c".into(),
            message: "m".into(),
            body: json!({ "error": "e", "message": "m", "code": "c" }),
        }
    }

    #[test]
    fn maps_errors_to_exit_codes() {
        assert_eq!(CliError::Usage("x".into()).exit_code(), EXIT_USAGE);
        assert_eq!(CliError::Auth("x".into()).exit_code(), EXIT_AUTH);
        assert_eq!(CliError::Network("x".into()).exit_code(), EXIT_SERVER);
        assert_eq!(CliError::Other("x".into()).exit_code(), EXIT_OTHER);
        assert_eq!(api(401).exit_code(), EXIT_AUTH);
        assert_eq!(api(403).exit_code(), EXIT_FORBIDDEN);
        assert_eq!(api(404).exit_code(), EXIT_NOT_FOUND);
        assert_eq!(api(409).exit_code(), EXIT_CONFLICT);
        assert_eq!(api(503).exit_code(), EXIT_SERVER);
        assert_eq!(api(400).exit_code(), EXIT_OTHER);
        assert_eq!(api(429).exit_code(), EXIT_OTHER);
    }

    #[test]
    fn api_error_display_names_code_and_status() {
        assert_eq!(api(404).to_string(), "m (code c, HTTP 404)");
    }

    #[test]
    fn json_form_is_the_server_body_for_api_errors() {
        assert_eq!(api(404).to_json()["code"], "c");
        let usage = CliError::Usage("bad flag".into()).to_json();
        assert_eq!(usage, json!({ "error": "usage", "message": "bad flag" }));
    }
}
