# FluxGate CLI Plan 1: crate, fixes, profiles and password login

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the `fluxgate` CLI into its own workspace crate, fix the defects of the existing commands, and add AWS-style profiles, a credentials file, password login sessions, `configure`, `teams` and `whoami`.

**Architecture:** New crate `fluxgate-cli` (library `fluxgate_cli` plus binary `fluxgate`). `lib::run` parses arguments, resolves profile, settings and credentials from an environment snapshot and the INI files, calls one command module, and renders its `Outcome` as json, table or text. All outside effects (environment, prompts, stdout, stderr, files under a configurable directory, HTTP base URL) are injectable, so integration tests drive `run` against `wiremock` and a temp directory.

**Tech Stack:** Rust 2024, clap 4 (derive), reqwest 0.12 (rustls), tokio, serde/serde_json, rust-ini 0.21, comfy-table 7, rpassword 7, dirs 6, chrono, base64, uuid, thiserror 2; tests: wiremock 0.6, tempfile 3.

**Spec:** `docs/superpowers/specs/2026-10-04-fluxgate-cli-auth-profiles-design.md` (this plan covers delivery steps 1 and 2; SSO loopback is Plan 2, device code is Plan 3).

## Global Constraints

- Crate `fluxgate-cli`, edition 2024, binary name `fluxgate`, library name `fluxgate_cli`. Workspace member in the root `Cargo.toml`.
- Rust 1.89 or newer (uses `std::fs::File::lock`; local toolchain is 1.98).
- No backend code changes in this plan except deleting `feature-toggle-backend/src/bin/fluxgate.rs` and the backend `clap` dependency.
- Files: `~/.fluxgate/config`, `~/.fluxgate/credentials`, `<dir of config file>/sessions/<session>.json` and `<session>.lock`. Overrides: `FLUXGATE_CONFIG_FILE`, `FLUXGATE_SHARED_CREDENTIALS_FILE`. Files the CLI writes are mode 0600 on unix.
- Config sections: `[default]`, `[profile <name>]`, `[session <name>]`. Credentials sections: `[<profile>]` with key `token`.
- Env vars: `FLUXGATE_PROFILE`, `FLUXGATE_URL`, `FLUXGATE_TEAM` (alias `FLUXGATE_TEAM_ID`), `FLUXGATE_ENVIRONMENT` (alias `FLUXGATE_ENVIRONMENT_ID`), `FLUXGATE_OUTPUT`, `FLUXGATE_TOKEN`, `FLUXGATE_TIMEOUT`.
- Flags (global): `--profile`, `--url` (alias `--base-url`), `--team` (alias `--team-id`), `--env` (alias `--environment-id`), `--output json|table|text`, `--json`, `--token`, `--timeout`.
- Defaults: url `http://localhost:8080/api/v1`, timeout 30 s, connect timeout 10 s, refresh when less than 60 s remain, output `table` on a TTY else `json`.
- Exit codes: 0 ok, 1 other, 2 usage/config, 3 auth (HTTP 401, no credentials, session expired), 4 HTTP 403, 5 HTTP 404, 6 HTTP 409, 7 HTTP 5xx/timeout/connect, 10 `evaluate --exit-code` with flag off.
- Exact messages: `session expired: run fluxgate login --profile <p>`, `no credentials for profile <p>: run fluxgate configure or fluxgate login`, `not logged in: run fluxgate login --profile <p>`.
- Never print or `Debug`-format tokens, refresh tokens or passwords.
- Comments and docs in plain English; match the backend's comment density (short doc comments on public items, comments only where the reason is not obvious).

### Spec deviations (decided while planning, from the code)

- `fd-lock` and `is-terminal` are not used: std `File::lock` and `std::io::IsTerminal` do the same.
- `whoami` reads the token claims and `GET /teams` instead of `GET /users/{id}` (that route may need admin rights).
- `POST /auth/reset-password` answers 204 without a session, so after a temporary-password change the CLI logs in again with the new password and logs the temporary session out.
- `rust-ini` does not keep comments when it rewrites a file, and does not support comments after a value on the same line. README says so.
- Errors print as JSON on stderr whenever the resolved output format is `json` (also the non-TTY default), not only with an explicit `--output json`.
- Deps `open`, `sha2`, `rand`, `url` move to Plan 2 (SSO).

## Review Focus

1. A proxy in front of the backend answers with an HTML error page (502/504): expect `error: <text> (code unknown, HTTP 502)` and exit 7, not a JSON parse error. Test in Task 5.
2. A session cache file that is empty or truncated (crash during write): expect exit 3 with a message that says to run `fluxgate login`, not a panic or exit 1. Test in Task 6.
3. `FLUXGATE_PROFILE` or `--profile` naming a profile that does not exist: expect exit 2 naming the profile, never a silent fall back to `default` (which could hit production). Test in Task 4.
4. A list longer than one page in table output: expect a footer `N of M shown; use --all or --offset for more`, so a truncated table is visible. Test in Task 2.
5. `rollout promote --flag <key> --env <uuid>`: the by-key route needs an environment name, so a UUID must be mapped to its name. Test in Task 9.

---

## File structure

```
fluxgate-cli/
  Cargo.toml
  README.md
  src/lib.rs              run(), Io
  src/main.rs             process entry point
  src/error.rs            CliError, exit codes
  src/output.rs           OutputFormat, Kind, Column, Outcome, render()
  src/prompt.rs           Prompter trait, TerminalPrompter, ScriptedPrompter
  src/api.rs              ApiClient
  src/context.rs          Context: authenticated client, team and environment resolution
  src/cli.rs              clap definitions
  src/config/mod.rs       re-exports, mask_token()
  src/config/env.rs       Env snapshot
  src/config/paths.rs     Paths
  src/config/files.rs     ConfigFiles (INI), write_private(), permission_warning()
  src/config/resolve.rs   Settings, Overrides, Credential, resolve(), selected_profile()
  src/auth/mod.rs
  src/auth/claims.rs      TokenClaims, decode_claims()
  src/auth/session.rs     SessionCache, SessionStore (locked refresh), LoginResponse
  src/auth/password.rs    password_login()
  src/commands/mod.rs     App, dispatch(), list_paged()
  src/commands/{health,flags,approvals,evaluate,config_export,rollout,login,logout,configure,teams,whoami}.rs
  tests/common/mod.rs     Harness (temp dir + MockServer), token helpers
  tests/read_commands.rs  tests/write_commands.rs  tests/login.rs  tests/configure.rs  tests/teams_whoami.rs
```

---

### Task 1: Crate skeleton, errors and environment snapshot

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `fluxgate-cli/Cargo.toml`, `fluxgate-cli/src/lib.rs`, `fluxgate-cli/src/error.rs`, `fluxgate-cli/src/config/mod.rs`, `fluxgate-cli/src/config/env.rs`

**Interfaces:**
- Produces: `fluxgate_cli::error::{CliError, EXIT_OK, EXIT_OTHER, EXIT_USAGE, EXIT_AUTH, EXIT_FORBIDDEN, EXIT_NOT_FOUND, EXIT_CONFLICT, EXIT_SERVER, EXIT_FLAG_OFF}`; `CliError::{Usage(String), Auth(String), Api { status: u16, code: String, message: String, body: Value }, Network(String), Other(String)}`; `CliError::exit_code(&self) -> i32`; `CliError::to_json(&self) -> Value`; `impl From<std::io::Error> for CliError`.
- Produces: `fluxgate_cli::config::Env` with `from_process() -> Env`, `from_pairs<K: Into<String>, V: Into<String>>(impl IntoIterator<Item = (K, V)>) -> Env`, `get(&self, &str) -> Option<&str>`, `first(&self, &[&str]) -> Option<&str>` (trimmed, blank counts as unset).

- [ ] **Step 1: Add the workspace member**

In the root `Cargo.toml`, change `members` to:

```toml
members = [
    "evaluation-engine",
    "feature-edge-server",
    "feature-toggle-backend",
    "feature-toggle-shared",
    "fluxgate-cli",
]
```

- [ ] **Step 2: Create `fluxgate-cli/Cargo.toml`**

```toml
[package]
name = "fluxgate-cli"
version = "0.1.0"
edition = "2024"
description = "FluxGate command line interface"

[lib]
name = "fluxgate_cli"
path = "src/lib.rs"

[dependencies]
base64 = "0.22.1"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4.5.40", features = ["derive"] }
comfy-table = "7"
dirs = "6"
reqwest = { version = "0.12.20", default-features = false, features = ["json", "rustls-tls"] }
rpassword = "7"
rust-ini = "0.21"
serde = { version = "1.0.219", features = ["derive"] }
serde_json = "1.0"
thiserror = "2"
tokio = { version = "1.45.1", features = ["macros", "rt-multi-thread"] }
uuid = "1.17.0"

[dev-dependencies]
tempfile = "3"
wiremock = "0.6.5"
```

- [ ] **Step 3: Write the failing tests**

`fluxgate-cli/src/lib.rs`:

```rust
//! FluxGate command line interface: profiles, login sessions and API commands.

pub mod config;
pub mod error;
```

`fluxgate-cli/src/config/mod.rs`:

```rust
//! Configuration: environment snapshot, config and credentials files, resolution.

pub mod env;

pub use env::Env;
```

`fluxgate-cli/src/error.rs` (only the tests for now; Step 5 adds the code above them):

```rust
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
```

`fluxgate-cli/src/config/env.rs` (only the tests for now):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_skips_unset_and_blank_values() {
        let env = Env::from_pairs([("A", "  "), ("B", " value ")]);
        assert_eq!(env.get("A"), None);
        assert_eq!(env.get("B"), Some("value"));
        assert_eq!(env.first(&["MISSING", "A", "B"]), Some("value"));
        assert_eq!(env.first(&["MISSING"]), None);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli`
Expected: FAIL to compile (`cannot find type CliError`, `cannot find type Env`).

- [ ] **Step 5: Implement**

Prepend to `fluxgate-cli/src/error.rs` (above the tests module):

```rust
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
```

Prepend to `fluxgate-cli/src/config/env.rs`:

```rust
//! Snapshot of the process environment, so resolution is testable.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Env(HashMap<String, String>);

impl Env {
    pub fn from_process() -> Self {
        Self(std::env::vars().collect())
    }

    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }

    /// Trimmed value of `key`; blank values count as unset.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.first(&[key])
    }

    /// Value of the first key in `keys` that is set and not blank.
    pub fn first(&self, keys: &[&str]) -> Option<&str> {
        keys.iter()
            .filter_map(|key| self.0.get(*key))
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli`
Expected: PASS (4 tests).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock fluxgate-cli
git commit -m "feat(cli): add fluxgate-cli crate with errors and exit codes"
```

---

### Task 2: Output rendering

**Files:**
- Create: `fluxgate-cli/src/output.rs`
- Modify: `fluxgate-cli/src/lib.rs` (add `pub mod output;`)

**Interfaces:**
- Produces: `OutputFormat::{Json, Table, Text}` (derives `clap::ValueEnum`, `FromStr<Err = String>`, `as_str()`); `Column { header: &'static str, pointer: &'static str }`; `Kind::{Object, Document, Message, List(&'static [Column])}`; column sets `FEATURE_COLUMNS`, `APPROVAL_COLUMNS`, `TEAM_COLUMNS`, `PROFILE_COLUMNS`, `CONFIG_COLUMNS`; `Outcome { value: Value, kind: Kind, exit_code: i32, warnings: Vec<String> }` with `Outcome::new(Value, Kind)` and `Outcome::message(impl Into<String>)`; `render(&Value, Kind, OutputFormat) -> String`; `cell(Option<&Value>) -> String`.

- [ ] **Step 1: Write the failing tests**

Create `fluxgate-cli/src/output.rs` with:

```rust
//! Rendering of command results as json, table or text.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one_feature(total: i64) -> Value {
        json!({
            "items": [{
                "key": "checkout", "featureType": "SIMPLE", "enabled": true,
                "lifecycleStage": "ACTIVE", "owner": null, "id": "f1"
            }],
            "meta": { "offset": 0, "limit": 50, "total": total }
        })
    }

    #[test]
    fn json_is_pretty_printed() {
        assert_eq!(
            render(&json!({ "a": 1 }), Kind::Object, OutputFormat::Json),
            "{\n  \"a\": 1\n}"
        );
    }

    #[test]
    fn list_table_shows_headers_cells_and_dash_for_null() {
        let out = render(&one_feature(1), Kind::List(FEATURE_COLUMNS), OutputFormat::Table);
        assert!(out.contains("KEY"));
        assert!(out.contains("LIFECYCLE"));
        assert!(out.contains("checkout"));
        assert!(!out.contains("null"));
        assert!(!out.contains("shown"));
    }

    #[test]
    fn list_table_says_when_more_items_exist() {
        let out = render(&one_feature(120), Kind::List(FEATURE_COLUMNS), OutputFormat::Table);
        assert!(out.ends_with("1 of 120 shown; use --all or --offset for more"));
    }

    #[test]
    fn list_text_is_tab_separated_without_header() {
        let out = render(&one_feature(1), Kind::List(FEATURE_COLUMNS), OutputFormat::Text);
        assert_eq!(out, "checkout\tSIMPLE\ttrue\tACTIVE\t-\tf1");
    }

    #[test]
    fn object_text_lists_sorted_key_value_lines() {
        let value = json!({ "value": true, "flagKey": "checkout", "tags": ["a", "b"] });
        assert_eq!(
            render(&value, Kind::Object, OutputFormat::Text),
            "flagKey: checkout\ntags: a, b\nvalue: true"
        );
    }

    #[test]
    fn object_table_has_key_and_value_columns() {
        let out = render(&json!({ "status": "ok" }), Kind::Object, OutputFormat::Table);
        assert!(out.contains("KEY") && out.contains("VALUE") && out.contains("status") && out.contains("ok"));
    }

    #[test]
    fn message_is_plain_text_outside_json() {
        let outcome = Outcome::message("Logged in");
        assert_eq!(render(&outcome.value, outcome.kind, OutputFormat::Table), "Logged in");
        assert_eq!(render(&outcome.value, outcome.kind, OutputFormat::Text), "Logged in");
        assert!(render(&outcome.value, outcome.kind, OutputFormat::Json).contains("\"message\""));
    }

    #[test]
    fn document_is_json_in_every_format() {
        let value = json!({ "team": { "id": "t" } });
        assert_eq!(
            render(&value, Kind::Document, OutputFormat::Table),
            render(&value, Kind::Document, OutputFormat::Json)
        );
    }

    #[test]
    fn output_format_parses_ignoring_case() {
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!(" table ".parse::<OutputFormat>(), Ok(OutputFormat::Table));
        assert!("yaml".parse::<OutputFormat>().unwrap_err().contains("expected json, table or text"));
    }
}
```

Add `pub mod output;` to `src/lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli output`
Expected: FAIL to compile (`cannot find function render`).

- [ ] **Step 3: Implement**

Insert above the tests module in `src/output.rs`:

```rust
use comfy_table::Table;
use comfy_table::presets::UTF8_HORIZONTAL_ONLY;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Json,
    Table,
    Text,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Json => "json",
            OutputFormat::Table => "table",
            OutputFormat::Text => "text",
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "table" => Ok(OutputFormat::Table),
            "text" => Ok(OutputFormat::Text),
            other => Err(format!(
                "invalid output '{other}': expected json, table or text"
            )),
        }
    }
}

/// One table column: header and JSON pointer into each item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub header: &'static str,
    pub pointer: &'static str,
}

const fn col(header: &'static str, pointer: &'static str) -> Column {
    Column { header, pointer }
}

pub const FEATURE_COLUMNS: &[Column] = &[
    col("KEY", "/key"),
    col("TYPE", "/featureType"),
    col("ENABLED", "/enabled"),
    col("LIFECYCLE", "/lifecycleStage"),
    col("OWNER", "/owner"),
    col("ID", "/id"),
];
pub const APPROVAL_COLUMNS: &[Column] = &[
    col("ID", "/id"),
    col("FEATURE", "/featureId"),
    col("CHANGE", "/changeType"),
    col("STATUS", "/status"),
    col("REQUESTED BY", "/requestedBy"),
    col("CREATED", "/createdAt"),
];
pub const TEAM_COLUMNS: &[Column] = &[col("ACTIVE", "/active"), col("NAME", "/name"), col("ID", "/id")];
pub const PROFILE_COLUMNS: &[Column] = &[
    col("ACTIVE", "/active"),
    col("NAME", "/name"),
    col("SESSION", "/session"),
    col("TEAM", "/team"),
    col("URL", "/url"),
];
pub const CONFIG_COLUMNS: &[Column] = &[col("NAME", "/name"), col("VALUE", "/value"), col("SOURCE", "/source")];

/// How a result is shown in table and text formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Key/value rows of a JSON object.
    Object,
    /// Always pretty JSON (exports).
    Document,
    /// `{"message": "..."}` shown as plain text.
    Message,
    /// `{"items": [...], "meta": {...}}` shown with these columns.
    List(&'static [Column]),
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub value: Value,
    pub kind: Kind,
    pub exit_code: i32,
    /// Printed on stderr after the command.
    pub warnings: Vec<String>,
}

impl Outcome {
    pub fn new(value: Value, kind: Kind) -> Self {
        Self { value, kind, exit_code: 0, warnings: Vec::new() }
    }

    pub fn message(text: impl Into<String>) -> Self {
        Self::new(json!({ "message": text.into() }), Kind::Message)
    }
}

pub fn render(value: &Value, kind: Kind, format: OutputFormat) -> String {
    match (kind, format) {
        (_, OutputFormat::Json) | (Kind::Document, _) => pretty(value),
        (Kind::Message, _) => value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        (Kind::List(columns), OutputFormat::Table) => list_table(value, columns),
        (Kind::List(columns), OutputFormat::Text) => list_text(value, columns),
        (Kind::Object, OutputFormat::Table) => object_table(value),
        (Kind::Object, OutputFormat::Text) => object_text(value),
    }
}

/// Display form of one value: `-` for missing or null, arrays joined.
pub fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| cell(Some(item)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(other) => other.to_string(),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn items(value: &Value) -> &[Value] {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn list_table(value: &Value, columns: &[Column]) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_HORIZONTAL_ONLY);
    table.set_header(columns.iter().map(|column| column.header));
    for item in items(value) {
        table.add_row(columns.iter().map(|column| cell(item.pointer(column.pointer))));
    }
    let mut out = table.to_string();
    let shown = items(value).len() as i64;
    if let Some(total) = value.pointer("/meta/total").and_then(Value::as_i64)
        && shown < total
    {
        out.push_str(&format!(
            "\n{shown} of {total} shown; use --all or --offset for more"
        ));
    }
    out
}

fn list_text(value: &Value, columns: &[Column]) -> String {
    items(value)
        .iter()
        .map(|item| {
            columns
                .iter()
                .map(|column| cell(item.pointer(column.pointer)))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn object_rows(value: &Value) -> Vec<(String, String)> {
    match value.as_object() {
        Some(map) => map
            .iter()
            .map(|(key, item)| {
                let shown = match item {
                    Value::Object(_) => item.to_string(),
                    other => cell(Some(other)),
                };
                (key.clone(), shown)
            })
            .collect(),
        None => vec![("value".to_string(), cell(Some(value)))],
    }
}

fn object_table(value: &Value) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_HORIZONTAL_ONLY);
    table.set_header(["KEY", "VALUE"]);
    for (key, shown) in object_rows(value) {
        table.add_row([key, shown]);
    }
    table.to_string()
}

fn object_text(value: &Value) -> String {
    object_rows(value)
        .into_iter()
        .map(|(key, shown)| format!("{key}: {shown}"))
        .collect::<Vec<_>>()
        .join("\n")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli output`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): render results as json, table or text"
```

---

### Task 3: Paths and INI config and credentials files

**Files:**
- Create: `fluxgate-cli/src/config/paths.rs`, `fluxgate-cli/src/config/files.rs`
- Modify: `fluxgate-cli/src/config/mod.rs`

**Interfaces:**
- Consumes: `Env` (Task 1), `CliError` (Task 1).
- Produces: `Paths { config: PathBuf, credentials: PathBuf, sessions: PathBuf }` with `Paths::from_env(&Env) -> Result<Paths, CliError>`.
- Produces: `ConfigFiles { config: Ini, credentials: Ini, warnings: Vec<String> }` with `empty()`, `load(&Paths)`, `profile_value(&self, profile, key) -> Option<&str>`, `session_value(&self, session, key) -> Option<&str>`, `credential_token(&self, profile) -> Option<&str>`, `profile_names(&self) -> Vec<String>` (default first, then sorted), `has_profile(&self, profile) -> bool`, `set_profile_value(&mut self, profile, key, value)`, `remove_profile_value(&mut self, profile, key)`, `set_session_value(&mut self, session, key, value)`, `set_credential_token(&mut self, profile, token)`, `save_config(&self, &Paths)`, `save_credentials(&self, &Paths)`.
- Produces: `PROFILE_KEYS: [&str; 6]`, `profile_section(&str) -> String`, `session_section(&str) -> String`, `write_private(&Path, &[u8]) -> Result<(), CliError>`, `permission_warning(&Path) -> Option<String>`.

- [ ] **Step 1: Write the failing tests**

`src/config/paths.rs`:

```rust
//! Locations of the config file, credentials file and session cache.

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
```

`src/config/files.rs`:

```rust
//! The INI config and credentials files.

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn paths(dir: &Path) -> Paths {
        Paths {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
            sessions: dir.join("sessions"),
        }
    }

    #[test]
    fn missing_files_load_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let files = ConfigFiles::load(&paths(dir.path())).unwrap();
        assert!(files.profile_names().is_empty());
        assert!(files.warnings.is_empty());
    }

    #[test]
    fn reads_default_and_named_profiles_sessions_and_tokens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config"),
            "[default]\nteam = payments\n\n[profile prod]\nsession = corp\nteam = checkout\n\n[session corp]\nurl = https://fg.example.com/api/v1\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("credentials"), "[ci]\ntoken = abc\n").unwrap();
        let files = ConfigFiles::load(&paths(dir.path())).unwrap();
        assert_eq!(files.profile_value("default", "team"), Some("payments"));
        assert_eq!(files.profile_value("prod", "team"), Some("checkout"));
        assert_eq!(files.profile_value("prod", "missing"), None);
        assert_eq!(files.session_value("corp", "url"), Some("https://fg.example.com/api/v1"));
        assert_eq!(files.credential_token("ci"), Some("abc"));
        assert_eq!(files.profile_names(), vec!["default", "ci", "prod"]);
        assert!(files.has_profile("ci"));
        assert!(!files.has_profile("nope"));
    }

    #[test]
    fn save_keeps_unknown_keys_and_adds_new_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.config, "[profile prod]\nteam = checkout\ncustom_key = keep-me\n").unwrap();
        let mut files = ConfigFiles::load(&p).unwrap();
        files.set_profile_value("prod", "environment", "production");
        files.set_session_value("corp", "url", "https://fg.example.com/api/v1");
        files.save_config(&p).unwrap();
        let again = ConfigFiles::load(&p).unwrap();
        assert_eq!(again.profile_value("prod", "custom_key"), Some("keep-me"));
        assert_eq!(again.profile_value("prod", "environment"), Some("production"));
        assert_eq!(again.session_value("corp", "url"), Some("https://fg.example.com/api/v1"));
    }

    #[test]
    fn remove_profile_value_deletes_the_key() {
        let mut files = ConfigFiles::empty();
        files.set_profile_value("default", "url", "u");
        files.remove_profile_value("default", "url");
        assert_eq!(files.profile_value("default", "url"), None);
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_are_private_and_parent_dirs_are_created() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = paths(&dir.path().join("nested"));
        let mut files = ConfigFiles::empty();
        files.set_credential_token("ci", "secret");
        files.save_credentials(&p).unwrap();
        let mode = std::fs::metadata(&p.credentials).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn warns_when_credentials_are_readable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.credentials, "[ci]\ntoken = abc\n").unwrap();
        std::fs::set_permissions(&p.credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
        let files = ConfigFiles::load(&p).unwrap();
        assert_eq!(files.warnings.len(), 1);
        assert!(files.warnings[0].contains("chmod 600"));
    }

    #[test]
    fn unreadable_ini_is_a_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::write(&p.config, "[unclosed\n").unwrap();
        let err = ConfigFiles::load(&p).err().unwrap();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
    }
}
```

Update `src/config/mod.rs`:

```rust
//! Configuration: environment snapshot, config and credentials files, resolution.

pub mod env;
pub mod files;
pub mod paths;

pub use env::Env;
pub use files::ConfigFiles;
pub use paths::Paths;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli config::`
Expected: FAIL to compile (`cannot find struct Paths`, `ConfigFiles`).

- [ ] **Step 3: Implement `paths.rs`**

Insert above its tests module:

```rust
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
```

- [ ] **Step 4: Implement `files.rs`**

Insert above its tests module:

```rust
use std::io::Write;
use std::path::Path;

use ini::Ini;

use super::Paths;
use crate::error::CliError;

/// Keys a profile section may hold.
pub const PROFILE_KEYS: [&str; 6] = ["session", "url", "team", "environment", "output", "timeout"];

pub fn profile_section(profile: &str) -> String {
    if profile == "default" {
        "default".to_string()
    } else {
        format!("profile {profile}")
    }
}

pub fn session_section(session: &str) -> String {
    format!("session {session}")
}

pub struct ConfigFiles {
    pub config: Ini,
    pub credentials: Ini,
    /// Problems worth telling the user about, such as loose file permissions.
    pub warnings: Vec<String>,
}

impl ConfigFiles {
    pub fn empty() -> Self {
        Self { config: Ini::new(), credentials: Ini::new(), warnings: Vec::new() }
    }

    /// Missing files load as empty.
    pub fn load(paths: &Paths) -> Result<Self, CliError> {
        Ok(Self {
            config: load_ini(&paths.config)?,
            credentials: load_ini(&paths.credentials)?,
            warnings: permission_warning(&paths.credentials).into_iter().collect(),
        })
    }

    pub fn profile_value(&self, profile: &str, key: &str) -> Option<&str> {
        value_of(&self.config, profile_section(profile), key)
    }

    pub fn session_value(&self, session: &str, key: &str) -> Option<&str> {
        value_of(&self.config, session_section(session), key)
    }

    pub fn credential_token(&self, profile: &str) -> Option<&str> {
        value_of(&self.credentials, profile.to_string(), "token")
    }

    /// Profiles in either file: `default` first, then sorted.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .config
            .sections()
            .flatten()
            .filter_map(|section| {
                if section == "default" {
                    Some("default".to_string())
                } else {
                    section.strip_prefix("profile ").map(|name| name.trim().to_string())
                }
            })
            .collect();
        names.extend(self.credentials.sections().flatten().map(|s| s.trim().to_string()));
        names.sort();
        names.dedup();
        if let Some(position) = names.iter().position(|name| name == "default") {
            let default = names.remove(position);
            names.insert(0, default);
        }
        names
    }

    pub fn has_profile(&self, profile: &str) -> bool {
        self.profile_names().iter().any(|name| name == profile)
    }

    pub fn set_profile_value(&mut self, profile: &str, key: &str, value: &str) {
        self.config.with_section(Some(profile_section(profile))).set(key, value);
    }

    pub fn remove_profile_value(&mut self, profile: &str, key: &str) {
        self.config.delete_from(Some(profile_section(profile)), key);
    }

    pub fn set_session_value(&mut self, session: &str, key: &str, value: &str) {
        self.config.with_section(Some(session_section(session))).set(key, value);
    }

    pub fn set_credential_token(&mut self, profile: &str, token: &str) {
        self.credentials.with_section(Some(profile)).set("token", token);
    }

    pub fn save_config(&self, paths: &Paths) -> Result<(), CliError> {
        save_ini(&self.config, &paths.config)
    }

    pub fn save_credentials(&self, paths: &Paths) -> Result<(), CliError> {
        save_ini(&self.credentials, &paths.credentials)
    }
}

fn value_of(ini: &Ini, section: String, key: &str) -> Option<&str> {
    ini.section(Some(section))
        .and_then(|properties| properties.get(key))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn load_ini(path: &Path) -> Result<Ini, CliError> {
    if !path.exists() {
        return Ok(Ini::new());
    }
    Ini::load_from_file(path)
        .map_err(|err| CliError::Usage(format!("cannot read {}: {err}", path.display())))
}

fn save_ini(ini: &Ini, path: &Path) -> Result<(), CliError> {
    let mut buffer = Vec::new();
    ini.write_to(&mut buffer)?;
    write_private(path, &buffer)
}

/// Writes `contents` to `path` with mode 0600, creating parent directories.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // `mode` applies only to new files; tighten files that already existed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents)?;
    Ok(())
}

/// A warning when group or others can read `path`.
pub fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "warning: {} is readable by other users; run chmod 600 {}",
                path.display(),
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    None
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli config::`
Expected: PASS (10 tests including Task 1's env test).

- [ ] **Step 6: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): read and write config and credentials files"
```

---

### Task 4: Profile, settings and credential resolution

**Files:**
- Create: `fluxgate-cli/src/config/resolve.rs`
- Modify: `fluxgate-cli/src/config/mod.rs`

**Interfaces:**
- Consumes: `ConfigFiles`, `Env` (Tasks 1 and 3), `OutputFormat` (Task 2).
- Produces: `DEFAULT_URL`, `DEFAULT_TIMEOUT_SECS`; `Source::{Flag, Env, Profile, Session, Default}` with `as_str()`; `Resolved<T> { value: T, source: Source }`; `Overrides { profile, url, team, environment: Option<String>, output: Option<OutputFormat>, token: Option<String>, timeout: Option<u64> }` (`Default`); `Credential::{Static { token: String, source: Source }, Session { name: String }, None}` (redacting `Debug`); `Settings { profile: String, url: Resolved<String>, team: Option<Resolved<String>>, environment: Option<Resolved<String>>, output: Resolved<OutputFormat>, timeout: Resolved<u64>, session: Option<String>, sso_provider: Option<String>, credential: Credential }`; `resolve(&ConfigFiles, &Env, &Overrides, is_tty: bool) -> Result<Settings, CliError>`; `selected_profile(&Overrides, &Env) -> String`; `config::mask_token(&str) -> String`.

- [ ] **Step 1: Write the failing tests**

`src/config/resolve.rs`:

```rust
//! Which profile, settings and credentials a command uses.

#[cfg(test)]
mod tests {
    use super::*;
    use ini::Ini;

    const CONFIG: &str = "[default]\nsession = corp\nteam = payments\noutput = text\n\n[profile prod]\nsession = corp\nteam = checkout\nenvironment = production\n\n[profile ci]\nurl = https://ci.example.com/api/v1/\n\n[session corp]\nurl = https://fg.example.com/api/v1\nsso_provider = okta\n";
    const CREDS: &str = "[ci]\ntoken = static-ci\n";

    fn files() -> ConfigFiles {
        ConfigFiles {
            config: Ini::load_from_str(CONFIG).unwrap(),
            credentials: Ini::load_from_str(CREDS).unwrap(),
            warnings: Vec::new(),
        }
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        Env::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn defaults_without_files() {
        let s = resolve(&ConfigFiles::empty(), &Env::default(), &Overrides::default(), false).unwrap();
        assert_eq!(s.profile, "default");
        assert_eq!(s.url, Resolved { value: DEFAULT_URL.to_string(), source: Source::Default });
        assert_eq!(s.team, None);
        assert_eq!(s.output.value, OutputFormat::Json);
        assert_eq!(s.timeout.value, 30);
        assert_eq!(s.credential, Credential::None);
        let tty = resolve(&ConfigFiles::empty(), &Env::default(), &Overrides::default(), true).unwrap();
        assert_eq!(tty.output.value, OutputFormat::Table);
    }

    #[test]
    fn profile_values_and_session_url() {
        let s = resolve(&files(), &Env::default(), &Overrides::default(), false).unwrap();
        assert_eq!(s.url, Resolved { value: "https://fg.example.com/api/v1".into(), source: Source::Session });
        assert_eq!(s.team, Some(Resolved { value: "payments".into(), source: Source::Profile }));
        assert_eq!(s.output, Resolved { value: OutputFormat::Text, source: Source::Profile });
        assert_eq!(s.credential, Credential::Session { name: "corp".into() });
        assert_eq!(s.session.as_deref(), Some("corp"));
        assert_eq!(s.sso_provider.as_deref(), Some("okta"));
    }

    #[test]
    fn flag_beats_env_beats_profile() {
        let overrides = Overrides { team: Some("flag-team".into()), ..Overrides::default() };
        let s = resolve(&files(), &env(&[("FLUXGATE_TEAM", "env-team")]), &overrides, false).unwrap();
        assert_eq!(s.team, Some(Resolved { value: "flag-team".into(), source: Source::Flag }));
        let s = resolve(&files(), &env(&[("FLUXGATE_TEAM", "env-team")]), &Overrides::default(), false).unwrap();
        assert_eq!(s.team, Some(Resolved { value: "env-team".into(), source: Source::Env }));
    }

    #[test]
    fn legacy_env_names_still_work_and_new_names_win() {
        let s = resolve(&ConfigFiles::empty(), &env(&[("FLUXGATE_TEAM_ID", "t-old"), ("FLUXGATE_ENVIRONMENT_ID", "e-old")]), &Overrides::default(), false).unwrap();
        assert_eq!(s.team.unwrap().value, "t-old");
        assert_eq!(s.environment.unwrap().value, "e-old");
        let s = resolve(&ConfigFiles::empty(), &env(&[("FLUXGATE_TEAM_ID", "t-old"), ("FLUXGATE_TEAM", "t-new")]), &Overrides::default(), false).unwrap();
        assert_eq!(s.team.unwrap().value, "t-new");
    }

    #[test]
    fn profile_comes_from_flag_then_env() {
        let s = resolve(&files(), &env(&[("FLUXGATE_PROFILE", "prod")]), &Overrides::default(), false).unwrap();
        assert_eq!(s.profile, "prod");
        assert_eq!(s.team.unwrap().value, "checkout");
        assert_eq!(s.environment.unwrap().value, "production");
        let overrides = Overrides { profile: Some("ci".into()), ..Overrides::default() };
        let s = resolve(&files(), &env(&[("FLUXGATE_PROFILE", "prod")]), &overrides, false).unwrap();
        assert_eq!(s.profile, "ci");
    }

    #[test]
    fn missing_profile_is_a_usage_error_naming_it() {
        let err = resolve(&files(), &env(&[("FLUXGATE_PROFILE", "nope")]), &Overrides::default(), false).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        assert!(err.to_string().contains("'nope'"));
    }

    #[test]
    fn credential_order_is_flag_env_file_session() {
        let ci = Overrides { profile: Some("ci".into()), ..Overrides::default() };
        let s = resolve(&files(), &Env::default(), &ci, false).unwrap();
        assert_eq!(s.credential, Credential::Static { token: "static-ci".into(), source: Source::Profile });
        let s = resolve(&files(), &env(&[("FLUXGATE_TOKEN", "from-env")]), &ci, false).unwrap();
        assert_eq!(s.credential, Credential::Static { token: "from-env".into(), source: Source::Env });
        let flag = Overrides { token: Some("from-flag".into()), ..ci };
        let s = resolve(&files(), &env(&[("FLUXGATE_TOKEN", "from-env")]), &flag, false).unwrap();
        assert_eq!(s.credential, Credential::Static { token: "from-flag".into(), source: Source::Flag });
    }

    #[test]
    fn url_trailing_slash_is_trimmed() {
        let ci = Overrides { profile: Some("ci".into()), ..Overrides::default() };
        let s = resolve(&files(), &Env::default(), &ci, false).unwrap();
        assert_eq!(s.url.value, "https://ci.example.com/api/v1");
    }

    #[test]
    fn invalid_output_and_timeout_are_usage_errors() {
        let err = resolve(&ConfigFiles::empty(), &env(&[("FLUXGATE_OUTPUT", "yaml")]), &Overrides::default(), false).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        let err = resolve(&ConfigFiles::empty(), &env(&[("FLUXGATE_TIMEOUT", "soon")]), &Overrides::default(), false).unwrap_err();
        assert!(err.to_string().contains("invalid timeout 'soon'"));
    }

    #[test]
    fn credential_debug_hides_the_token() {
        let shown = format!("{:?}", Credential::Static { token: "secret-token".into(), source: Source::Env });
        assert!(!shown.contains("secret-token"));
    }

    #[test]
    fn selected_profile_defaults_to_default() {
        assert_eq!(selected_profile(&Overrides::default(), &Env::default()), "default");
        assert_eq!(selected_profile(&Overrides::default(), &env(&[("FLUXGATE_PROFILE", "prod")])), "prod");
    }
}
```

Add to `src/config/mod.rs`:

```rust
pub mod resolve;

pub use resolve::{Credential, Overrides, Resolved, Settings, Source, resolve, selected_profile};

/// `****` followed by the last 4 characters.
pub fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("****{tail}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn mask_token_keeps_the_last_four_characters() {
        assert_eq!(super::mask_token("abcdef123456"), "****3456");
        assert_eq!(super::mask_token("ab"), "****ab");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli config::`
Expected: FAIL to compile (`cannot find function resolve`).

- [ ] **Step 3: Implement**

Insert above the tests module of `src/config/resolve.rs`:

```rust
use serde::Serialize;

use super::{ConfigFiles, Env};
use crate::error::CliError;
use crate::output::OutputFormat;

pub const DEFAULT_URL: &str = "http://localhost:8080/api/v1";
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Flag,
    Env,
    Profile,
    Session,
    Default,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Flag => "flag",
            Source::Env => "env",
            Source::Profile => "profile",
            Source::Session => "session",
            Source::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: Source,
}

/// Values given as global command line flags.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub profile: Option<String>,
    pub url: Option<String>,
    pub team: Option<String>,
    pub environment: Option<String>,
    pub output: Option<OutputFormat>,
    pub token: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// A bearer token from a flag, env var or the credentials file.
    Static { token: String, source: Source },
    /// A cached login session.
    Session { name: String },
    None,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::Static { source, .. } => {
                write!(f, "Static {{ token: <redacted>, source: {source:?} }}")
            }
            Credential::Session { name } => write!(f, "Session {{ name: {name:?} }}"),
            Credential::None => write!(f, "None"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub profile: String,
    pub url: Resolved<String>,
    pub team: Option<Resolved<String>>,
    pub environment: Option<Resolved<String>>,
    pub output: Resolved<OutputFormat>,
    pub timeout: Resolved<u64>,
    pub session: Option<String>,
    pub sso_provider: Option<String>,
    pub credential: Credential,
}

/// `--profile`, then `FLUXGATE_PROFILE`, then `default`.
pub fn selected_profile(overrides: &Overrides, env: &Env) -> String {
    overrides
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .or_else(|| env.get("FLUXGATE_PROFILE"))
        .unwrap_or("default")
        .to_string()
}

pub fn resolve(
    files: &ConfigFiles,
    env: &Env,
    overrides: &Overrides,
    is_tty: bool,
) -> Result<Settings, CliError> {
    let profile = selected_profile(overrides, env);
    // A named profile that does not exist must not fall back to default, which
    // could point at another environment.
    if profile != "default" && !files.has_profile(&profile) {
        return Err(CliError::Usage(format!(
            "profile '{profile}' not found: run fluxgate configure --profile {profile}"
        )));
    }
    let session = files.profile_value(&profile, "session").map(str::to_string);

    let pick = |flag: Option<String>, env_keys: &[&str], key: &str| -> Option<Resolved<String>> {
        if let Some(value) = flag.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            return Some(Resolved { value: value.to_string(), source: Source::Flag });
        }
        if let Some(value) = env.first(env_keys) {
            return Some(Resolved { value: value.to_string(), source: Source::Env });
        }
        files
            .profile_value(&profile, key)
            .map(|value| Resolved { value: value.to_string(), source: Source::Profile })
    };

    let url = pick(overrides.url.clone(), &["FLUXGATE_URL"], "url")
        .or_else(|| {
            session
                .as_deref()
                .and_then(|name| files.session_value(name, "url"))
                .map(|value| Resolved { value: value.to_string(), source: Source::Session })
        })
        .unwrap_or(Resolved { value: DEFAULT_URL.to_string(), source: Source::Default });
    let url = Resolved { value: url.value.trim_end_matches('/').to_string(), source: url.source };

    let team = pick(overrides.team.clone(), &["FLUXGATE_TEAM", "FLUXGATE_TEAM_ID"], "team");
    let environment = pick(
        overrides.environment.clone(),
        &["FLUXGATE_ENVIRONMENT", "FLUXGATE_ENVIRONMENT_ID"],
        "environment",
    );

    let output = match pick(
        overrides.output.map(|o| o.as_str().to_string()),
        &["FLUXGATE_OUTPUT"],
        "output",
    ) {
        Some(raw) => Resolved {
            value: raw.value.parse::<OutputFormat>().map_err(CliError::Usage)?,
            source: raw.source,
        },
        None => Resolved {
            value: if is_tty { OutputFormat::Table } else { OutputFormat::Json },
            source: Source::Default,
        },
    };

    let timeout = match pick(overrides.timeout.map(|t| t.to_string()), &["FLUXGATE_TIMEOUT"], "timeout") {
        Some(raw) => Resolved {
            value: raw.value.parse::<u64>().map_err(|_| {
                CliError::Usage(format!("invalid timeout '{}': expected seconds", raw.value))
            })?,
            source: raw.source,
        },
        None => Resolved { value: DEFAULT_TIMEOUT_SECS, source: Source::Default },
    };

    let credential = if let Some(token) =
        overrides.token.as_deref().map(str::trim).filter(|t| !t.is_empty())
    {
        Credential::Static { token: token.to_string(), source: Source::Flag }
    } else if let Some(token) = env.get("FLUXGATE_TOKEN") {
        Credential::Static { token: token.to_string(), source: Source::Env }
    } else if let Some(token) = files.credential_token(&profile) {
        Credential::Static { token: token.to_string(), source: Source::Profile }
    } else if let Some(name) = &session {
        Credential::Session { name: name.clone() }
    } else {
        Credential::None
    };

    let sso_provider = session
        .as_deref()
        .and_then(|name| files.session_value(name, "sso_provider"))
        .map(str::to_string);

    Ok(Settings {
        profile,
        url,
        team,
        environment,
        output,
        timeout,
        session,
        sso_provider,
        credential,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli config::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): resolve profile, settings and credentials like the AWS CLI"
```

---

### Task 5: HTTP client

**Files:**
- Create: `fluxgate-cli/src/api.rs`
- Modify: `fluxgate-cli/src/lib.rs` (add `pub mod api;`)

**Interfaces:**
- Consumes: `CliError` (Task 1).
- Produces: `ApiClient` (`Clone`, redacting `Debug`) with `new(base_url: &str, token: Option<String>, timeout: Duration) -> Result<ApiClient, CliError>`, `with_token(&self, Option<String>) -> ApiClient`, `url(&self, &[&str]) -> reqwest::Url` (each segment percent-encoded), `get(&self, &[&str], &[(&str, String)]) -> Result<Value, CliError>`, `post(&self, &[&str], &Value) -> Result<Value, CliError>`, `get_all_pages(&self, &[&str], &[(&str, String)]) -> Result<Vec<Value>, CliError>`; `PAGE_SIZE: i64 = 200`.

- [ ] **Step 1: Write the failing tests**

`src/api.rs`:

```rust
//! HTTP client for the FluxGate REST API.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{EXIT_NOT_FOUND, EXIT_SERVER, EXIT_USAGE};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer, token: Option<&str>) -> ApiClient {
        ApiClient::new(&format!("{}/api/v1", server.uri()), token.map(str::to_string), Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn url_segments_are_encoded_and_trailing_slash_ignored() {
        let api = ApiClient::new("http://h/api/v1/", None, Duration::from_secs(1)).unwrap();
        assert_eq!(
            api.url(&["teams", "a b/c", "features"]).as_str(),
            "http://h/api/v1/teams/a%20b%2Fc/features"
        );
    }

    #[test]
    fn invalid_url_is_a_usage_error() {
        let err = ApiClient::new("not a url", None, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
    }

    #[test]
    fn debug_hides_the_token() {
        let api = ApiClient::new("http://h/api/v1", Some("secret-token".into()), Duration::from_secs(1)).unwrap();
        assert!(!format!("{api:?}").contains("secret-token"));
    }

    #[tokio::test]
    async fn sends_bearer_token_and_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/teams/t1/approval-requests"))
            .and(header("authorization", "Bearer tok"))
            .and(query_param("statuses", "pending"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
            .expect(1)
            .mount(&server)
            .await;
        let value = client(&server, Some("tok"))
            .get(&["teams", "t1", "approval-requests"], &[("statuses", "pending".into())])
            .await
            .unwrap();
        assert_eq!(value, json!({ "items": [] }));
    }

    #[tokio::test]
    async fn error_body_becomes_an_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": "not_found", "message": "feature not found", "code": "feature_not_found", "details": null
            })))
            .mount(&server)
            .await;
        let err = client(&server, None).get(&["features", "x"], &[]).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_NOT_FOUND);
        assert_eq!(err.to_string(), "feature not found (code feature_not_found, HTTP 404)");
    }

    #[tokio::test]
    async fn non_json_proxy_error_still_maps_the_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
            .mount(&server)
            .await;
        let err = client(&server, None).get(&["health"], &[]).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_SERVER);
        assert!(err.to_string().contains("Bad Gateway"));
        assert!(err.to_string().contains("HTTP 502"));
    }

    #[tokio::test]
    async fn empty_success_body_is_an_empty_object() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let value = client(&server, None).post(&["auth", "logout"], &json!({})).await.unwrap();
        assert_eq!(value, json!({}));
    }

    #[tokio::test]
    async fn timeout_is_a_network_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let api = ApiClient::new(&server.uri(), None, Duration::from_millis(200)).unwrap();
        let err = api.get(&["health"], &[]).await.unwrap_err();
        assert!(matches!(err, CliError::Network(_)));
        assert_eq!(err.exit_code(), EXIT_SERVER);
    }

    #[tokio::test]
    async fn get_all_pages_follows_meta_total() {
        let server = MockServer::start().await;
        let page = |ids: &[&str], offset: i64| {
            json!({ "items": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
                    "meta": { "offset": offset, "limit": 200, "total": 3 } })
        };
        Mock::given(method("GET"))
            .and(query_param("offset", "0"))
            .and(query_param("limit", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&["a", "b"], 0)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("offset", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&["c"], 2)))
            .expect(1)
            .mount(&server)
            .await;
        let items = client(&server, None).get_all_pages(&["teams", "t", "features"], &[]).await.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[2]["id"], "c");
    }
}
```

Add `pub mod api;` to `src/lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli api::`
Expected: FAIL to compile (`cannot find struct ApiClient`).

- [ ] **Step 3: Implement**

Insert above the tests module:

```rust
use std::time::Duration;

use reqwest::{Method, Url};
use serde_json::{Value, json};

use crate::error::CliError;

/// Page size used when fetching every page (the server maximum).
pub const PAGE_SIZE: i64 = 200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest part of a non-JSON error body kept in the message.
const MAX_ERROR_TEXT: usize = 200;

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base", &self.base.as_str())
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl ApiClient {
    pub fn new(base_url: &str, token: Option<String>, timeout: Duration) -> Result<Self, CliError> {
        let base = Url::parse(base_url.trim().trim_end_matches('/'))
            .map_err(|err| CliError::Usage(format!("invalid url '{base_url}': {err}")))?;
        if base.cannot_be_a_base() {
            return Err(CliError::Usage(format!("invalid url '{base_url}'")));
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|err| CliError::Other(err.to_string()))?;
        Ok(Self {
            http,
            base,
            token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        })
    }

    pub fn with_token(&self, token: Option<String>) -> Self {
        Self { token, ..self.clone() }
    }

    /// Base URL plus `segments`, each percent-encoded as one path segment.
    pub fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("checked in ApiClient::new")
            .pop_if_empty()
            .extend(segments);
        url
    }

    pub async fn get(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Value, CliError> {
        self.send(Method::GET, segments, query, None).await
    }

    pub async fn post(&self, segments: &[&str], body: &Value) -> Result<Value, CliError> {
        self.send(Method::POST, segments, &[], Some(body)).await
    }

    /// Follows `meta.total` of a paginated list and returns all items.
    pub async fn get_all_pages(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<Vec<Value>, CliError> {
        let mut items = Vec::new();
        let mut offset: i64 = 0;
        loop {
            let mut page_query = query.to_vec();
            page_query.push(("limit", PAGE_SIZE.to_string()));
            page_query.push(("offset", offset.to_string()));
            let page = self.get(segments, &page_query).await?;
            let batch = page
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| CliError::Other("unexpected list response: no items".into()))?;
            let total = page.pointer("/meta/total").and_then(Value::as_i64).unwrap_or(0);
            let count = batch.len() as i64;
            items.extend(batch);
            offset += count;
            if count == 0 || offset >= total {
                return Ok(items);
            }
        }
    }

    async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, CliError> {
        let mut request = self.http.request(method, self.url(segments));
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(network_error)?;
        decode(response).await
    }
}

fn network_error(err: reqwest::Error) -> CliError {
    if err.is_timeout() {
        CliError::Network(format!("request timed out: {err}"))
    } else if err.is_connect() {
        CliError::Network(format!("cannot connect: {err}"))
    } else {
        CliError::Network(err.to_string())
    }
}

async fn decode(response: reqwest::Response) -> Result<Value, CliError> {
    let status = response.status();
    let text = response.text().await.map_err(network_error)?;
    let body: Value = if text.trim().is_empty() {
        json!({})
    } else {
        // Proxies answer with HTML; keep the start of it as the message.
        serde_json::from_str(&text).unwrap_or_else(|_| {
            json!({ "message": text.chars().take(MAX_ERROR_TEXT).collect::<String>() })
        })
    };
    if status.is_success() {
        return Ok(body);
    }
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("request failed"))
        .to_string();
    Err(CliError::Api { status: status.as_u16(), code, message, body })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli api::`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): HTTP client with timeouts, encoding and error mapping"
```

---

### Task 6: Token claims and the session cache with locked refresh

**Files:**
- Create: `fluxgate-cli/src/auth/mod.rs`, `fluxgate-cli/src/auth/claims.rs`, `fluxgate-cli/src/auth/session.rs`
- Modify: `fluxgate-cli/src/lib.rs` (add `pub mod auth;`)

**Interfaces:**
- Consumes: `ApiClient` (Task 5), `write_private` (Task 3), `CliError`.
- Produces: `TokenClaims { sub: String, username: String, is_admin: bool, exp: i64, token_type: String, team_id: Option<String> }` with `is_system_client()`; `decode_claims(&str) -> Option<TokenClaims>`.
- Produces: `REFRESH_MARGIN_SECS: i64 = 60`; `SessionUser { id: String, username: String }`; `SessionCache { access_token, refresh_token: String, expires_at: DateTime<Utc>, user: SessionUser }` with `from_login(&LoginResponse, DateTime<Utc>)`, `needs_refresh(&self, DateTime<Utc>) -> bool`; `LoginResponse { token, refresh_token: String, expires_in: i64, user: SessionUser, is_temporary: bool }` (Deserialize, camelCase); `SessionStore::new(PathBuf)` with `load(&self, name) -> Result<Option<SessionCache>, CliError>`, `save(&self, name, &SessionCache)`, `delete(&self, name) -> Result<bool, CliError>`, `names(&self) -> Result<Vec<String>, CliError>`, `access_token(&self, name: &str, profile: &str, base_url: &str, timeout: Duration) -> Result<String, CliError>`.

- [ ] **Step 1: Write the failing tests**

`src/auth/mod.rs`:

```rust
//! Login sessions and token handling.

pub mod claims;
pub mod session;
```

`src/auth/claims.rs`:

```rust
//! Reading JWT claims for display and the team check.

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    fn jwt(claims: serde_json::Value) -> String {
        format!("{}.{}.sig", URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#), URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn decodes_system_client_claims() {
        let claims = decode_claims(&jwt(json!({
            "sub": "sc1", "username": "ci-bot", "is_admin": false, "exp": 4102444800i64,
            "token_type": "system_client", "team_id": "t1", "roles": [], "iat": 1
        })))
        .unwrap();
        assert!(claims.is_system_client());
        assert_eq!(claims.team_id.as_deref(), Some("t1"));
        assert_eq!(claims.exp, 4102444800);
    }

    #[test]
    fn user_tokens_default_to_user_type() {
        let claims = decode_claims(&jwt(json!({ "sub": "u1", "username": "alice", "exp": 1 }))).unwrap();
        assert!(!claims.is_system_client());
        assert_eq!(claims.token_type, "user");
    }

    #[test]
    fn opaque_tokens_have_no_claims() {
        assert_eq!(decode_claims("not-a-jwt"), None);
        assert_eq!(decode_claims("a.!!!.c"), None);
    }
}
```

`src/auth/session.rs`:

```rust
//! Cached login sessions under the sessions directory.

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
```

Add `pub mod auth;` to `src/lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli auth::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `claims.rs`**

Insert above its tests module:

```rust
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TokenClaims {
    pub sub: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub exp: i64,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub team_id: Option<String>,
}

fn default_token_type() -> String {
    "user".to_string()
}

impl TokenClaims {
    pub fn is_system_client(&self) -> bool {
        self.token_type == "system_client"
    }
}

/// Payload of a JWT, without checking the signature. Use it only for display
/// and to stop requests that the server would answer for another team.
pub fn decode_claims(token: &str) -> Option<TokenClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}
```

- [ ] **Step 4: Implement `session.rs`**

Insert above its tests module:

```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli auth::`
Expected: PASS (11 tests). If `concurrent_refreshes_send_one_request` fails with "expected 1 request, got 2", the lock is not held across the refresh; check `_lock` is bound (not `_`).

- [ ] **Step 6: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): session cache with locked token refresh"
```

---

### Task 7: Prompts and the command context

**Files:**
- Create: `fluxgate-cli/src/prompt.rs`, `fluxgate-cli/src/context.rs`
- Modify: `fluxgate-cli/src/lib.rs` (add `pub mod context;` and `pub mod prompt;`)

**Interfaces:**
- Consumes: `Settings`, `Credential`, `Paths`, `resolve`, `ConfigFiles`, `Env`, `Overrides` (Tasks 3–4); `ApiClient` (Task 5); `SessionStore`, `TokenClaims`, `decode_claims` (Task 6).
- Produces: `trait Prompter { fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError>; fn secret(&mut self, label: &str) -> Result<String, CliError>; fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError>; }`; `TerminalPrompter`; `ScriptedPrompter::new(&[&str])` (select answers are option text or an index; field `asked: Vec<String>`).
- Produces: `Team { id: String, name: String, description: Option<String> }`; `Environment { id: String, name: String, active: bool }`; `is_uuid(&str) -> bool`; `find_team(&[Team], &str) -> Result<Team, CliError>`; `team_names(&[Team]) -> String`; `Context { settings: Settings, paths: Paths, api: ApiClient, claims: Option<TokenClaims> }` with `Context::timeout(&Settings) -> Duration`, `anonymous(Settings, Paths) -> Result<Context, CliError>`, `async connect(Settings, Paths) -> Result<Context, CliError>`, `token_team(&self) -> Option<&str>`, `async teams(&self) -> Result<Vec<Team>, CliError>`, `async team_id(&self) -> Result<String, CliError>`, `async environments(&self, team_id: &str) -> Result<Vec<Environment>, CliError>`, `async environment_id(&self, team_id: &str) -> Result<String, CliError>`, `async environment_name(&self, team_id: &str) -> Result<String, CliError>`.

- [ ] **Step 1: Write the failing tests**

`src/prompt.rs`:

```rust
//! Interactive questions, replaceable in tests.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_answers_in_order_with_defaults_and_selection_by_text() {
        let options = vec!["table".to_string(), "json".to_string()];
        let mut prompter = ScriptedPrompter::new(&["", "alice", "pw", "json", "0"]);
        assert_eq!(prompter.input("URL", Some("http://h")).unwrap(), "http://h");
        assert_eq!(prompter.input("Username", None).unwrap(), "alice");
        assert_eq!(prompter.secret("Password").unwrap(), "pw");
        assert_eq!(prompter.select("Output", &options).unwrap(), 1);
        assert_eq!(prompter.select("Output", &options).unwrap(), 0);
        assert_eq!(prompter.asked, vec!["URL", "Username", "Password", "Output", "Output"]);
        assert!(prompter.input("More", None).is_err());
    }

    #[test]
    fn scripted_select_rejects_unknown_answers() {
        let mut prompter = ScriptedPrompter::new(&["yaml"]);
        assert!(prompter.select("Output", &["json".to_string()]).is_err());
    }
}
```

`src/context.rs`:

```rust
//! Authenticated API access plus team and environment resolution.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigFiles, Env, Overrides, resolve};
    use crate::error::{EXIT_AUTH, EXIT_USAGE};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEAM_A: &str = "11111111-1111-1111-1111-111111111111";
    const TEAM_B: &str = "22222222-2222-2222-2222-222222222222";
    const ENV_ID: &str = "33333333-3333-3333-3333-333333333333";

    fn jwt(claims: serde_json::Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    async fn context(server: &MockServer, token: &str, pairs: &[(&str, &str)]) -> Context {
        let url = format!("{}/api/v1", server.uri());
        let mut all = vec![("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token)];
        all.extend_from_slice(pairs);
        let settings = resolve(&ConfigFiles::empty(), &Env::from_pairs(all), &Overrides::default(), false).unwrap();
        let dir = std::env::temp_dir();
        let paths = Paths { config: dir.join("c"), credentials: dir.join("k"), sessions: dir.join("s") };
        Context::connect(settings, paths).await.unwrap()
    }

    fn user() -> String {
        jwt(json!({ "sub": "u1", "username": "alice", "exp": 4102444800i64 }))
    }

    fn system(team: &str) -> String {
        jwt(json!({ "sub": "sc1", "username": "ci", "exp": 4102444800i64, "token_type": "system_client", "team_id": team }))
    }

    async fn mount_teams(server: &MockServer, teams: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/api/v1/teams"))
            .respond_with(ResponseTemplate::new(200).set_body_json(teams))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn uuid_team_is_used_without_a_request() {
        let server = MockServer::start().await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", TEAM_A)]).await;
        assert_eq!(ctx.team_id().await.unwrap(), TEAM_A);
    }

    #[tokio::test]
    async fn team_name_is_matched_ignoring_case() {
        let server = MockServer::start().await;
        mount_teams(&server, json!([{ "id": TEAM_A, "name": "Payments" }, { "id": TEAM_B, "name": "Checkout" }])).await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", "payments")]).await;
        assert_eq!(ctx.team_id().await.unwrap(), TEAM_A);
    }

    #[tokio::test]
    async fn unknown_team_lists_the_available_names() {
        let server = MockServer::start().await;
        mount_teams(&server, json!([{ "id": TEAM_A, "name": "Payments" }])).await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", "billing")]).await;
        let err = ctx.team_id().await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.to_string().contains("available: Payments"));
    }

    #[tokio::test]
    async fn duplicate_team_names_ask_for_the_id() {
        let server = MockServer::start().await;
        mount_teams(&server, json!([{ "id": TEAM_A, "name": "Ops" }, { "id": TEAM_B, "name": "ops" }])).await;
        let ctx = context(&server, &user(), &[("FLUXGATE_TEAM", "OPS")]).await;
        let err = ctx.team_id().await.unwrap_err();
        assert!(err.to_string().contains(TEAM_A) && err.to_string().contains(TEAM_B));
    }

    #[tokio::test]
    async fn missing_team_for_a_user_token_is_a_usage_error() {
        let server = MockServer::start().await;
        let ctx = context(&server, &user(), &[]).await;
        assert!(ctx.team_id().await.unwrap_err().to_string().starts_with("team required"));
    }

    #[tokio::test]
    async fn system_token_supplies_and_guards_the_team() {
        let server = MockServer::start().await;
        let ctx = context(&server, &system(TEAM_A), &[]).await;
        assert_eq!(ctx.token_team(), Some(TEAM_A));
        assert_eq!(ctx.team_id().await.unwrap(), TEAM_A);
        let ctx = context(&server, &system(TEAM_A), &[("FLUXGATE_TEAM", TEAM_B)]).await;
        let err = ctx.team_id().await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.to_string().contains("does not match the system-client token's team"));
    }

    #[tokio::test]
    async fn environment_names_and_ids_are_resolved() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [{ "id": ENV_ID, "name": "staging", "teamId": TEAM_A, "active": true, "environmentType": "STAGING" }],
                "meta": { "offset": 0, "limit": 200, "total": 1 }
            })))
            .mount(&server)
            .await;
        let ctx = context(&server, &user(), &[("FLUXGATE_ENVIRONMENT", "Staging")]).await;
        assert_eq!(ctx.environment_id(TEAM_A).await.unwrap(), ENV_ID);
        assert_eq!(ctx.environment_name(TEAM_A).await.unwrap(), "Staging");
        let ctx = context(&server, &user(), &[("FLUXGATE_ENVIRONMENT", ENV_ID)]).await;
        assert_eq!(ctx.environment_id(TEAM_A).await.unwrap(), ENV_ID);
        assert_eq!(ctx.environment_name(TEAM_A).await.unwrap(), "staging");
        let ctx = context(&server, &user(), &[("FLUXGATE_ENVIRONMENT", "prod")]).await;
        assert!(ctx.environment_id(TEAM_A).await.unwrap_err().to_string().contains("available: staging"));
    }

    #[tokio::test]
    async fn connect_without_credentials_is_an_auth_error() {
        let settings = resolve(&ConfigFiles::empty(), &Env::default(), &Overrides::default(), false).unwrap();
        let dir = std::env::temp_dir();
        let paths = Paths { config: dir.join("c"), credentials: dir.join("k"), sessions: dir.join("s") };
        let err = Context::connect(settings, paths).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_AUTH);
        assert_eq!(err.to_string(), "no credentials for profile default: run fluxgate configure or fluxgate login");
    }
}
```

Add `pub mod context;` and `pub mod prompt;` to `src/lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli -- prompt:: context::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `prompt.rs`**

Insert above its tests module:

```rust
use std::collections::VecDeque;
use std::io::{BufRead, Write};

use crate::error::CliError;

pub trait Prompter {
    /// One line of text; an empty answer gives `default`.
    fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError>;
    /// Hidden input such as a password or token.
    fn secret(&mut self, label: &str) -> Result<String, CliError>;
    /// Index of the chosen option.
    fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError>;
}

/// Asks on stderr and reads stdin, so stdout stays clean for results.
pub struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError> {
        let mut err = std::io::stderr();
        match default {
            Some(default) => write!(err, "{label} [{default}]: ")?,
            None => write!(err, "{label}: ")?,
        }
        err.flush()?;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            return Err(CliError::Usage(format!("no input for '{label}'")));
        }
        let line = line.trim();
        Ok(if line.is_empty() { default.unwrap_or_default().to_string() } else { line.to_string() })
    }

    fn secret(&mut self, label: &str) -> Result<String, CliError> {
        rpassword::prompt_password(format!("{label}: "))
            .map_err(|err| CliError::Usage(format!("cannot read '{label}': {err}")))
    }

    fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError> {
        if options.is_empty() {
            return Err(CliError::Usage(format!("nothing to choose for '{label}'")));
        }
        let mut err = std::io::stderr();
        writeln!(err, "{label}:")?;
        for (index, option) in options.iter().enumerate() {
            writeln!(err, "  {}) {option}", index + 1)?;
        }
        loop {
            let answer = self.input("Choose a number", Some("1"))?;
            match answer.parse::<usize>() {
                Ok(number) if (1..=options.len()).contains(&number) => return Ok(number - 1),
                _ => writeln!(err, "enter a number from 1 to {}", options.len())?,
            }
        }
    }
}

/// Answers questions from a list, for tests.
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    answers: VecDeque<String>,
    /// Labels of the questions asked, in order.
    pub asked: Vec<String>,
}

impl ScriptedPrompter {
    pub fn new(answers: &[&str]) -> Self {
        Self { answers: answers.iter().map(|a| a.to_string()).collect(), asked: Vec::new() }
    }

    fn next(&mut self, label: &str) -> Result<String, CliError> {
        self.asked.push(label.to_string());
        self.answers
            .pop_front()
            .ok_or_else(|| CliError::Usage(format!("no scripted answer for '{label}'")))
    }
}

impl Prompter for ScriptedPrompter {
    fn input(&mut self, label: &str, default: Option<&str>) -> Result<String, CliError> {
        let answer = self.next(label)?;
        Ok(if answer.is_empty() { default.unwrap_or_default().to_string() } else { answer })
    }

    fn secret(&mut self, label: &str) -> Result<String, CliError> {
        self.next(label)
    }

    fn select(&mut self, label: &str, options: &[String]) -> Result<usize, CliError> {
        let answer = self.next(label)?;
        options
            .iter()
            .position(|option| option == &answer)
            .or_else(|| answer.parse::<usize>().ok().filter(|index| *index < options.len()))
            .ok_or_else(|| {
                CliError::Usage(format!("scripted answer '{answer}' is not an option for '{label}'"))
            })
    }
}
```

- [ ] **Step 4: Implement `context.rs`**

Insert above its tests module:

```rust
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::ApiClient;
use crate::auth::claims::{TokenClaims, decode_claims};
use crate::auth::session::SessionStore;
use crate::config::{Credential, Paths, Settings};
use crate::error::CliError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub active: bool,
}

pub fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value.trim()).is_ok()
}

pub fn team_names(teams: &[Team]) -> String {
    teams.iter().map(|team| team.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The one team whose name matches `wanted`, ignoring case.
pub fn find_team(teams: &[Team], wanted: &str) -> Result<Team, CliError> {
    let wanted = wanted.trim();
    let matches: Vec<&Team> = teams.iter().filter(|team| team.name.eq_ignore_ascii_case(wanted)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CliError::Usage(format!("team '{wanted}' not found; available: {}", team_names(teams)))),
        many => Err(CliError::Usage(format!(
            "team name '{wanted}' matches several teams ({}); use the team id",
            many.iter().map(|team| team.id.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

#[derive(Debug)]
pub struct Context {
    pub settings: Settings,
    pub paths: Paths,
    pub api: ApiClient,
    pub claims: Option<TokenClaims>,
}

impl Context {
    pub fn timeout(settings: &Settings) -> Duration {
        Duration::from_secs(settings.timeout.value)
    }

    /// A client without credentials, for public routes.
    pub fn anonymous(settings: Settings, paths: Paths) -> Result<Self, CliError> {
        let api = ApiClient::new(&settings.url.value, None, Self::timeout(&settings))?;
        Ok(Self { settings, paths, api, claims: None })
    }

    pub async fn connect(settings: Settings, paths: Paths) -> Result<Self, CliError> {
        let token = match &settings.credential {
            Credential::Static { token, .. } => token.clone(),
            Credential::Session { name } => {
                SessionStore::new(paths.sessions.clone())
                    .access_token(name, &settings.profile, &settings.url.value, Self::timeout(&settings))
                    .await?
            }
            Credential::None => {
                return Err(CliError::Auth(format!(
                    "no credentials for profile {}: run fluxgate configure or fluxgate login",
                    settings.profile
                )));
            }
        };
        let claims = decode_claims(&token);
        let api = ApiClient::new(&settings.url.value, Some(token), Self::timeout(&settings))?;
        Ok(Self { settings, paths, api, claims })
    }

    /// The team a system-client token is bound to.
    pub fn token_team(&self) -> Option<&str> {
        self.claims
            .as_ref()
            .filter(|claims| claims.is_system_client())
            .and_then(|claims| claims.team_id.as_deref())
    }

    pub async fn teams(&self) -> Result<Vec<Team>, CliError> {
        let value = self.api.get(&["teams"], &[]).await?;
        serde_json::from_value(value)
            .map_err(|err| CliError::Other(format!("unexpected teams response: {err}")))
    }

    /// Team id from the settings (name or id) or from a system-client token.
    pub async fn team_id(&self) -> Result<String, CliError> {
        let token_team = self.token_team().map(str::to_string);
        let Some(wanted) = self.settings.team.as_ref().map(|team| team.value.clone()) else {
            return token_team.ok_or_else(|| {
                CliError::Usage(
                    "team required: pass --team, set FLUXGATE_TEAM, or run fluxgate teams use <name>".into(),
                )
            });
        };
        let id = if is_uuid(&wanted) {
            wanted.trim().to_string()
        } else {
            find_team(&self.teams().await?, &wanted)?.id
        };
        // The server answers for the token's team whatever the request says,
        // so a different team would silently act on the wrong one.
        if let Some(token_team) = token_team
            && !token_team.eq_ignore_ascii_case(&id)
        {
            return Err(CliError::Usage(format!(
                "team '{wanted}' does not match the system-client token's team {token_team}"
            )));
        }
        Ok(id)
    }

    pub async fn environments(&self, team_id: &str) -> Result<Vec<Environment>, CliError> {
        let items = self.api.get_all_pages(&["teams", team_id, "environments"], &[]).await?;
        serde_json::from_value(Value::Array(items))
            .map_err(|err| CliError::Other(format!("unexpected environments response: {err}")))
    }

    pub async fn environment_id(&self, team_id: &str) -> Result<String, CliError> {
        let wanted = self.wanted_environment()?;
        if is_uuid(&wanted) {
            return Ok(wanted);
        }
        let environments = self.environments(team_id).await?;
        environments
            .iter()
            .find(|environment| environment.name.eq_ignore_ascii_case(&wanted))
            .map(|environment| environment.id.clone())
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "environment '{wanted}' not found; available: {}",
                    environments.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ")
                ))
            })
    }

    /// Environment name, for routes that address environments by name.
    pub async fn environment_name(&self, team_id: &str) -> Result<String, CliError> {
        let wanted = self.wanted_environment()?;
        if !is_uuid(&wanted) {
            return Ok(wanted);
        }
        self.environments(team_id)
            .await?
            .into_iter()
            .find(|environment| environment.id.eq_ignore_ascii_case(&wanted))
            .map(|environment| environment.name)
            .ok_or_else(|| CliError::Usage(format!("environment {wanted} not found in team {team_id}")))
    }

    fn wanted_environment(&self) -> Result<String, CliError> {
        self.settings
            .environment
            .as_ref()
            .map(|environment| environment.value.trim().to_string())
            .ok_or_else(|| CliError::Usage("environment required: pass --env or set FLUXGATE_ENVIRONMENT".into()))
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli -- prompt:: context::`
Expected: PASS (10 tests).

- [ ] **Step 6: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): command context with team and environment resolution"
```

---

### Task 8: `run`, the binary, and the read commands (health, flags, approvals)

**Files:**
- Create: `fluxgate-cli/src/cli.rs`, `fluxgate-cli/src/main.rs`, `fluxgate-cli/src/commands/mod.rs`, `fluxgate-cli/src/commands/health.rs`, `fluxgate-cli/src/commands/flags.rs`, `fluxgate-cli/src/commands/approvals.rs`, `fluxgate-cli/tests/common/mod.rs`, `fluxgate-cli/tests/read_commands.rs`
- Modify: `fluxgate-cli/Cargo.toml` (add `[[bin]]`), `fluxgate-cli/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: `fluxgate_cli::run<I, T>(args: I, io: Io<'_>) -> i32` where `I: IntoIterator<Item = T>, T: Into<OsString> + Clone`; `Io { env: Env, is_tty: bool, prompter: &mut dyn Prompter, out: &mut dyn Write, err: &mut dyn Write }`.
- Produces: `cli::{Cli, Command, PageArgs, FlagsArgs, FlagsSubcommand, ApprovalsArgs, ApprovalsSubcommand}`, `Cli::overrides(&self) -> Overrides`.
- Produces: `commands::App { env, paths, overrides, is_tty, prompter }` with `files()`, `settings(&ConfigFiles)`, `profile()`, `async connect()`; `commands::dispatch(Command, &mut App) -> Result<Outcome, CliError>`; `commands::list_paged(&ApiClient, &[&str], &[(&str, String)], &PageArgs) -> Result<Value, CliError>`.
- Produces (tests): `tests/common`: `Harness { dir, server }`, `Harness::new()`, `url()`, `write(name, contents)`, `read(name)`, `exists(name)`, `write_session(name, access, refresh, expires_in_secs)`, `run(args, env)`, `run_with(args, env, answers)`; `RunResult { code, stdout, stderr }`; `json_out(&RunResult) -> Value`; `user_token(username)`, `system_token(team)`, `login_body(access, refresh)`; constants `TEAM_A`, `TEAM_B`, `ENV_STAGING`, `FEATURE_ID`.

- [ ] **Step 1: Add the binary target**

Append to `fluxgate-cli/Cargo.toml` after `[lib]`:

```toml
[[bin]]
name = "fluxgate"
path = "src/main.rs"
```

- [ ] **Step 2: Write the CLI definitions with parse tests**

`src/cli.rs`:

```rust
//! Command line definitions.

use clap::{Args, Parser, Subcommand};

use crate::config::Overrides;
use crate::output::OutputFormat;

#[derive(Debug, Parser)]
#[command(name = "fluxgate", version, about = "FluxGate CLI for flag operations and CI automation")]
pub struct Cli {
    /// Profile from ~/.fluxgate/config (env FLUXGATE_PROFILE).
    #[arg(long, global = true)]
    pub profile: Option<String>,
    /// Backend API URL such as https://fluxgate.example.com/api/v1 (env FLUXGATE_URL).
    #[arg(long, alias = "base-url", global = true)]
    pub url: Option<String>,
    /// Team name or id (env FLUXGATE_TEAM).
    #[arg(long, alias = "team-id", global = true)]
    pub team: Option<String>,
    /// Environment name or id (env FLUXGATE_ENVIRONMENT).
    #[arg(long = "env", alias = "environment-id", global = true)]
    pub environment: Option<String>,
    /// Output format (env FLUXGATE_OUTPUT); table on a terminal, json otherwise.
    #[arg(long, value_enum, global = true)]
    pub output: Option<OutputFormat>,
    /// Same as --output json.
    #[arg(long, global = true, conflicts_with = "output")]
    pub json: bool,
    /// Bearer token; wins over profiles and sessions (env FLUXGATE_TOKEN).
    #[arg(long, global = true)]
    pub token: Option<String>,
    /// Request timeout in seconds (env FLUXGATE_TIMEOUT, default 30).
    #[arg(long, global = true)]
    pub timeout: Option<u64>,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn overrides(&self) -> Overrides {
        Overrides {
            profile: self.profile.clone(),
            url: self.url.clone(),
            team: self.team.clone(),
            environment: self.environment.clone(),
            output: if self.json { Some(OutputFormat::Json) } else { self.output },
            token: self.token.clone(),
            timeout: self.timeout,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that the backend is up.
    Health,
    /// Read flags.
    Flags(FlagsArgs),
    /// Approval requests.
    Approvals(ApprovalsArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct PageArgs {
    /// Page size (server maximum 200).
    #[arg(long)]
    pub limit: Option<i64>,
    /// Number of items to skip.
    #[arg(long)]
    pub offset: Option<i64>,
    /// Fetch every page.
    #[arg(long, conflicts_with_all = ["limit", "offset"])]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct FlagsArgs {
    #[command(subcommand)]
    pub command: FlagsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum FlagsSubcommand {
    /// List the team's flags.
    List {
        #[command(flatten)]
        page: PageArgs,
    },
    /// Show one flag by id or key.
    Get {
        /// Feature id (UUID) or key.
        id_or_key: String,
    },
}

#[derive(Debug, Args)]
pub struct ApprovalsArgs {
    #[command(subcommand)]
    pub command: ApprovalsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ApprovalsSubcommand {
    /// List the team's approval requests.
    List {
        /// Comma-separated statuses: pending, approved, rejected, cancelled, auto_approved.
        #[arg(long, default_value = "pending")]
        status: String,
        #[command(flatten)]
        page: PageArgs,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_global_flags_still_parse() {
        let cli = Cli::parse_from(["fluxgate", "--base-url", "http://h/api/v1", "--team-id", "team-a", "health"]);
        assert_eq!(cli.url.as_deref(), Some("http://h/api/v1"));
        assert_eq!(cli.team.as_deref(), Some("team-a"));
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::parse_from(["fluxgate", "flags", "list", "--team-id", "team-b", "--limit", "10"]);
        assert_eq!(cli.team.as_deref(), Some("team-b"));
        match cli.command {
            Command::Flags(FlagsArgs { command: FlagsSubcommand::List { page } }) => assert_eq!(page.limit, Some(10)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn json_flag_means_json_output_and_conflicts_with_output() {
        let cli = Cli::parse_from(["fluxgate", "--json", "health"]);
        assert_eq!(cli.overrides().output, Some(OutputFormat::Json));
        assert!(Cli::try_parse_from(["fluxgate", "--json", "--output", "table", "health"]).is_err());
    }

    #[test]
    fn all_conflicts_with_limit() {
        assert!(Cli::try_parse_from(["fluxgate", "flags", "list", "--all", "--limit", "5"]).is_err());
    }
}
```

- [ ] **Step 3: Write the integration test harness**

`tests/common/mod.rs`:

```rust
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
```

- [ ] **Step 4: Write the failing integration tests**

`tests/read_commands.rs`:

```rust
mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn health_needs_no_credentials() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "ok" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h.run(&["health"], &[("FLUXGATE_URL", url.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(json_out(&r)["status"], "ok");
}

#[tokio::test]
async fn flags_list_all_follows_every_page() {
    let h = Harness::new().await;
    let features_path = format!("/api/v1/teams/{TEAM_A}/features");
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(0..200), "meta": { "offset": 0, "limit": 200, "total": 250 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(200..250), "meta": { "offset": 200, "limit": 200, "total": 250 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["flags", "list", "--all"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["items"].as_array().unwrap().len(), 250);
    assert_eq!(value["meta"]["total"], 250);
}

#[tokio::test]
async fn flags_list_sends_limit_offset_and_accepts_legacy_team_flag() {
    let h = Harness::new().await;
    let token = user_token("alice");
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(query_param("limit", "10"))
        .and(query_param("offset", "20"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(0..1), "meta": { "offset": 20, "limit": 10, "total": 21 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h
        .run(
            &["flags", "list", "--team-id", TEAM_A, "--limit", "10", "--offset", "20", "--output", "table"],
            &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("flag-0"));
    assert!(r.stdout.contains("1 of 21 shown"));
}

#[tokio::test]
async fn flags_get_by_key_uses_the_encoded_by_key_route() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features/by-key/checkout%20v2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": FEATURE_ID, "key": "checkout v2" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["flags", "get", "checkout v2"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(json_out(&r)["key"], "checkout v2");
}

#[tokio::test]
async fn flags_get_by_id_needs_no_team() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/features/{FEATURE_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": FEATURE_ID, "key": "checkout" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["flags", "get", FEATURE_ID], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn team_name_from_a_profile_is_resolved() {
    let h = Harness::new().await;
    h.write("config", "[default]\nteam = payments\n");
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": TEAM_A, "name": "Payments" }])))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [], "meta": { "offset": 0, "limit": 50, "total": 0 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["flags", "list"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn approvals_list_sends_statuses_and_renders_a_table() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/approval-requests")))
        .and(query_param("statuses", "pending,approved"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": "ap1", "featureId": FEATURE_ID, "changeType": "STAGE_CHANGE", "status": "PENDING",
                        "requestedBy": "alice", "createdAt": "2026-10-04T10:00:00Z" }],
            "meta": { "offset": 0, "limit": 50, "total": 1 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["approvals", "list", "--status", "pending,approved", "--output", "table"],
            &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("STATUS") && r.stdout.contains("PENDING"));
}

#[tokio::test]
async fn forbidden_exits_4_with_the_server_message() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "forbidden", "message": "policy denied", "code": "policy_denied", "details": null })))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let env = [("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)];
    let r = h.run(&["flags", "list", "--output", "text"], &env).await;
    assert_eq!(r.code, 4);
    assert!(r.stderr.contains("error: policy denied (code policy_denied, HTTP 403)"), "{}", r.stderr);
    let r = h.run(&["flags", "list"], &env).await;
    let body: serde_json::Value = serde_json::from_str(&r.stderr).unwrap();
    assert_eq!(body["code"], "policy_denied");
}

#[tokio::test]
async fn missing_credentials_exit_3() {
    let h = Harness::new().await;
    let url = h.url();
    let r = h.run(&["flags", "list", "--output", "text"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TEAM", TEAM_A)]).await;
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("no credentials for profile default"));
}

#[tokio::test]
async fn unknown_subcommand_is_a_usage_error() {
    let h = Harness::new().await;
    let r = h.run(&["flags", "nope"], &[]).await;
    assert_eq!(r.code, 2);
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli --test read_commands`
Expected: FAIL to compile (`cannot find function run in crate fluxgate_cli`).

- [ ] **Step 6: Implement `run` and `Io` in `src/lib.rs`**

Replace `src/lib.rs` with:

```rust
//! FluxGate command line interface: profiles, login sessions and API commands.

pub mod api;
pub mod auth;
pub mod cli;
pub mod commands;
pub mod config;
pub mod context;
pub mod error;
pub mod output;
pub mod prompt;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;

use crate::cli::Cli;
use crate::commands::{App, dispatch};
use crate::config::{ConfigFiles, Env, Overrides, Paths, resolve};
use crate::error::{EXIT_OK, EXIT_USAGE};
use crate::output::{OutputFormat, render};
use crate::prompt::Prompter;

/// Everything `run` takes from the outside world, so tests can replace it.
pub struct Io<'a> {
    pub env: Env,
    /// Whether stdout is a terminal; picks the default output format.
    pub is_tty: bool,
    pub prompter: &'a mut dyn Prompter,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Parses `args`, runs the command and returns the process exit code.
pub async fn run<I, T>(args: I, io: Io<'_>) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let Io { env, is_tty, prompter, out, err } = io;
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(parse_error) => {
            // --help and --version are "errors" that belong on stdout.
            return if parse_error.use_stderr() {
                let _ = write!(err, "{parse_error}");
                EXIT_USAGE
            } else {
                let _ = write!(out, "{parse_error}");
                EXIT_OK
            };
        }
    };
    let overrides = cli.overrides();
    let paths = match Paths::from_env(&env) {
        Ok(paths) => paths,
        Err(error) => {
            let _ = writeln!(err, "error: {error}");
            return error.exit_code();
        }
    };
    let files = ConfigFiles::load(&paths);
    if let Ok(files) = &files {
        for warning in &files.warnings {
            let _ = writeln!(err, "{warning}");
        }
    }
    let format = files
        .as_ref()
        .ok()
        .and_then(|files| resolve(files, &env, &overrides, is_tty).ok())
        .map(|settings| settings.output.value)
        .unwrap_or_else(|| fallback_format(&overrides, &env, is_tty));

    let mut app = App { env, paths, overrides, is_tty, prompter };
    match dispatch(cli.command, &mut app).await {
        Ok(outcome) => {
            for warning in &outcome.warnings {
                let _ = writeln!(err, "{warning}");
            }
            let text = render(&outcome.value, outcome.kind, format);
            if !text.is_empty() {
                let _ = writeln!(out, "{text}");
            }
            outcome.exit_code
        }
        Err(error) => {
            if format == OutputFormat::Json {
                let body = serde_json::to_string_pretty(&error.to_json()).unwrap_or_default();
                let _ = writeln!(err, "{body}");
            } else {
                let _ = writeln!(err, "error: {error}");
            }
            error.exit_code()
        }
    }
}

/// Output format when the profile cannot be resolved (the error is reported
/// by the command itself).
fn fallback_format(overrides: &Overrides, env: &Env, is_tty: bool) -> OutputFormat {
    overrides
        .output
        .or_else(|| env.get("FLUXGATE_OUTPUT").and_then(|value| value.parse().ok()))
        .unwrap_or(if is_tty { OutputFormat::Table } else { OutputFormat::Json })
}
```

- [ ] **Step 7: Implement `src/main.rs`**

```rust
use std::io::{IsTerminal, Write};

use fluxgate_cli::config::Env;
use fluxgate_cli::prompt::TerminalPrompter;
use fluxgate_cli::{Io, run};

#[tokio::main]
async fn main() {
    let mut prompter = TerminalPrompter;
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let is_tty = out.is_terminal();
    let code = run(
        std::env::args_os(),
        Io { env: Env::from_process(), is_tty, prompter: &mut prompter, out: &mut out, err: &mut err },
    )
    .await;
    let _ = out.flush();
    std::process::exit(code);
}
```

- [ ] **Step 8: Implement the commands**

`src/commands/mod.rs`:

```rust
//! Command implementations. Each returns an [`Outcome`] for `run` to render.

pub mod approvals;
pub mod flags;
pub mod health;

use serde_json::{Value, json};

use crate::api::ApiClient;
use crate::cli::{Command, PageArgs};
use crate::config::{ConfigFiles, Env, Overrides, Paths, Settings, resolve, selected_profile};
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;
use crate::prompt::Prompter;

pub struct App<'a> {
    pub env: Env,
    pub paths: Paths,
    pub overrides: Overrides,
    pub is_tty: bool,
    pub prompter: &'a mut dyn Prompter,
}

impl App<'_> {
    pub fn files(&self) -> Result<ConfigFiles, CliError> {
        ConfigFiles::load(&self.paths)
    }

    pub fn settings(&self, files: &ConfigFiles) -> Result<Settings, CliError> {
        resolve(files, &self.env, &self.overrides, self.is_tty)
    }

    /// The selected profile name, whether or not it exists yet.
    pub fn profile(&self) -> String {
        selected_profile(&self.overrides, &self.env)
    }

    pub async fn connect(&self) -> Result<Context, CliError> {
        let files = self.files()?;
        Context::connect(self.settings(&files)?, self.paths.clone()).await
    }
}

pub async fn dispatch(command: Command, app: &mut App<'_>) -> Result<Outcome, CliError> {
    match command {
        Command::Health => health::run(app).await,
        Command::Flags(args) => flags::run(args, app).await,
        Command::Approvals(args) => approvals::run(args, app).await,
    }
}

/// One page as asked with `--limit`/`--offset`, or every page with `--all`.
pub async fn list_paged(
    api: &ApiClient,
    segments: &[&str],
    query: &[(&str, String)],
    page: &PageArgs,
) -> Result<Value, CliError> {
    if page.all {
        let items = api.get_all_pages(segments, query).await?;
        let total = items.len();
        return Ok(json!({ "items": items, "meta": { "offset": 0, "limit": total, "total": total } }));
    }
    let mut page_query = query.to_vec();
    if let Some(limit) = page.limit {
        page_query.push(("limit", limit.to_string()));
    }
    if let Some(offset) = page.offset {
        page_query.push(("offset", offset.to_string()));
    }
    api.get(segments, &page_query).await
}
```

`src/commands/health.rs`:

```rust
use super::App;
use crate::context::Context;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let context = Context::anonymous(app.settings(&files)?, app.paths.clone())?;
    Ok(Outcome::new(context.api.get(&["health"], &[]).await?, Kind::Object))
}
```

`src/commands/flags.rs`:

```rust
use super::{App, list_paged};
use crate::cli::{FlagsArgs, FlagsSubcommand};
use crate::context::is_uuid;
use crate::error::CliError;
use crate::output::{FEATURE_COLUMNS, Kind, Outcome};

pub async fn run(args: FlagsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    match args.command {
        FlagsSubcommand::List { page } => {
            let team = context.team_id().await?;
            let value = list_paged(&context.api, &["teams", &team, "features"], &[], &page).await?;
            Ok(Outcome::new(value, Kind::List(FEATURE_COLUMNS)))
        }
        FlagsSubcommand::Get { id_or_key } => {
            let value = if is_uuid(&id_or_key) {
                context.api.get(&["features", id_or_key.trim()], &[]).await?
            } else {
                let team = context.team_id().await?;
                context.api.get(&["teams", &team, "features", "by-key", &id_or_key], &[]).await?
            };
            Ok(Outcome::new(value, Kind::Object))
        }
    }
}
```

`src/commands/approvals.rs`:

```rust
use super::{App, list_paged};
use crate::cli::{ApprovalsArgs, ApprovalsSubcommand};
use crate::error::CliError;
use crate::output::{APPROVAL_COLUMNS, Kind, Outcome};

pub async fn run(args: ApprovalsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    match args.command {
        ApprovalsSubcommand::List { status, page } => {
            let team = context.team_id().await?;
            // The API parameter is `statuses` and takes a comma-separated list.
            let query = [("statuses", status.trim().to_string())];
            let value = list_paged(&context.api, &["teams", &team, "approval-requests"], &query, &page).await?;
            Ok(Outcome::new(value, Kind::List(APPROVAL_COLUMNS)))
        }
    }
}
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli`
Expected: PASS (unit tests plus 10 tests in `read_commands`).

- [ ] **Step 10: Commit**

```bash
git add fluxgate-cli Cargo.lock
git commit -m "feat(cli): run entry point and read commands with pagination"
```

---

### Task 9: evaluate, config export, rollout promote; remove the old binary

**Files:**
- Create: `fluxgate-cli/src/commands/evaluate.rs`, `fluxgate-cli/src/commands/config_export.rs`, `fluxgate-cli/src/commands/rollout.rs`, `fluxgate-cli/tests/write_commands.rs`
- Modify: `fluxgate-cli/src/cli.rs`, `fluxgate-cli/src/commands/mod.rs`, `feature-toggle-backend/Cargo.toml`
- Delete: `feature-toggle-backend/src/bin/fluxgate.rs`

**Interfaces:**
- Consumes: `App`, `Context::{team_id, environment_id, environment_name, teams}`, `ApiClient::get_all_pages`, `cell`, `EXIT_FLAG_OFF`.
- Produces: `Command::{Evaluate(EvaluateArgs), Config(ConfigArgs), Rollout(RolloutArgs)}`; `EvaluateArgs { flag: String, targeting_key: String, context: String, exit_code: bool }`; `ConfigSubcommand::Export`; `RolloutSubcommand::Promote { stage_id: Option<String>, flag: Option<String>, request: String, reason, external_ref, freeze_override_reason: Option<String> }`; `STAGE_REQUESTS: [&str; 6]`.

- [ ] **Step 1: Write the failing parse tests**

In `src/cli.rs`, add to the `tests` module:

```rust
    #[test]
    fn parses_legacy_evaluate_command_for_ci() {
        let cli = Cli::parse_from([
            "fluxgate", "--base-url", "http://localhost:8080/api/v1", "--token", "secret", "evaluate",
            "--feature-key", "checkout", "--environment-id", "env", "--targeting-key", "user-1",
            "--context", "{\"plan\":\"pro\"}",
        ]);
        assert_eq!(cli.environment.as_deref(), Some("env"));
        match cli.command {
            Command::Evaluate(args) => {
                assert_eq!(args.flag, "checkout");
                assert_eq!(args.targeting_key, "user-1");
                assert!(!args.exit_code);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_legacy_rollout_promote_command() {
        let cli = Cli::parse_from(["fluxgate", "rollout", "promote", "stage-123", "--request", "DEPLOYED"]);
        match cli.command {
            Command::Rollout(RolloutArgs { command: RolloutSubcommand::Promote { stage_id, request, flag, .. } }) => {
                assert_eq!(stage_id.as_deref(), Some("stage-123"));
                assert_eq!(request, "DEPLOYED");
                assert_eq!(flag, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rollout_request_is_checked() {
        assert!(Cli::try_parse_from(["fluxgate", "rollout", "promote", "s", "--request", "deployed"]).is_ok());
        assert!(Cli::try_parse_from(["fluxgate", "rollout", "promote", "s", "--request", "LAUNCH"]).is_err());
    }
```

- [ ] **Step 2: Write the failing integration tests**

`tests/write_commands.rs`:

```rust
mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

async fn mount_environments(h: &Harness) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "teamId": TEAM_A, "active": true, "environmentType": "STAGING" }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
}

async fn mount_evaluate(h: &Harness, value: bool) {
    Mock::given(method("POST"))
        .and(path("/api/v1/evaluate"))
        .and(body_json(json!({ "teamId": TEAM_A, "featureKey": "checkout", "environmentId": ENV_STAGING,
                               "targetingKey": "user-1", "context": { "plan": "pro" } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "flagKey": "checkout", "value": value, "variant": null, "reason": "TARGETING_MATCH", "errorCode": null })))
        .expect(1)
        .mount(&h.server)
        .await;
}

const EVALUATE: [&str; 9] = [
    "evaluate", "--flag", "checkout", "--targeting-key", "user-1", "--context", "{\"plan\":\"pro\"}", "--env", ENV_STAGING,
];

#[tokio::test]
async fn evaluate_exit_code_is_10_when_the_flag_is_off() {
    let h = Harness::new().await;
    mount_evaluate(&h, false).await;
    let (url, token) = (h.url(), user_token("alice"));
    let mut args = EVALUATE.to_vec();
    args.push("--exit-code");
    let r = h.run(&args, &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)]).await;
    assert_eq!(r.code, 10, "{}", r.stderr);
    assert_eq!(json_out(&r)["value"], false);
}

#[tokio::test]
async fn evaluate_exit_code_is_0_when_the_flag_is_on() {
    let h = Harness::new().await;
    mount_evaluate(&h, true).await;
    let (url, token) = (h.url(), user_token("alice"));
    let mut args = EVALUATE.to_vec();
    args.push("--exit-code");
    let r = h.run(&args, &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn evaluate_resolves_an_environment_name_with_legacy_flags() {
    let h = Harness::new().await;
    mount_environments(&h).await;
    mount_evaluate(&h, true).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["evaluate", "--feature-key", "checkout", "--environment-id", "staging", "--targeting-key", "user-1", "--context", "{\"plan\":\"pro\"}"],
            &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM_ID", TEAM_A)],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn evaluate_uses_the_system_token_team_and_refuses_another() {
    let h = Harness::new().await;
    mount_evaluate(&h, true).await;
    let (url, token) = (h.url(), system_token(TEAM_A));
    let r = h.run(&EVALUATE, &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = h
        .run(&EVALUATE, &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_B)])
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("does not match"), "{}", r.stderr);
}

#[tokio::test]
async fn evaluate_rejects_a_context_that_is_not_an_object() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["evaluate", "--flag", "checkout", "--targeting-key", "u", "--context", "[1]", "--env", ENV_STAGING, "--output", "text"],
            &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--context must be a JSON object"));
}

#[tokio::test]
async fn rollout_promote_by_flag_maps_an_environment_id_to_its_name() {
    let h = Harness::new().await;
    mount_environments(&h).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features/by-key/checkout/environments/staging/request-change")))
        .and(body_json(json!({ "request": "DEPLOYED", "reason": "ship it", "externalRef": "PROJ-1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": FEATURE_ID, "key": "checkout" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["rollout", "promote", "--flag", "checkout", "--env", ENV_STAGING, "--request", "deployed",
              "--reason", "ship it", "--external-ref", "PROJ-1"],
            &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(json_out(&r)["key"], "checkout");
}

#[tokio::test]
async fn rollout_promote_by_stage_keeps_the_old_form() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/stages/stage-123/request-change"))
        .and(body_json(json!({ "request": "DEPLOYMENT_REQUESTED", "freezeOverrideReason": "hotfix" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": FEATURE_ID })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["rollout", "promote", "stage-123", "--freeze-override-reason", "hotfix"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn rollout_promote_needs_a_stage_or_a_flag() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["rollout", "promote"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())]).await;
    assert_eq!(r.code, 2);
}

#[tokio::test]
async fn config_export_includes_every_page() {
    let h = Harness::new().await;
    mount_environments(&h).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": TEAM_A, "name": "Payments" }])))
        .mount(&h.server)
        .await;
    let features_path = format!("/api/v1/teams/{TEAM_A}/features");
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(0..200), "meta": { "offset": 0, "limit": 200, "total": 201 } })))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(200..201), "meta": { "offset": 200, "limit": 200, "total": 201 } })))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["config", "export", "--output", "table"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", TEAM_A)])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["team"]["name"], "Payments");
    assert_eq!(value["environments"].as_array().unwrap().len(), 1);
    assert_eq!(value["features"].as_array().unwrap().len(), 201);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli --test write_commands; cargo test -p fluxgate-cli cli::`
Expected: FAIL to compile (`no variant Evaluate`).

- [ ] **Step 4: Add the CLI definitions**

In `src/cli.rs`, add to `enum Command`:

```rust
    /// Evaluate a flag for a targeting key.
    Evaluate(EvaluateArgs),
    /// Team configuration.
    Config(ConfigArgs),
    /// Stage changes.
    Rollout(RolloutArgs),
```

and add these items below `ApprovalsSubcommand`:

```rust
#[derive(Debug, Args)]
pub struct EvaluateArgs {
    /// Flag key.
    #[arg(long = "flag", alias = "feature-key")]
    pub flag: String,
    /// User or entity the flag is evaluated for.
    #[arg(long)]
    pub targeting_key: String,
    /// Evaluation context as a JSON object.
    #[arg(long, default_value = "{}")]
    pub context: String,
    /// Exit 0 when the flag is true and 10 when it is false.
    #[arg(long)]
    pub exit_code: bool,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigSubcommand {
    /// Print the team, its environments and all flags as JSON.
    Export,
}

pub const STAGE_REQUESTS: [&str; 6] = [
    "DEPLOYMENT_REQUESTED",
    "DEPLOYMENT_REJECTED",
    "DEPLOYED",
    "ROLLBACK_REQUESTED",
    "ROLLBACK_REJECTED",
    "ROLLBACKED",
];

#[derive(Debug, Args)]
pub struct RolloutArgs {
    #[command(subcommand)]
    pub command: RolloutSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum RolloutSubcommand {
    /// Request a stage change, by stage id or by flag key and environment.
    Promote {
        /// Stage id; or use --flag with --env.
        stage_id: Option<String>,
        /// Flag key; the stage is found from --env.
        #[arg(long, conflicts_with = "stage_id")]
        flag: Option<String>,
        #[arg(
            long,
            default_value = "DEPLOYMENT_REQUESTED",
            value_parser = clap::builder::PossibleValuesParser::new(STAGE_REQUESTS),
            ignore_case = true
        )]
        request: String,
        /// Why the change is requested.
        #[arg(long)]
        reason: Option<String>,
        /// Ticket or change id, for example a Jira issue key.
        #[arg(long)]
        external_ref: Option<String>,
        /// Reason for changing during a freeze window.
        #[arg(long)]
        freeze_override_reason: Option<String>,
    },
}
```

- [ ] **Step 5: Implement the commands**

In `src/commands/mod.rs` add `pub mod config_export; pub mod evaluate; pub mod rollout;` (keep the list sorted) and add to `dispatch`:

```rust
        Command::Evaluate(args) => evaluate::run(args, app).await,
        Command::Config(_) => config_export::run(app).await,
        Command::Rollout(args) => rollout::run(args, app).await,
```

`src/commands/evaluate.rs`:

```rust
use serde_json::{Value, json};

use super::App;
use crate::cli::EvaluateArgs;
use crate::error::{CliError, EXIT_FLAG_OFF, EXIT_OK};
use crate::output::{Kind, Outcome, cell};

pub async fn run(args: EvaluateArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = match serde_json::from_str::<Value>(&args.context) {
        Ok(Value::Object(context)) => context,
        Ok(_) => return Err(CliError::Usage("--context must be a JSON object".into())),
        Err(err) => return Err(CliError::Usage(format!("--context must be JSON: {err}"))),
    };
    let api_context = app.connect().await?;
    let team = api_context.team_id().await?;
    let environment = api_context.environment_id(&team).await?;
    let value = api_context
        .api
        .post(
            &["evaluate"],
            &json!({
                "teamId": team,
                "featureKey": args.flag,
                "environmentId": environment,
                "targetingKey": args.targeting_key,
                "context": context,
            }),
        )
        .await?;
    let mut outcome = Outcome::new(value, Kind::Object);
    if args.exit_code {
        outcome.exit_code = match outcome.value.get("value") {
            Some(Value::Bool(true)) => EXIT_OK,
            Some(Value::Bool(false)) => EXIT_FLAG_OFF,
            other => {
                return Err(CliError::Usage(format!(
                    "--exit-code needs a boolean flag; '{}' returned {}",
                    args.flag,
                    cell(other)
                )));
            }
        };
    }
    Ok(outcome)
}
```

`src/commands/config_export.rs`:

```rust
use serde_json::json;

use super::App;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    let team_id = context.team_id().await?;
    // System-client tokens may not read /teams; the id alone is still useful.
    let team = context
        .teams()
        .await
        .ok()
        .and_then(|teams| teams.into_iter().find(|team| team.id.eq_ignore_ascii_case(&team_id)))
        .map(|team| json!(team))
        .unwrap_or_else(|| json!({ "id": team_id }));
    let environments = context.api.get_all_pages(&["teams", &team_id, "environments"], &[]).await?;
    let features = context.api.get_all_pages(&["teams", &team_id, "features"], &[]).await?;
    Ok(Outcome::new(
        json!({ "team": team, "environments": environments, "features": features }),
        Kind::Document,
    ))
}
```

`src/commands/rollout.rs`:

```rust
use serde_json::json;

use super::App;
use crate::cli::{RolloutArgs, RolloutSubcommand};
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(args: RolloutArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let RolloutSubcommand::Promote { stage_id, flag, request, reason, external_ref, freeze_override_reason } =
        args.command;
    if stage_id.is_none() && flag.is_none() {
        return Err(CliError::Usage("pass a stage id, or --flag <key> with --env <name>".into()));
    }
    let mut body = json!({ "request": request.to_ascii_uppercase() });
    if let Some(reason) = reason {
        body["reason"] = json!(reason);
    }
    if let Some(external_ref) = external_ref {
        body["externalRef"] = json!(external_ref);
    }
    if let Some(freeze_override_reason) = freeze_override_reason {
        body["freezeOverrideReason"] = json!(freeze_override_reason);
    }

    let context = app.connect().await?;
    let value = match (stage_id, flag) {
        (_, Some(flag)) => {
            let team = context.team_id().await?;
            // The by-key route addresses the environment by name.
            let environment = context.environment_name(&team).await?;
            context
                .api
                .post(
                    &["teams", &team, "features", "by-key", &flag, "environments", &environment, "request-change"],
                    &body,
                )
                .await?
        }
        (Some(stage_id), None) => context.api.post(&["stages", &stage_id, "request-change"], &body).await?,
        (None, None) => unreachable!("checked above"),
    };
    Ok(Outcome::new(value, Kind::Object))
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli`
Expected: PASS (including 9 tests in `write_commands` and 7 in `cli::tests`).

- [ ] **Step 7: Remove the old backend binary**

```bash
git rm feature-toggle-backend/src/bin/fluxgate.rs
grep -rn "clap" feature-toggle-backend/src feature-toggle-backend/tests
```

Expected: no output from grep. Then delete this line from `feature-toggle-backend/Cargo.toml`:

```toml
clap = { version = "4.5.40", features = ["derive", "env"] }
```

Run: `SQLX_OFFLINE=true cargo build -p feature-toggle-backend --all-targets`
Expected: builds; no `fluxgate` binary target in the backend.

- [ ] **Step 8: Commit**

```bash
git add fluxgate-cli feature-toggle-backend/Cargo.toml Cargo.lock
git commit -m "feat(cli): evaluate exit codes, key-based promote, full config export

Moves the CLI out of the backend crate; the backend no longer depends on clap."
```

---

### Task 10: Password login and logout

**Files:**
- Create: `fluxgate-cli/src/auth/password.rs`, `fluxgate-cli/src/commands/login.rs`, `fluxgate-cli/src/commands/logout.rs`, `fluxgate-cli/tests/login.rs`
- Modify: `fluxgate-cli/src/auth/mod.rs`, `fluxgate-cli/src/cli.rs`, `fluxgate-cli/src/commands/mod.rs`

**Interfaces:**
- Consumes: `ApiClient`, `SessionStore`, `SessionCache`, `LoginResponse`, `Prompter`, `App`, `ConfigFiles::{set_profile_value, set_session_value, save_config, session_value}`, `Context::timeout`.
- Produces: `auth::password::password_login(prompter: &mut dyn Prompter, base_url: &str, username: &str, timeout: Duration) -> Result<LoginResponse, CliError>`; `Command::{Login(LoginArgs), Logout(LogoutArgs)}`; `LoginArgs { password: bool, username: Option<String> }`; `LogoutArgs { all: bool }`.

- [ ] **Step 1: Write the failing tests**

`tests/login.rs`:

```rust
mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

async fn mount_login(h: &Harness, password: &str, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(json!({ "username": "alice", "password": password })))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn password_login_writes_the_session_and_links_the_profile() {
    let h = Harness::new().await;
    h.write("config", "[default]\nteam = payments\n");
    mount_login(&h, "pw", login_body("a1", "r1")).await;
    let url = h.url();
    let r = h.run_with(&["login", "--password", "--username", "alice", "--output", "text"], &[("FLUXGATE_URL", url.as_str())], &["pw"]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged in as alice (session 'default')");
    let session: serde_json::Value = serde_json::from_str(&h.read("sessions/default.json")).unwrap();
    assert_eq!(session["accessToken"], "a1");
    let config = h.read("config");
    assert!(config.contains("session=default"), "{config}");
    assert!(config.contains("[session default]"), "{config}");
    assert!(config.contains(&format!("url={url}")), "{config}");
}

#[tokio::test]
async fn later_commands_use_and_refresh_the_session() {
    let h = Harness::new().await;
    h.write("config", &format!("[default]\nsession = corp\nteam = {TEAM_A}\n\n[session corp]\nurl = {}\n", h.url()));
    h.write_session("corp", "a1", "r1", -5);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({ "refreshToken": "r1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a2", "r2")))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(header("authorization", "Bearer a2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [], "meta": { "offset": 0, "limit": 50, "total": 0 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let r = h.run(&["flags", "list"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn temporary_password_is_changed_then_login_repeats() {
    let h = Harness::new().await;
    let mut temporary = login_body("temp", "temp-refresh");
    temporary["isTemporary"] = json!(true);
    mount_login(&h, "old", temporary).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/reset-password"))
        .and(header("authorization", "Bearer temp"))
        .and(body_json(json!({ "currentPassword": "old", "newPassword": "new-pass" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&h.server)
        .await;
    mount_login(&h, "new-pass", login_body("a1", "r1")).await;
    let url = h.url();
    let r = h
        .run_with(&["login", "--password", "--username", "alice"], &[("FLUXGATE_URL", url.as_str())], &["old", "new-pass", "new-pass"])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.read("sessions/default.json").contains("\"a1\""));
}

#[tokio::test]
async fn mismatched_new_passwords_stop_the_login() {
    let h = Harness::new().await;
    let mut temporary = login_body("temp", "temp-refresh");
    temporary["isTemporary"] = json!(true);
    mount_login(&h, "old", temporary).await;
    let url = h.url();
    let r = h
        .run_with(&["login", "--password", "--username", "alice", "--output", "text"], &[("FLUXGATE_URL", url.as_str())], &["old", "a", "b"])
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("do not match"));
    assert!(!h.exists("sessions/default.json"));
}

#[tokio::test]
async fn wrong_password_exits_3() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "unauthorized", "message": "invalid credentials" })))
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h.run_with(&["login", "--username", "alice", "--output", "text"], &[("FLUXGATE_URL", url.as_str())], &["bad"]).await;
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("login failed: invalid credentials"));
}

#[tokio::test]
async fn sso_sessions_need_the_password_flag_in_this_version() {
    let h = Harness::new().await;
    h.write("config", &format!("[default]\nsession = corp\n\n[session corp]\nurl = {}\nsso_provider = okta\n", h.url()));
    let r = h.run(&["login", "--output", "text"], &[]).await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("fluxgate login --password"));
}

#[tokio::test]
async fn logout_revokes_the_refresh_token_and_deletes_the_cache() {
    let h = Harness::new().await;
    h.write("config", &format!("[default]\nsession = corp\n\n[session corp]\nurl = {}\n", h.url()));
    h.write_session("corp", "a1", "r1", 600);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(header("authorization", "Bearer a1"))
        .and(body_json(json!({ "refreshToken": "r1" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&h.server)
        .await;
    let r = h.run(&["logout", "--output", "text"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged out of 'corp'");
    assert!(!h.exists("sessions/corp.json"));
}

#[tokio::test]
async fn logout_deletes_the_cache_even_when_the_server_fails() {
    let h = Harness::new().await;
    h.write("config", &format!("[default]\nsession = corp\n\n[session corp]\nurl = {}\n", h.url()));
    h.write_session("corp", "a1", "r1", 600);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "error": "internal", "message": "boom" })))
        .mount(&h.server)
        .await;
    let r = h.run(&["logout"], &[]).await;
    assert_eq!(r.code, 0);
    assert!(r.stderr.contains("warning: server logout failed for session 'corp'"));
    assert!(!h.exists("sessions/corp.json"));
}

#[tokio::test]
async fn logout_all_ends_every_cached_session() {
    let h = Harness::new().await;
    let url = h.url();
    h.write("config", &format!("[session one]\nurl = {url}\n\n[session two]\nurl = {url}\n"));
    h.write_session("one", "a1", "r1", 600);
    h.write_session("two", "a2", "r2", 600);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&h.server)
        .await;
    let r = h.run(&["logout", "--all", "--output", "text"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged out of 'one', 'two'");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli --test login`
Expected: FAIL (`unrecognized subcommand 'login'` assertions, exit code 2 instead of 0).

- [ ] **Step 3: Add the CLI definitions**

In `src/cli.rs`, add to `enum Command`:

```rust
    /// Log in and cache a session for the profile.
    Login(LoginArgs),
    /// End the profile's session, or every session with --all.
    Logout(LogoutArgs),
```

and:

```rust
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Log in with username and password.
    #[arg(long)]
    pub password: bool,
    /// Username; asked for when not given.
    #[arg(long)]
    pub username: Option<String>,
}

#[derive(Debug, Args)]
pub struct LogoutArgs {
    /// Log out of every cached session.
    #[arg(long)]
    pub all: bool,
}
```

- [ ] **Step 4: Implement `auth/password.rs`**

Add `pub mod password;` to `src/auth/mod.rs`, then create `src/auth/password.rs`:

```rust
//! Username and password login, including the temporary-password change.

use std::time::Duration;

use serde_json::json;

use super::session::LoginResponse;
use crate::api::ApiClient;
use crate::error::CliError;
use crate::prompt::Prompter;

pub async fn password_login(
    prompter: &mut dyn Prompter,
    base_url: &str,
    username: &str,
    timeout: Duration,
) -> Result<LoginResponse, CliError> {
    let api = ApiClient::new(base_url, None, timeout)?;
    let password = prompter.secret("Password")?;
    let response = login(&api, username, &password).await?;
    if !response.is_temporary {
        return Ok(response);
    }

    let new_password = prompter.secret("New password (the current one is temporary)")?;
    let confirm = prompter.secret("Confirm new password")?;
    if new_password != confirm {
        return Err(CliError::Usage("the new passwords do not match".into()));
    }
    let temporary = api.with_token(Some(response.token.clone()));
    temporary
        .post(
            &["auth", "reset-password"],
            &json!({ "currentPassword": password, "newPassword": new_password }),
        )
        .await?;
    // The reset answers 204 without a session: end the temporary one and log
    // in again with the new password.
    let _ = temporary
        .post(&["auth", "logout"], &json!({ "refreshToken": response.refresh_token }))
        .await;
    login(&api, username, &new_password).await
}

async fn login(api: &ApiClient, username: &str, password: &str) -> Result<LoginResponse, CliError> {
    let value = api
        .post(&["auth", "login"], &json!({ "username": username, "password": password }))
        .await
        .map_err(|err| match err {
            CliError::Api { status: 401, message, .. } => CliError::Auth(format!("login failed: {message}")),
            other => other,
        })?;
    serde_json::from_value(value)
        .map_err(|err| CliError::Other(format!("unexpected login response: {err}")))
}
```

- [ ] **Step 5: Implement the commands**

In `src/commands/mod.rs` add `pub mod login; pub mod logout;` and to `dispatch`:

```rust
        Command::Login(args) => login::run(args, app).await,
        Command::Logout(args) => logout::run(args, app).await,
```

`src/commands/login.rs`:

```rust
use chrono::Utc;

use super::App;
use crate::auth::password::password_login;
use crate::auth::session::{SessionCache, SessionStore};
use crate::cli::LoginArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;

pub async fn run(args: LoginArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let mut files = app.files()?;
    let settings = app.settings(&files)?;
    if let Some(provider) = &settings.sso_provider
        && !args.password
    {
        return Err(CliError::Usage(format!(
            "this session uses SSO provider '{provider}', which this version cannot log in to: run fluxgate login --password"
        )));
    }
    // A profile without a session gets one named after the profile.
    let session = settings.session.clone().unwrap_or_else(|| settings.profile.clone());
    let username = match args.username {
        Some(username) => username,
        None => app.prompter.input("Username", None)?,
    };
    let response =
        password_login(&mut *app.prompter, &settings.url.value, &username, Context::timeout(&settings)).await?;
    SessionStore::new(app.paths.sessions.clone()).save(&session, &SessionCache::from_login(&response, Utc::now()))?;
    if settings.session.is_none() {
        files.set_profile_value(&settings.profile, "session", &session);
        files.set_session_value(&session, "url", &settings.url.value);
        files.save_config(&app.paths)?;
    }
    Ok(Outcome::message(format!("Logged in as {} (session '{session}')", response.user.username)))
}
```

`src/commands/logout.rs`:

```rust
use std::time::Duration;

use serde_json::json;

use super::App;
use crate::api::ApiClient;
use crate::auth::session::SessionStore;
use crate::cli::LogoutArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output::Outcome;

pub async fn run(args: LogoutArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let settings = app.settings(&files)?;
    let store = SessionStore::new(app.paths.sessions.clone());
    let names = if args.all {
        store.names()?
    } else {
        vec![settings.session.clone().ok_or_else(|| {
            CliError::Usage(format!("profile '{}' has no login session", settings.profile))
        })?]
    };

    let mut warnings = Vec::new();
    let mut ended = Vec::new();
    for name in names {
        // A damaged cache cannot be revoked; just remove it.
        if !matches!(store.load(&name), Ok(Some(_))) {
            store.delete(&name)?;
            continue;
        }
        let url = files
            .session_value(&name, "url")
            .map(str::to_string)
            .unwrap_or_else(|| settings.url.value.clone());
        if let Err(err) = server_logout(&store, &name, &settings.profile, &url, Context::timeout(&settings)).await {
            warnings.push(format!("warning: server logout failed for session '{name}': {err}"));
        }
        store.delete(&name)?;
        ended.push(format!("'{name}'"));
    }

    let mut outcome = Outcome::message(if ended.is_empty() {
        "No active sessions".to_string()
    } else {
        format!("Logged out of {}", ended.join(", "))
    });
    outcome.warnings = warnings;
    Ok(outcome)
}

/// Revokes the session's refresh token family on the server.
async fn server_logout(
    store: &SessionStore,
    name: &str,
    profile: &str,
    url: &str,
    timeout: Duration,
) -> Result<(), CliError> {
    let access_token = store.access_token(name, profile, url, timeout).await?;
    let cache = store
        .load(name)?
        .ok_or_else(|| CliError::Other("session cache disappeared".into()))?;
    ApiClient::new(url, Some(access_token), timeout)?
        .post(&["auth", "logout"], &json!({ "refreshToken": cache.refresh_token }))
        .await?;
    Ok(())
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli`
Expected: PASS (9 tests in `login`).

- [ ] **Step 7: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): password login sessions and logout"
```

---

### Task 11: configure (interactive, set, get, list, list-profiles)

**Files:**
- Create: `fluxgate-cli/src/commands/configure.rs`, `fluxgate-cli/tests/configure.rs`
- Modify: `fluxgate-cli/src/cli.rs`, `fluxgate-cli/src/commands/mod.rs`

**Interfaces:**
- Consumes: `ConfigFiles` (all setters, `PROFILE_KEYS`, `profile_section`), `Settings`, `Credential`, `Resolved`, `Source`, `mask_token`, `DEFAULT_URL`, `DEFAULT_TIMEOUT_SECS`, `password_login`, `SessionStore`, `SessionCache`, `Context::{connect, teams, token_team, environments}`, `CONFIG_COLUMNS`, `PROFILE_COLUMNS`.
- Produces: `Command::Configure(ConfigureArgs)`; `ConfigureArgs { command: Option<ConfigureSubcommand> }`; `ConfigureSubcommand::{Set { key, value }, Get { key }, List, ListProfiles}`; `configure::LOGIN_PASSWORD = "Log in with username and password"`, `configure::LOGIN_TOKEN = "Static token (system client)"`.

- [ ] **Step 1: Write the failing tests**

`tests/configure.rs`:

```rust
mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn set_then_get_a_profile_value() {
    let h = Harness::new().await;
    let r = h.run(&["configure", "set", "team", "payments"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = h.run(&["configure", "get", "team", "--output", "text"], &[]).await;
    assert_eq!(r.stdout.trim(), "payments");
    let r = h.run(&["--profile", "prod", "configure", "set", "url", "https://fg.example.com/api/v1"], &[]).await;
    assert_eq!(r.code, 0);
    assert!(h.read("config").contains("[profile prod]"));
}

#[tokio::test]
async fn set_rejects_bad_keys_and_values() {
    let h = Harness::new().await;
    assert_eq!(h.run(&["configure", "set", "colour", "blue"], &[]).await.code, 2);
    assert_eq!(h.run(&["configure", "set", "output", "yaml"], &[]).await.code, 2);
    assert_eq!(h.run(&["configure", "set", "timeout", "soon"], &[]).await.code, 2);
}

#[tokio::test]
async fn get_of_an_unset_key_exits_1() {
    let h = Harness::new().await;
    let r = h.run(&["configure", "get", "environment", "--output", "text"], &[]).await;
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("environment is not set for profile 'default'"));
}

#[cfg(unix)]
#[tokio::test]
async fn set_token_goes_to_the_private_credentials_file_and_get_masks_it() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new().await;
    let r = h.run(&["--profile", "ci", "configure", "set", "token", "abcdef123456"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.read("credentials").contains("[ci]"));
    let mode = std::fs::metadata(h.path("credentials")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let r = h.run(&["--profile", "ci", "configure", "get", "token", "--output", "text"], &[]).await;
    assert_eq!(r.stdout.trim(), "****3456");
}

#[tokio::test]
async fn list_shows_values_and_their_sources() {
    let h = Harness::new().await;
    h.write("config", "[default]\nurl = https://fg.example.com/api/v1\n");
    let r = h.run(&["configure", "list"], &[("FLUXGATE_TEAM", "payments"), ("FLUXGATE_TOKEN", "abcdef123456")]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let items = json_out(&r)["items"].as_array().unwrap().clone();
    let find = |name: &str| items.iter().find(|i| i["name"] == name).unwrap().clone();
    assert_eq!(find("url")["source"], "profile");
    assert_eq!(find("team")["value"], "payments");
    assert_eq!(find("team")["source"], "env");
    assert_eq!(find("environment")["source"], "unset");
    assert_eq!(find("token")["value"], "****3456 (static)");
    assert!(!r.stdout.contains("abcdef123456"));
}

#[tokio::test]
async fn list_profiles_marks_the_active_one() {
    let h = Harness::new().await;
    h.write("config", "[default]\nteam = a\n\n[profile prod]\nteam = b\n");
    h.write("credentials", "[ci]\ntoken = t\n");
    let r = h.run(&["configure", "list-profiles"], &[("FLUXGATE_PROFILE", "prod")]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let items = json_out(&r)["items"].as_array().unwrap().clone();
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["default", "ci", "prod"]);
    assert_eq!(items[2]["active"], true);
    assert_eq!(items[0]["active"], false);
}

#[tokio::test]
async fn interactive_password_setup_logs_in_and_picks_team_and_environment() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(json!({ "username": "alice", "password": "pw" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .and(header("authorization", "Bearer a1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": TEAM_A, "name": "Payments" }, { "id": TEAM_B, "name": "Checkout" } ])))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "active": true }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h
        .run_with(
            &["configure", "--output", "text"],
            &[],
            &[url.as_str(), "Log in with username and password", "alice", "pw", "Payments", "staging", "table"],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let config = h.read("config");
    for expected in ["session=default", "team=Payments", "environment=staging", "output=table", "[session default]"] {
        assert!(config.contains(expected), "missing {expected} in {config}");
    }
    assert!(h.exists("sessions/default.json"));
}

#[tokio::test]
async fn interactive_token_setup_stores_the_token_and_the_token_team() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "active": true }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), system_token(TEAM_A));
    let r = h
        .run_with(
            &["--profile", "ci", "configure"],
            &[],
            &[url.as_str(), "Static token (system client)", token.as_str(), "staging", "json"],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.read("credentials").contains("[ci]"));
    let config = h.read("config");
    assert!(config.contains("[profile ci]") && config.contains(&format!("team={TEAM_A}")), "{config}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli --test configure`
Expected: FAIL (exit code 2, `unrecognized subcommand 'configure'`).

- [ ] **Step 3: Add the CLI definitions**

In `src/cli.rs`, add to `enum Command`:

```rust
    /// Set up a profile, or read and change its settings.
    Configure(ConfigureArgs),
```

and:

```rust
#[derive(Debug, Args)]
pub struct ConfigureArgs {
    #[command(subcommand)]
    pub command: Option<ConfigureSubcommand>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigureSubcommand {
    /// Set a profile value: session, url, team, environment, output, timeout or token.
    Set { key: String, value: String },
    /// Print a profile value from the files.
    Get { key: String },
    /// Show the resolved settings and where each comes from.
    List,
    /// List profiles.
    ListProfiles,
}
```

- [ ] **Step 4: Implement `commands/configure.rs`**

In `src/commands/mod.rs` add `pub mod configure;` and to `dispatch`:

```rust
        Command::Configure(args) => configure::run(args, app).await,
```

Create `src/commands/configure.rs`:

```rust
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};

use super::App;
use crate::api::ApiClient;
use crate::auth::password::password_login;
use crate::auth::session::{SessionCache, SessionStore};
use crate::cli::{ConfigureArgs, ConfigureSubcommand};
use crate::config::files::PROFILE_KEYS;
use crate::config::resolve::{DEFAULT_TIMEOUT_SECS, DEFAULT_URL};
use crate::config::{Credential, Resolved, Source, mask_token};
use crate::context::Context;
use crate::error::CliError;
use crate::output::{CONFIG_COLUMNS, Kind, OutputFormat, Outcome, PROFILE_COLUMNS};

pub const LOGIN_PASSWORD: &str = "Log in with username and password";
pub const LOGIN_TOKEN: &str = "Static token (system client)";

pub async fn run(args: ConfigureArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    match args.command {
        None => interactive(app).await,
        Some(ConfigureSubcommand::Set { key, value }) => set(app, &key, &value),
        Some(ConfigureSubcommand::Get { key }) => get(app, &key),
        Some(ConfigureSubcommand::List) => list(app),
        Some(ConfigureSubcommand::ListProfiles) => list_profiles(app),
    }
}

fn set(app: &App<'_>, key: &str, value: &str) -> Result<Outcome, CliError> {
    let profile = app.profile();
    let mut files = app.files()?;
    let value = value.trim();
    match key {
        "token" => {
            files.set_credential_token(&profile, value);
            files.save_credentials(&app.paths)?;
        }
        _ if PROFILE_KEYS.contains(&key) => {
            if key == "output" {
                value.parse::<OutputFormat>().map_err(CliError::Usage)?;
            }
            if key == "timeout" {
                value
                    .parse::<u64>()
                    .map_err(|_| CliError::Usage("timeout must be a number of seconds".into()))?;
            }
            files.set_profile_value(&profile, key, value);
            files.save_config(&app.paths)?;
        }
        other => {
            return Err(CliError::Usage(format!(
                "unknown key '{other}'; expected one of: {}, token",
                PROFILE_KEYS.join(", ")
            )));
        }
    }
    Ok(Outcome::message(format!("Set {key} for profile '{profile}'")))
}

fn get(app: &App<'_>, key: &str) -> Result<Outcome, CliError> {
    let profile = app.profile();
    let files = app.files()?;
    let value = if key == "token" {
        files.credential_token(&profile).map(mask_token)
    } else {
        files.profile_value(&profile, key).map(str::to_string)
    };
    value
        .map(Outcome::message)
        .ok_or_else(|| CliError::Other(format!("{key} is not set for profile '{profile}'")))
}

fn row(name: &str, value: &str, source: &str) -> Value {
    json!({ "name": name, "value": value, "source": source })
}

fn optional_row(name: &str, value: Option<&Resolved<String>>) -> Value {
    match value {
        Some(resolved) => row(name, &resolved.value, resolved.source.as_str()),
        None => row(name, "-", "unset"),
    }
}

fn list(app: &App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let settings = app.settings(&files)?;
    let profile_source = if app.overrides.profile.is_some() {
        Source::Flag
    } else if app.env.get("FLUXGATE_PROFILE").is_some() {
        Source::Env
    } else {
        Source::Default
    };
    let token = match &settings.credential {
        Credential::Static { token, source } => {
            row("token", &format!("{} (static)", mask_token(token)), source.as_str())
        }
        Credential::Session { name } => {
            let status = match SessionStore::new(app.paths.sessions.clone()).load(name) {
                Ok(Some(cache)) => format!("session '{name}', expires {}", cache.expires_at.to_rfc3339()),
                Ok(None) => format!("session '{name}', not logged in"),
                Err(_) => format!("session '{name}', cache damaged"),
            };
            row("token", &status, "session")
        }
        Credential::None => row("token", "-", "unset"),
    };
    let items = vec![
        row("profile", &settings.profile, profile_source.as_str()),
        row("url", &settings.url.value, settings.url.source.as_str()),
        optional_row("team", settings.team.as_ref()),
        optional_row("environment", settings.environment.as_ref()),
        row("output", settings.output.value.as_str(), settings.output.source.as_str()),
        row("timeout", &settings.timeout.value.to_string(), settings.timeout.source.as_str()),
        match &settings.session {
            Some(session) => row("session", session, "profile"),
            None => row("session", "-", "unset"),
        },
        token,
    ];
    Ok(Outcome::new(json!({ "items": items }), Kind::List(CONFIG_COLUMNS)))
}

fn list_profiles(app: &App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let active = app.profile();
    let items: Vec<Value> = files
        .profile_names()
        .into_iter()
        .map(|name| {
            let session = files.profile_value(&name, "session");
            let url = files
                .profile_value(&name, "url")
                .or_else(|| session.and_then(|s| files.session_value(s, "url")));
            json!({
                "active": name == active,
                "name": name,
                "session": session,
                "team": files.profile_value(&name, "team"),
                "url": url,
            })
        })
        .collect();
    Ok(Outcome::new(json!({ "items": items }), Kind::List(PROFILE_COLUMNS)))
}

async fn interactive(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let profile = app.profile();
    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    let mut files = app.files()?;
    let current_url = files
        .profile_value(&profile, "url")
        .or_else(|| files.profile_value(&profile, "session").and_then(|s| files.session_value(s, "url")))
        .unwrap_or(DEFAULT_URL)
        .to_string();
    let url = app
        .prompter
        .input("FluxGate API URL", Some(&current_url))?
        .trim()
        .trim_end_matches('/')
        .to_string();
    ApiClient::new(&url, None, timeout)?;

    let methods = vec![LOGIN_PASSWORD.to_string(), LOGIN_TOKEN.to_string()];
    if app.prompter.select("How do you sign in", &methods)? == 0 {
        let session = files.profile_value(&profile, "session").unwrap_or(profile.as_str()).to_string();
        files.set_profile_value(&profile, "session", &session);
        files.set_session_value(&session, "url", &url);
        // The session holds the url; a profile url would shadow it.
        files.remove_profile_value(&profile, "url");
        files.save_config(&app.paths)?;
        let username = app.prompter.input("Username", None)?;
        let response = password_login(&mut *app.prompter, &url, &username, timeout).await?;
        SessionStore::new(app.paths.sessions.clone())
            .save(&session, &SessionCache::from_login(&response, Utc::now()))?;
    } else {
        let token = app.prompter.secret("Token")?;
        files.set_credential_token(&profile, token.trim());
        files.set_profile_value(&profile, "url", &url);
        files.save_credentials(&app.paths)?;
        files.save_config(&app.paths)?;
    }

    let context = Context::connect(app.settings(&app.files()?)?, app.paths.clone()).await?;
    let mut files = app.files()?;
    let team_id = match context.token_team() {
        Some(token_team) => {
            files.set_profile_value(&profile, "team", token_team);
            Some(token_team.to_string())
        }
        None => {
            let teams = context.teams().await?;
            if teams.is_empty() {
                None
            } else {
                let names: Vec<String> = teams.iter().map(|team| team.name.clone()).collect();
                let chosen = &teams[app.prompter.select("Team", &names)?];
                // Store the name unless another team has the same name.
                let duplicate = teams.iter().filter(|t| t.name.eq_ignore_ascii_case(&chosen.name)).count() > 1;
                files.set_profile_value(&profile, "team", if duplicate { &chosen.id } else { &chosen.name });
                Some(chosen.id.clone())
            }
        }
    };
    if let Some(team_id) = team_id {
        let environments = context.environments(&team_id).await?;
        if !environments.is_empty() {
            let names: Vec<String> = environments.iter().map(|e| e.name.clone()).collect();
            let chosen = app.prompter.select("Default environment", &names)?;
            files.set_profile_value(&profile, "environment", &environments[chosen].name);
        }
    }
    let outputs = vec!["table".to_string(), "json".to_string(), "text".to_string()];
    let output = app.prompter.select("Default output", &outputs)?;
    files.set_profile_value(&profile, "output", &outputs[output]);
    files.save_config(&app.paths)?;
    Ok(Outcome::message(format!("Profile '{profile}' saved to {}", app.paths.config.display())))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli`
Expected: PASS (8 tests in `configure`).

- [ ] **Step 6: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): configure command for profiles, tokens and sessions"
```

---

### Task 12: teams and whoami

**Files:**
- Create: `fluxgate-cli/src/commands/teams.rs`, `fluxgate-cli/src/commands/whoami.rs`, `fluxgate-cli/tests/teams_whoami.rs`
- Modify: `fluxgate-cli/src/cli.rs`, `fluxgate-cli/src/commands/mod.rs`

**Interfaces:**
- Consumes: `Context::{teams, team_id, token_team, claims, settings}`, `find_team`, `team_names`, `is_uuid`, `Credential`, `TEAM_COLUMNS`.
- Produces: `Command::{Whoami, Teams(TeamsArgs)}`; `TeamsSubcommand::{List, Use { team: String }}`.

- [ ] **Step 1: Write the failing tests**

`tests/teams_whoami.rs`:

```rust
mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn mount_teams(h: &Harness) {
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": TEAM_A, "name": "Payments" }, { "id": TEAM_B, "name": "Checkout" } ])))
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn teams_list_marks_the_active_team() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["teams", "list"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", "checkout")]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let items = json_out(&r)["items"].as_array().unwrap().clone();
    assert_eq!(items[0]["active"], false);
    assert_eq!(items[1]["active"], true);
}

#[tokio::test]
async fn teams_use_saves_the_team_on_the_profile() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["teams", "use", "Checkout", "--output", "text"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), format!("Profile 'default' now uses team Checkout ({TEAM_B})"));
    assert!(h.read("config").contains("team=Checkout"));
}

#[tokio::test]
async fn teams_use_of_an_unknown_team_lists_the_choices() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["teams", "use", "billing", "--output", "text"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())])
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("available: Payments, Checkout"));
}

#[tokio::test]
async fn whoami_for_a_user_shows_the_active_team_and_all_teams() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["whoami"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", "payments")]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["kind"], "user");
    assert_eq!(value["username"], "alice");
    assert_eq!(value["team"], format!("Payments ({TEAM_A})"));
    assert_eq!(value["teams"], json!(["Payments", "Checkout"]));
    assert_eq!(value["credential"], "static token (env)");
    assert_eq!(value["tokenExpiresAt"], "2100-01-01T00:00:00+00:00");
}

#[tokio::test]
async fn whoami_for_a_system_client_needs_no_teams_request() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), system_token(TEAM_A));
    let r = h.run(&["whoami"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["kind"], "system_client");
    assert_eq!(value["team"], TEAM_A);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fluxgate-cli --test teams_whoami`
Expected: FAIL (exit code 2, unrecognized subcommand).

- [ ] **Step 3: Add the CLI definitions**

In `src/cli.rs`, add to `enum Command`:

```rust
    /// Show who the current credentials belong to.
    Whoami,
    /// Teams you can use.
    Teams(TeamsArgs),
```

and:

```rust
#[derive(Debug, Args)]
pub struct TeamsArgs {
    #[command(subcommand)]
    pub command: TeamsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum TeamsSubcommand {
    /// List your teams; the active one is marked.
    List,
    /// Make a team the profile's default.
    Use {
        /// Team name or id.
        team: String,
    },
}
```

- [ ] **Step 4: Implement the commands**

In `src/commands/mod.rs` add `pub mod teams; pub mod whoami;` and to `dispatch`:

```rust
        Command::Whoami => whoami::run(app).await,
        Command::Teams(args) => teams::run(args, app).await,
```

`src/commands/teams.rs`:

```rust
use serde_json::{Value, json};

use super::App;
use crate::cli::{TeamsArgs, TeamsSubcommand};
use crate::context::{find_team, is_uuid, team_names};
use crate::error::CliError;
use crate::output::{Kind, Outcome, TEAM_COLUMNS};

pub async fn run(args: TeamsArgs, app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    let teams = context.teams().await?;
    match args.command {
        TeamsSubcommand::List => {
            let active = context.settings.team.as_ref().map(|team| team.value.clone());
            let items: Vec<Value> = teams
                .iter()
                .map(|team| {
                    let is_active = active
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(&team.id) || a.eq_ignore_ascii_case(&team.name));
                    json!({ "active": is_active, "name": team.name, "id": team.id })
                })
                .collect();
            Ok(Outcome::new(json!({ "items": items }), Kind::List(TEAM_COLUMNS)))
        }
        TeamsSubcommand::Use { team } => {
            let wanted = team.trim();
            let chosen = if is_uuid(wanted) {
                teams
                    .iter()
                    .find(|t| t.id.eq_ignore_ascii_case(wanted))
                    .cloned()
                    .ok_or_else(|| {
                        CliError::Usage(format!("team {wanted} not found; available: {}", team_names(&teams)))
                    })?
            } else {
                find_team(&teams, wanted)?
            };
            let profile = context.settings.profile.clone();
            let mut files = app.files()?;
            files.set_profile_value(&profile, "team", wanted);
            files.save_config(&app.paths)?;
            Ok(Outcome::message(format!(
                "Profile '{profile}' now uses team {} ({})",
                chosen.name, chosen.id
            )))
        }
    }
}
```

`src/commands/whoami.rs`:

```rust
use chrono::{DateTime, Utc};
use serde_json::json;

use super::App;
use crate::config::Credential;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let context = app.connect().await?;
    let claims = context.claims.clone();
    let kind = match &claims {
        Some(claims) if claims.is_system_client() => "system_client",
        Some(_) => "user",
        None => "unknown",
    };
    let credential = match &context.settings.credential {
        Credential::Static { source, .. } => format!("static token ({})", source.as_str()),
        Credential::Session { name } => format!("session '{name}'"),
        Credential::None => "none".to_string(),
    };
    let mut value = json!({
        "profile": context.settings.profile,
        "session": context.settings.session,
        "credential": credential,
        "kind": kind,
        "username": claims.as_ref().map(|c| c.username.clone()),
        "id": claims.as_ref().map(|c| c.sub.clone()),
        "isAdmin": claims.as_ref().map(|c| c.is_admin),
        "tokenExpiresAt": claims
            .as_ref()
            .and_then(|c| DateTime::<Utc>::from_timestamp(c.exp, 0))
            .map(|expires| expires.to_rfc3339()),
    });
    if kind == "system_client" {
        // System clients may not list teams; the token names its team.
        value["team"] = json!(context.token_team());
    } else {
        let teams = context.teams().await?;
        value["teams"] = json!(teams.iter().map(|team| team.name.clone()).collect::<Vec<_>>());
        value["team"] = match context.team_id().await {
            Ok(id) => json!(teams
                .iter()
                .find(|team| team.id.eq_ignore_ascii_case(&id))
                .map(|team| format!("{} ({})", team.name, team.id))
                .unwrap_or(id)),
            Err(CliError::Usage(message)) => json!(format!("unresolved: {message}")),
            Err(err) => return Err(err),
        };
    }
    Ok(Outcome::new(value, Kind::Object))
}
```

If `whoami_for_a_user_shows_the_active_team_and_all_teams` fails on `team` because no team is configured, check the test passes `FLUXGATE_TEAM`; the `unresolved:` text is the expected value when none is set.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p fluxgate-cli`
Expected: PASS (5 tests in `teams_whoami`).

- [ ] **Step 6: Commit**

```bash
git add fluxgate-cli
git commit -m "feat(cli): teams and whoami commands"
```

---

### Task 13: Docs, lint and final verification

**Files:**
- Create: `fluxgate-cli/README.md`
- Modify: `CLAUDE.md`, `CHANGELOG.md`

- [ ] **Step 1: Write `fluxgate-cli/README.md`**

````markdown
# fluxgate CLI

Command line client for the FluxGate admin API: flag reads, evaluation, approvals, stage changes, and AWS-style profiles and login sessions.

## Install

```bash
cargo install --path fluxgate-cli
```

## Quick start

```bash
fluxgate configure                 # url, login, team, environment, output
fluxgate whoami
fluxgate flags list
fluxgate evaluate --flag checkout --targeting-key user-1 --exit-code
```

## Profiles

Settings live in `~/.fluxgate/config`:

```ini
[default]
session = corp
team = payments
environment = staging
output = table

[profile checkout-prod]
session = corp
team = checkout
environment = production

[session corp]
url = https://fluxgate.example.com/api/v1
```

Profiles that name the same `session` share one login: log in once, then switch teams with `--profile checkout-prod` or `fluxgate teams use checkout`.

Static tokens, such as system-client tokens for CI, live in `~/.fluxgate/credentials` (mode 0600):

```ini
[ci-payments]
token = <system-client token>
```

A profile that uses a static token needs a `url` (and optionally `team`) in its config section.

`fluxgate` rewrites these files with `configure`, `login` and `teams use`. Comments are not kept when a file is rewritten, and comments must be on their own line (`key = value # note` makes `# note` part of the value).

## Where values come from

Each setting is taken from the first place that has it:

1. Command line flag
2. Environment variable
3. Profile in `~/.fluxgate/config`
4. The profile's session (url only)
5. Default

| Setting     | Flag                           | Environment variable                          | Default                        |
|-------------|--------------------------------|-----------------------------------------------|--------------------------------|
| profile     | `--profile`                    | `FLUXGATE_PROFILE`                            | `default`                      |
| url         | `--url` (`--base-url`)         | `FLUXGATE_URL`                                | `http://localhost:8080/api/v1` |
| team        | `--team` (`--team-id`)         | `FLUXGATE_TEAM` (`FLUXGATE_TEAM_ID`)          | none                           |
| environment | `--env` (`--environment-id`)   | `FLUXGATE_ENVIRONMENT` (`FLUXGATE_ENVIRONMENT_ID`) | none                      |
| output      | `--output json\|table\|text`, `--json` | `FLUXGATE_OUTPUT`                     | `table` on a terminal, else `json` |
| timeout     | `--timeout`                    | `FLUXGATE_TIMEOUT`                            | 30 seconds                     |

Credentials: `--token`, then `FLUXGATE_TOKEN`, then the profile's token in the credentials file, then the profile's login session. `fluxgate configure list` shows every value and its source.

File locations can be changed with `FLUXGATE_CONFIG_FILE` and `FLUXGATE_SHARED_CREDENTIALS_FILE`. Login sessions are cached in a `sessions` directory next to the config file.

Teams and environments can be given by name or id. A system-client token is bound to one team: `fluxgate` uses that team and refuses a different one.

## Login sessions

```bash
fluxgate login --password            # asks for username and password
fluxgate logout                      # revokes the session on the server
fluxgate logout --all
```

Access tokens are refreshed automatically. When the refresh token has expired, commands fail with `session expired: run fluxgate login --profile <p>`. SSO login is not available yet; SSO users can use a static token.

## CI usage

```bash
export FLUXGATE_URL=https://fluxgate.example.com/api/v1
export FLUXGATE_TOKEN=$SYSTEM_CLIENT_TOKEN
if fluxgate evaluate --flag new-checkout --env production --targeting-key ci --exit-code; then
  echo "flag on"
fi
fluxgate rollout promote --flag new-checkout --env production --request DEPLOYMENT_REQUESTED \
  --reason "release 1.4" --external-ref PROJ-123
```

## Exit codes

| Code | Meaning                                                   |
|------|-----------------------------------------------------------|
| 0    | Success                                                   |
| 1    | Other error                                               |
| 2    | Usage or configuration error                              |
| 3    | Authentication: HTTP 401, no credentials, session expired |
| 4    | Forbidden (HTTP 403)                                      |
| 5    | Not found (HTTP 404)                                      |
| 6    | Conflict (HTTP 409)                                       |
| 7    | Server or network error (HTTP 5xx, timeout, connection)   |
| 10   | `evaluate --exit-code` and the flag is off                |

Errors go to stderr: `error: <message> (code <code>, HTTP <status>)`, or the server's error JSON when the output format is json.
````

- [ ] **Step 2: Update `CLAUDE.md`**

In the Overview, change "Cargo workspace (edition 2024) with four crates" to "five crates", and add this bullet after `feature-toggle-shared`:

```markdown
- `fluxgate-cli` — the `fluxgate` command line client (library `fluxgate_cli` + binary). AWS-style profiles (`~/.fluxgate/config`, `~/.fluxgate/credentials`), cached login sessions with locked refresh, and commands over the REST API. Pure HTTP client: no DB, builds without `DATABASE_URL`. Integration tests drive `fluxgate_cli::run` against `wiremock`. See `fluxgate-cli/README.md`.
```

In the Commands block, add:

```bash
cargo test -p fluxgate-cli                   # no DB needed
cargo run -p fluxgate-cli -- --help
```

In the Overview bullet for `feature-toggle-backend`, remove "Binary `fluxgate` (`src/bin/fluxgate.rs`);" so it reads "Binary `src/bin/export-contracts.rs` exports API contracts."

- [ ] **Step 3: Update `CHANGELOG.md`**

Add under `## Unreleased` → `### Added` as the first bullet:

```markdown
- **`fluxgate` CLI moved to its own crate with profiles and login (CLI).** The CLI now lives in the new `fluxgate-cli` workspace crate (binary `fluxgate`) and no longer builds as part of the backend. New: AWS-style profiles in `~/.fluxgate/config` (`[default]`, `[profile <name>]`, shared `[session <name>]` sections), static tokens in `~/.fluxgate/credentials`, `fluxgate login --password` with cached sessions and automatic token refresh, `logout`, `configure` (interactive, `set`, `get`, `list`, `list-profiles`), `teams list|use` and `whoami`. Teams and environments can be given by name. Fixed: `--json` now changes the output (`--output json|table|text`, default table on a terminal), `approvals list --status` filters again (it sent the wrong parameter), lists page with `--limit`, `--offset` and `--all` instead of stopping at the first page, `config export` includes every flag and the team's environments, requests time out (30 s default), a system-client token used with another team's id is refused instead of acting on the token's team, and exit codes tell auth, permission, not-found, conflict and server errors apart. New options: `evaluate --exit-code`, `rollout promote --flag <key> --env <name>` with `--reason`, `--external-ref` and `--freeze-override-reason`. Old flags and env vars (`--base-url`, `--team-id`, `--environment-id`, `--feature-key`, `FLUXGATE_TEAM_ID`, `FLUXGATE_ENVIRONMENT_ID`) still work. See `fluxgate-cli/README.md`.
```

- [ ] **Step 4: Format, lint and test the workspace**

Run:

```bash
cargo fmt
cargo clippy -p fluxgate-cli --all-targets -- -D warnings
cargo test -p fluxgate-cli
SQLX_OFFLINE=true cargo clippy --all-targets
```

Expected: no formatting diff left after `cargo fmt`, no clippy warnings for `fluxgate-cli`, all `fluxgate-cli` tests pass, workspace clippy passes. Fix any clippy findings in the files of this plan before continuing.

- [ ] **Step 5: Smoke-test the binary**

Run:

```bash
cargo run -q -p fluxgate-cli -- --version
cargo run -q -p fluxgate-cli -- --help
FLUXGATE_CONFIG_FILE=/tmp/fg-smoke/config cargo run -q -p fluxgate-cli -- configure list --output table
```

Expected: version `fluxgate 0.1.0`; help lists `health flags approvals evaluate config rollout login logout configure whoami teams`; the list shows `url http://localhost:8080/api/v1 default` and `token - unset`. Remove `/tmp/fg-smoke` afterwards.

- [ ] **Step 6: Update the knowledge graph**

Run: `graphify update .`
Expected: completes without errors.

- [ ] **Step 7: Commit**

```bash
git add fluxgate-cli/README.md CLAUDE.md CHANGELOG.md fluxgate-cli
git commit -m "docs(cli): README, CLAUDE.md and changelog for the fluxgate CLI"
```
