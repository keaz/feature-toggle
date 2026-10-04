//! Cached login sessions under the sessions directory.

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::ApiClient;
use crate::config::files::write_private;
use crate::error::CliError;

/// Refresh when the access token expires within this many seconds.
pub const REFRESH_MARGIN_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUser {
    pub id: String,
    pub username: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCache {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub user: SessionUser,
}

impl std::fmt::Debug for SessionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCache")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("user", &self.user)
            .finish()
    }
}

/// Body of `POST /auth/login` and `POST /auth/refresh`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: SessionUser,
    #[serde(default)]
    pub is_temporary: bool,
}

impl SessionCache {
    pub fn from_login(response: &LoginResponse, now: DateTime<Utc>) -> Self {
        Self {
            access_token: response.token.clone(),
            refresh_token: response.refresh_token.clone(),
            expires_at: now + chrono::Duration::seconds(response.expires_in),
            user: response.user.clone(),
        }
    }

    pub fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        self.expires_at - now < chrono::Duration::seconds(REFRESH_MARGIN_SECS)
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    pub fn load(&self, name: &str) -> Result<Option<SessionCache>, CliError> {
        let path = self.file(name);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|err| {
                CliError::Auth(format!(
                    "session cache {} is damaged ({err}): run fluxgate login",
                    path.display()
                ))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub fn save(&self, name: &str, cache: &SessionCache) -> Result<(), CliError> {
        let bytes = serde_json::to_vec_pretty(cache).map_err(|err| CliError::Other(err.to_string()))?;
        write_private(&self.file(name), &bytes)
    }

    pub fn delete(&self, name: &str) -> Result<bool, CliError> {
        match std::fs::remove_file(self.file(name)) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Names of all cached sessions, sorted.
    pub fn names(&self) -> Result<Vec<String>, CliError> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// A usable access token for session `name`, refreshed when it expires
    /// within [`REFRESH_MARGIN_SECS`].
    pub async fn access_token(
        &self,
        name: &str,
        profile: &str,
        base_url: &str,
        timeout: Duration,
    ) -> Result<String, CliError> {
        let expired = || CliError::Auth(format!("session expired: run fluxgate login --profile {profile}"));
        let cache = self.load(name)?.ok_or_else(|| {
            CliError::Auth(format!("not logged in: run fluxgate login --profile {profile}"))
        })?;
        if !cache.needs_refresh(Utc::now()) {
            return Ok(cache.access_token);
        }

        // Refresh tokens rotate and a reused one revokes the whole family, so
        // only one process may refresh. The lock is released when `_lock` drops.
        let _lock = self.lock(name).await?;
        // Another process may have refreshed while this one waited.
        let cache = self.load(name)?.ok_or_else(expired)?;
        if !cache.needs_refresh(Utc::now()) {
            return Ok(cache.access_token);
        }
        let api = ApiClient::new(base_url, None, timeout)?;
        let response = match api
            .post(&["auth", "refresh"], &json!({ "refreshToken": cache.refresh_token }))
            .await
        {
            Ok(value) => value,
            Err(CliError::Api { status: 400 | 401, .. }) => return Err(expired()),
            Err(err) => return Err(err),
        };
        let response: LoginResponse = serde_json::from_value(response)
            .map_err(|err| CliError::Other(format!("unexpected refresh response: {err}")))?;
        let refreshed = SessionCache::from_login(&response, Utc::now());
        self.save(name, &refreshed)?;
        Ok(refreshed.access_token)
    }

    async fn lock(&self, name: &str) -> Result<File, CliError> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{name}.lock"));
        tokio::task::spawn_blocking(move || -> std::io::Result<File> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)?;
            file.lock()?;
            Ok(file)
        })
        .await
        .map_err(|err| CliError::Other(err.to_string()))?
        .map_err(CliError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_AUTH;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cache(access: &str, refresh: &str, expires_in_secs: i64) -> SessionCache {
        SessionCache {
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at: Utc::now() + chrono::Duration::seconds(expires_in_secs),
            user: SessionUser { id: "u1".into(), username: "alice".into() },
        }
    }

    fn login_body(access: &str, refresh: &str) -> serde_json::Value {
        json!({ "token": access, "refreshToken": refresh, "expiresIn": 1800,
                "user": { "id": "u1", "username": "alice", "isAdmin": false } })
    }

    #[test]
    fn save_load_names_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("sessions"));
        assert_eq!(store.load("corp").unwrap(), None);
        store.save("corp", &cache("a1", "r1", 600)).unwrap();
        store.save("other", &cache("a2", "r2", 600)).unwrap();
        assert_eq!(store.load("corp").unwrap().unwrap().access_token, "a1");
        assert_eq!(store.names().unwrap(), vec!["corp", "other"]);
        assert!(store.delete("corp").unwrap());
        assert!(!store.delete("corp").unwrap());
    }

    #[test]
    fn needs_refresh_inside_the_margin() {
        assert!(cache("a", "r", 30).needs_refresh(Utc::now()));
        assert!(!cache("a", "r", 600).needs_refresh(Utc::now()));
    }

    #[test]
    fn debug_hides_tokens() {
        let shown = format!("{:?}", cache("access-secret", "refresh-secret", 600));
        assert!(!shown.contains("access-secret") && !shown.contains("refresh-secret"));
    }

    #[tokio::test]
    async fn fresh_token_is_returned_without_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        store.save("corp", &cache("a1", "r1", 600)).unwrap();
        // No server: any request would fail to connect.
        let token = store.access_token("corp", "default", "http://127.0.0.1:9/api/v1", Duration::from_secs(1)).await.unwrap();
        assert_eq!(token, "a1");
    }

    #[tokio::test]
    async fn expiring_token_is_refreshed_and_saved() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .and(body_json(json!({ "refreshToken": "r1" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a2", "r2")))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        store.save("corp", &cache("a1", "r1", 10)).unwrap();
        let url = format!("{}/api/v1", server.uri());
        assert_eq!(store.access_token("corp", "default", &url, Duration::from_secs(5)).await.unwrap(), "a2");
        let saved = store.load("corp").unwrap().unwrap();
        assert_eq!(saved.refresh_token, "r2");
        assert!(!saved.needs_refresh(Utc::now()));
    }

    #[tokio::test]
    async fn rejected_refresh_says_session_expired() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "unauthorized", "message": "invalid refresh token" })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        store.save("corp", &cache("a1", "r1", -5)).unwrap();
        let url = format!("{}/api/v1", server.uri());
        let err = store.access_token("corp", "prod", &url, Duration::from_secs(5)).await.unwrap_err();
        assert_eq!(err.to_string(), "session expired: run fluxgate login --profile prod");
        assert_eq!(err.exit_code(), EXIT_AUTH);
    }

    #[tokio::test]
    async fn missing_session_says_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let err = store.access_token("corp", "prod", "http://h/api/v1", Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(err.to_string(), "not logged in: run fluxgate login --profile prod");
    }

    #[tokio::test]
    async fn damaged_cache_asks_for_login() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("corp.json"), "{\"accessToken\": \"a").unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let err = store.access_token("corp", "prod", "http://h/api/v1", Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_AUTH);
        assert!(err.to_string().contains("damaged"));
        assert!(err.to_string().contains("fluxgate login"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_refreshes_send_one_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .and(body_json(json!({ "refreshToken": "r1" })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(login_body("a2", "r2"))
                    .set_delay(Duration::from_millis(300)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        store.save("corp", &cache("a1", "r1", -5)).unwrap();
        let url = format!("{}/api/v1", server.uri());
        let (first, second) = tokio::join!(
            store.access_token("corp", "default", &url, Duration::from_secs(5)),
            store.access_token("corp", "default", &url, Duration::from_secs(5)),
        );
        assert_eq!(first.unwrap(), "a2");
        assert_eq!(second.unwrap(), "a2");
    }
}
