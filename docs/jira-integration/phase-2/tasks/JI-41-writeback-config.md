# JI-41: Write-back configuration, encrypted credential, test connection (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 3de0dec |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — |
| Behavior change | Additive. New columns, config section, three endpoints, new fields on the integration response. Nothing is sent to Jira except by "test connection". |
| Design | [design.md §3.1, J17-J19](../design.md#31-write-back-configuration-ji-41) |

## Goal

A team admin can turn write-back on for a Jira integration: Jira edition, account email (Cloud), API token or PAT (stored encrypted, never returned), toggles for comments and remote link. A test button proves the credential works.

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Integration row | `database/entity.rs` `JiraIntegrationRow` (~663) |
| Repository | `database/jira_integration.rs`: trait ~62-98, tx trait ~107-146, `UpdateJiraIntegration` (~41, `Option<Option<..>>`), dynamic update `QueryBuilder` (~266) |
| Logic | `logic/jira_integration.rs` (validation), `logic/jira_integration_tx.rs` (`update_jira_integration_in_tx` ~159 writes `jira_integration_updated` with `changed_fields`) |
| REST | `rest/jira_integration.rs`: `JiraIntegrationResponse` (~34), PATCH handler (~350-389), `configure` (~539) |
| Encryption | `logic/secret_box.rs`: `encrypt_with_aad`, `decrypt_with_aad`, `is_configured`; error `RestError::encryption_key_missing()` (`rest/error.rs` ~173) |
| Config | `config.rs`: optional-section pattern `#[serde(default)] pub auth: AuthConfig` with `impl Default` and `sanitized()`; `allowed_origin` |
| Policy | `logic/policy.rs` ~311-324: every `/jira-integrations/**` path is already `ManageJiraIntegrations` (team admin). Nothing to add. |
| Wiring | `lib.rs`: `web::Data` for Jira repositories ~401-409; `cfg.auth` registered as data ~396 |
| REST tests with DB | `rest/jira_integration.rs` tests (`test_pool` ~558) |

## Interfaces

Produces (later tasks use these exact names):

- Migration `<ts>_jira_writeback.sql`: columns of design §3.1 (`writeback_enabled`, `writeback_comments`, `writeback_remote_link`, `jira_auth_kind`, `jira_account_email`, `jira_credential_enc`, `writeback_paused_reason`, `native_webhook_secret_enc`).
- `JiraIntegrationRow` gains the same fields (`jira_auth_kind: Option<String>`, others as typed in SQL).
- `config.rs`:
  ```rust
  #[derive(Debug, Clone, Deserialize)]
  #[serde(default)]
  pub struct JiraConfig {
      /// Allow `http://` Jira base URLs for write-back (default false).
      pub allow_insecure_http: bool,
      /// UI origin for remote links; falls back to `allowed_origin`.
      pub ui_base_url: Option<String>,
  }
  // Config gains: #[serde(default)] pub jira: JiraConfig
  impl Config { pub fn jira_ui_base_url(&self) -> Option<String> }  // ui_base_url, else allowed_origin if one absolute http(s) URL; trailing '/' trimmed
  ```
  Register `web::Data<JiraConfig>` in `lib.rs`.
- `logic/jira_client.rs` (new):
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum JiraEdition { Cloud, DataCenter }        // from jira_auth_kind 'cloud_basic' | 'dc_pat'
  impl JiraEdition {
      pub fn from_auth_kind(kind: &str) -> Option<Self>;
      pub fn api_version(self) -> &'static str;     // "3" | "2"
  }
  pub enum JiraAuth { Basic { email: String, token: String }, Bearer { token: String } }
  pub struct JiraClient { /* reqwest::Client (timeout 10 s, redirect none), base_url, edition, auth */ }
  #[derive(Debug, PartialEq, Eq)]
  pub struct JiraResponse { pub status: u16, pub retry_after_secs: Option<u64>, pub body_excerpt: String }  // body cut to 300 chars
  impl JiraClient {
      pub fn new(base_url: &str, edition: JiraEdition, auth: JiraAuth) -> Result<Self, Error>;
      pub async fn myself(&self) -> Result<JiraResponse, JiraTransportError>;   // GET rest/api/{v}/myself
  }
  #[derive(Debug)]
  pub struct JiraTransportError(pub String);   // timeout or connect error; message never contains the auth value
  /// Decrypts the stored credential and builds the client. None when write-back is not configured.
  pub fn client_for(row: &JiraIntegrationRow) -> Result<Option<JiraClient>, Error>;
  ```
  `Debug` for `JiraAuth` and `JiraClient` prints `***` for the token (manual impl, not derived).
- REST (tag `Jira`, register in `ApiDoc`):
  - `PUT /api/v1/jira-integrations/{id}/writeback`. Body `UpdateJiraWritebackRequest { enabled: bool, comments: bool, remoteLink: bool, authKind: Option<String>, accountEmail: Option<String>, credential: Option<String> }`. Returns `JiraIntegrationResponse`.
  - `POST /api/v1/jira-integrations/{id}/writeback/test`. Returns `JiraWritebackTestResponse { ok: bool, status: Option<u16>, message: String }`.
  - `POST /api/v1/jira-integrations/{id}/writeback/resume`. Returns `JiraIntegrationResponse`.
  - `JiraIntegrationResponse` gains `writeback: JiraWritebackResponse { enabled, comments, remoteLink, authKind, accountEmail, hasCredential, pausedReason }` and `hasNativeWebhookSecret: bool`.
- Logic: `update_jira_writeback_in_tx(conn, actor, id, WritebackPatch, &JiraConfig, ui_base_url: Option<&str>)` and `resume_jira_writeback_in_tx(conn, actor, id)`. Both write `jira_integration_updated` with `changed_fields` (for example `["writeback_enabled","jira_credential"]`), never the value.

## Rules

- **Validation**, applied only when `enabled = true`:
  - `jira_base_url` must be set and must be `https`, or `http` with `allow_insecure_http`.
  - `authKind` must be `cloud_basic` or `dc_pat`.
  - A credential must be stored or sent now.
  - `cloud_basic` needs `accountEmail` (contains `@`, at most 254 chars).
  - `remoteLink = true` needs a UI base URL; otherwise 400 `ui base URL is not configured`.
- `credential`: trimmed, 1-1000 chars.
  - Omitted keeps the stored value. `""` clears it, and is only allowed when `enabled = false`.
  - Saving a new credential clears `writeback_paused_reason`.
- No `FLUXGATE_ENCRYPTION_KEY` while a credential is sent: `encryption_key_missing`.
- **Test connection:**
  - Not configured: 400 `write-back is not configured`.
  - Jira 2xx: `{ok: true, status, message: "connected as <displayName>"}`. Read `displayName` from the JSON when present.
  - Other status: `{ok: false, status, message: "Jira returned <status>"}`.
  - Transport error: `{ok: false, status: null, message: "could not reach Jira: <timeout|connect>"}`.
  - The response is always HTTP 200 except the 400 above.
- Turning write-back off sets pending jobs to `dead`. That needs the JI-42 table. Do it in JI-42, not here, and note it in the handoff log.

## Steps

- [ ] **Step 1: dev dependency.** `cargo add --dev wiremock` in `feature-toggle-backend/`.
- [ ] **Step 2: failing unit tests** in `logic/jira_client.rs`:
  ```rust
  #[test] fn edition_from_auth_kind() // cloud_basic -> Cloud "3"; dc_pat -> DataCenter "2"; other -> None
  #[test] fn debug_hides_the_token() { let a = JiraAuth::Bearer { token: "tok-123".into() }; assert!(!format!("{a:?}").contains("tok-123")); }
  #[tokio::test] async fn myself_sends_basic_auth_on_cloud()   // wiremock: GET /rest/api/3/myself with header Authorization "Basic base64(email:token)" -> 200
  #[tokio::test] async fn myself_sends_bearer_on_data_center() // GET /rest/api/2/myself, "Bearer <token>"
  #[tokio::test] async fn redirect_is_not_followed()          // 302 Location: other host -> JiraResponse.status == 302, no request to the other host
  #[tokio::test] async fn body_excerpt_is_cut_to_300_chars()
  ```
  The token is `format!("tok-{}", uuid::Uuid::new_v4())`, never a literal stored in the repo.
- [ ] **Step 3:** run `cargo test -p feature-toggle-backend jira_client`. Expected: compile failure, because the module does not exist.
- [ ] **Step 4:** implement `logic/jira_client.rs`. Register it in `logic/mod.rs`. Re-run: pass.
- [ ] **Step 5: failing config tests** in `config.rs`:
  - `jira_section_defaults` (no `[jira]` section, so `allow_insecure_http` is false).
  - `jira_ui_base_url_falls_back_to_allowed_origin`.
  - `jira_ui_base_url_none_for_a_non_url_origin` (`allowed_origin = "*"`).
- [ ] **Step 6:** implement `JiraConfig`. Pass.
- [ ] **Step 7: migration and repository.**
  - Add the migration.
  - Add the fields to `JiraIntegrationRow` and to every `SELECT` of the row. `grep -n "secret_hash" database/jira_integration.rs` finds them.
  - Add repository functions `set_writeback_tx(conn, id, JiraWritebackColumns)` and `set_writeback_paused_tx(conn, id, Option<String>)`, plus pool versions.
- [ ] **Step 8: failing REST tests** (DB, like the existing `rest/jira_integration.rs` tests):
  ```rust
  #[actix_web::test] async fn writeback_credential_is_stored_encrypted_and_never_returned()
  // PUT with credential -> 200; response JSON as string does not contain the credential; hasCredential = true;
  // DB column jira_credential_enc != credential and decrypt_with_aad(.., id) == credential.
  #[actix_web::test] async fn enabling_without_base_url_or_credential_is_400()
  #[actix_web::test] async fn http_base_url_needs_allow_insecure_http()
  #[actix_web::test] async fn cloud_needs_account_email()
  #[actix_web::test] async fn remote_link_needs_a_ui_base_url()
  #[actix_web::test] async fn saving_a_credential_clears_the_paused_reason()
  #[actix_web::test] async fn test_connection_reports_jira_status()      // wiremock 200 and 401
  #[actix_web::test] async fn plain_user_and_system_client_get_403()
  #[actix_web::test] async fn activity_row_lists_changed_fields_without_values()
  ```
  Set `FLUXGATE_ENCRYPTION_KEY` for these tests the way the SSO tests do (`grep -rn FLUXGATE_ENCRYPTION_KEY src tests`). Mark them `#[serial_test::serial]` if the SSO tests are.
- [ ] **Step 9:** run them. Expected: fail with 404 (routes missing).
- [ ] **Step 10:** implement the logic, handlers and response fields. Register them in `configure` and `ApiDoc`. Pass.
- [ ] **Step 11:**
  - Run `cargo fmt`, `cargo clippy --all-targets` and the full `cargo test -p feature-toggle-backend`.
  - Run `./scripts/export-contracts.sh`, copy the hashes to the baseline, then run `./scripts/check-contract-compat.sh`.
  - Run `graphify update .`.
- [ ] **Step 12:** commit `feat(jira): write-back configuration and test connection (JI-41)`. The message says the contract baseline was updated.

## Done when

- The credential round-trips encrypted, never appears in a response or activity row, and test connection reports the Jira status.
- All checks pass. Handoff log, `../HANDOFF.md` and the README table updated.

## Handoff log

2026-10-03, backend `3de0dec`:
- Implemented as specified: migration `20261004050000_jira_writeback.sql`, `JiraIntegrationRow` fields, `JiraWritebackColumns` + `set_writeback(_tx)` / `set_writeback_paused(_tx)` on the repository, `config::JiraConfig`, `Config::jira_ui_base_url`, `logic/jira_client.rs`, the three endpoints and response fields, contract baseline updated.
- Deviations (JI-42 must know):
  - `update_jira_writeback_in_tx` and `resume_jira_writeback_in_tx` follow the existing tx pattern: `(conn, repo, activity_repo, id, patch, &JiraConfig, ui_base_url: Option<&str>, actor)` and `(conn, repo, activity_repo, id, actor)`. The brief omitted `repo`/`activity_repo`.
  - Besides `web::Data<JiraConfig>`, `lib.rs` registers `web::Data<config::JiraUiBaseUrl>` (the resolved `jira_ui_base_url()`), which the PUT handler passes to the logic.
  - `client_for` returns `None` when base URL, auth kind or credential is missing (or the Cloud email). It does NOT check `writeback_enabled` (so "test connection" works before enabling). The JI-42 sender must check `row.writeback_enabled` (and `writeback_paused_reason`) itself. A decrypt failure is `Error::InvalidInput`.
  - Extra method `JiraClient::myself_display_name()`: `body_excerpt` (300 chars) is too short to hold `displayName` on Cloud, so test connection parses the full body.
  - `authKind` / `accountEmail`: omitted keeps, blank clears. A blank credential clears only with `enabled = false`.
  - Credential decrypts with AAD `integration_id.as_bytes()` (same as SSO).
- NOT done here (JI-42): turning write-back off must set pending jobs to `dead` with `last_error = 'write-back disabled'`; the jobs table does not exist yet. Hook it into `update_jira_writeback_in_tx` when `enabled` goes true to false (and in `update_jira_integration_in_tx` if it should follow `enabled`).
- Activity: `jira_integration_updated` with `changed_fields` (`writeback_enabled`, `writeback_comments`, `writeback_remote_link`, `jira_auth_kind`, `jira_account_email`, `jira_credential`, `writeback_paused_reason`) plus flags and `has_credential`; never the credential, sealed value or email.
- Tests: 6 `jira_client` unit, 3 config, 8 REST DB tests (`rest::jira_integration`, `#[serial]`, random key set once via env), policy route lists extended (403 for plain users, other-team admins and system clients covered by the existing policy tests). Full `cargo test -p feature-toggle-backend`: 862 lib + 327 integration pass; one run flaked `database::feature_test::test_pending_approval_listing_maps_feature_metadata` (passes alone and on 2 reruns; unrelated).
- `wiremock` dev dependency added; `Cargo.lock` also bumped hyper 1.6.0 to 1.11.1 and h2 0.4.12 to 0.4.19.
- The secret_box key is cached in a `OnceLock` on first use: do not call `secret_box::is_configured()` in a test before setting the env key.
