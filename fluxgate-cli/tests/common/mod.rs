#![allow(dead_code)]

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fluxgate_cli::Io;
use fluxgate_cli::config::Env;
use fluxgate_cli::prompt::ScriptedPrompter;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::MockServer;

pub const TEAM_A: &str = "11111111-1111-1111-1111-111111111111";
pub const TEAM_B: &str = "22222222-2222-2222-2222-222222222222";
pub const ENV_STAGING: &str = "33333333-3333-3333-3333-333333333333";
pub const FEATURE_ID: &str = "44444444-4444-4444-4444-444444444444";

pub struct Harness {
    pub dir: TempDir,
    pub server: MockServer,
}

pub struct RunResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Harness {
    pub async fn new() -> Self {
        Self { dir: tempfile::tempdir().unwrap(), server: MockServer::start().await }
    }

    pub fn url(&self) -> String {
        format!("{}/api/v1", self.server.uri())
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn write(&self, name: &str, contents: &str) {
        let path = self.path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    pub fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    pub fn write_session(&self, name: &str, access: &str, refresh: &str, expires_in_secs: i64) {
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in_secs);
        self.write(
            &format!("sessions/{name}.json"),
            &json!({ "accessToken": access, "refreshToken": refresh, "expiresAt": expires_at.to_rfc3339(),
                     "user": { "id": "u1", "username": "alice" } })
            .to_string(),
        );
    }

    fn env(&self, extra: &[(&str, &str)]) -> Env {
        let mut pairs = vec![
            ("FLUXGATE_CONFIG_FILE".to_string(), self.path("config").display().to_string()),
            ("FLUXGATE_SHARED_CREDENTIALS_FILE".to_string(), self.path("credentials").display().to_string()),
        ];
        pairs.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Env::from_pairs(pairs)
    }

    pub async fn run(&self, args: &[&str], env: &[(&str, &str)]) -> RunResult {
        self.run_with(args, env, &[]).await
    }

    pub async fn run_with(&self, args: &[&str], env: &[(&str, &str)], answers: &[&str]) -> RunResult {
        let mut prompter = ScriptedPrompter::new(answers);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut argv = vec!["fluxgate"];
        argv.extend_from_slice(args);
        let code = fluxgate_cli::run(
            argv,
            Io { env: self.env(env), is_tty: false, prompter: &mut prompter, out: &mut out, err: &mut err },
        )
        .await;
        RunResult { code, stdout: String::from_utf8(out).unwrap(), stderr: String::from_utf8(err).unwrap() }
    }
}

pub fn json_out(result: &RunResult) -> Value {
    serde_json::from_str(&result.stdout).unwrap_or_else(|err| panic!("{err}: {}", result.stdout))
}

pub fn fake_jwt(claims: Value) -> String {
    format!("{}.{}.sig", URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#), URL_SAFE_NO_PAD.encode(claims.to_string()))
}

pub fn user_token(username: &str) -> String {
    fake_jwt(json!({ "sub": "u1", "username": username, "is_admin": false, "exp": 4102444800i64, "token_type": "user" }))
}

pub fn system_token(team_id: &str) -> String {
    fake_jwt(json!({ "sub": "sc1", "username": "ci-bot", "is_admin": false, "exp": 4102444800i64,
                     "token_type": "system_client", "team_id": team_id }))
}

pub fn login_body(access: &str, refresh: &str) -> Value {
    json!({ "token": access, "refreshToken": refresh, "expiresIn": 1800, "isTemporary": false,
            "user": { "id": "u1", "username": "alice", "isAdmin": false } })
}

pub fn features(range: std::ops::Range<usize>) -> Vec<Value> {
    range
        .map(|i| json!({ "id": format!("id-{i}"), "key": format!("flag-{i}"), "featureType": "SIMPLE",
                         "enabled": true, "lifecycleStage": "ACTIVE" }))
        .collect()
}
