# JI-45: Native Jira webhook HMAC authentication (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Done in 088c2b2 |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | JI-41 (column `native_webhook_secret_enc`) |
| Behavior change | Additive. A second way to authenticate inbound events. The phase 1 secret keeps working. |
| Design | [design.md §3.5, J24](../design.md#35-native-webhook-hmac-ji-45) |

## Goal

Let a Jira admin use a native Jira webhook (no Automation rule). The webhook signs the body with a shared secret, and FluxGate checks the signature.

## Step 0: confirm what Jira sends (before code)

Read Atlassian's current docs and write the answers in the handoff log, with the links:
- Jira Cloud: the "Secret" field on admin webhooks. The header name, expected `X-Hub-Signature`. The format, expected `sha256=<lowercase hex>`. What the HMAC covers, expected the raw request body.
- Jira Data Center: which version, if any, signs webhooks, and with which header and format.

If the Cloud header or format differs from `X-Hub-Signature: sha256=<hex>`, stop and ask the user before coding. If Data Center does not sign, implement Cloud only. JI-50 then documents "Data Center: use Automation".

## Current code (verify first, paths under `feature-toggle-backend/src`)

| Piece | Where |
|---|---|
| Handler | `rest/jira_events.rs` `receive_jira_event` (~155): `presented_secret` (~114), `secret_matches` (~129), body is `web::Bytes` (raw, good for HMAC) |
| Order after JI-44 | parse id, get integration, rate limit, auth, body parse |
| Secret generation | `logic/jira_integration.rs` `generate_secret` (32 random bytes, base64url) |
| Encryption | `secret_box::encrypt_with_aad` / `decrypt_with_aad` (AAD = integration id bytes) |
| Column | `jira_integrations.native_webhook_secret_enc` (JI-41) |
| Constant-time compare | `subtle` is a dependency already |
| Fixtures | `tests/fixtures/jira/*.json` (Cloud webhook bodies) |

## Interfaces

- Produces:
  - `logic/jira_signature.rs`:
    ```rust
    /// True when `header` is `sha256=<hex>` and equals HMAC-SHA256(secret, body). Constant time.
    pub fn signature_matches(secret: &str, body: &[u8], header: &str) -> bool;
    pub const SIGNATURE_HEADER: &str = "X-Hub-Signature";
    ```
  - Repository: `set_native_webhook_secret_tx(conn, id, Option<String>)`.
  - REST (tag `Jira`, `ApiDoc`), team admin:
    - `POST /api/v1/jira-integrations/{id}/native-webhook-secret` returns `200 {"secret": "<plaintext>"}`, shown once. A second call rotates the secret.
    - `DELETE /api/v1/jira-integrations/{id}/native-webhook-secret` returns `204`.
    - Activity `jira_integration_updated` with `changed_fields: ["native_webhook_secret"]`, never the value.
  - `JiraIntegrationResponse.hasNativeWebhookSecret` (field added in JI-41) becomes true.

## Rules

- Auth passes when **either** check passes:
  1. The phase 1 secret: `presented_secret` plus `secret_matches`, as today.
  2. `X-Hub-Signature` is present, the integration has a native secret, it decrypts, and `signature_matches(secret, &body, header)`.
- Both fail: 401 with the same message as today. The response must not reveal which check failed.
- `signature_matches`:
  - The header must be exactly `sha256=` followed by 64 hex chars. Accept upper case hex by lowercasing.
  - Any other prefix (`sha1=`) or length returns false.
  - Compute with `hmac::Hmac<sha2::Sha256>`, then `verify_slice` (constant time).
- The HMAC covers the raw bytes **before** JSON parsing. Do not re-serialise.
- If decryption fails (the key changed), treat it as no native secret. Log `warn` once, with the integration id only.
- Creating a native secret without `FLUXGATE_ENCRYPTION_KEY` returns `encryption_key_missing`.

## Steps

- [ ] **Step 1:** run `cargo add hmac` (same major version line as `sha2 0.10`, that is `hmac 0.12`).
- [ ] **Step 2: failing unit tests** in `logic/jira_signature.rs`:
  ```rust
  fn sign(secret: &str, body: &[u8]) -> String {
      let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
      mac.update(body);
      format!("sha256={}", hex_lower(&mac.finalize().into_bytes()))
  }
  #[test] fn valid_signature_matches()            { let s = random_secret(); assert!(signature_matches(&s, b"{}", &sign(&s, b"{}"))); }
  #[test] fn upper_case_hex_matches()
  #[test] fn changed_body_fails()                 // sign b"{}" , check b"{ }"
  #[test] fn wrong_secret_fails()
  #[test] fn sha1_prefix_fails()
  #[test] fn missing_prefix_or_bad_length_fails() // "abc", "sha256=", "sha256=zz..", 63 hex chars
  ```
  `random_secret()` uses `generate_secret()`. Never a literal.
- [ ] **Step 3:** run `cargo test -p feature-toggle-backend jira_signature`. It fails. Implement it. It passes.
- [ ] **Step 4: failing handler tests** (`rest/jira_events.rs` mocks; the mock integration row carries a `native_webhook_secret_enc` sealed in the test with a test key):
  ```rust
  #[actix_web::test] async fn signed_native_webhook_is_accepted_without_the_bearer_secret()
  // body = tests/fixtures/jira/<cloud webhook fixture>, header X-Hub-Signature = sign(native, body) -> 200, engine runs
  #[actix_web::test] async fn bad_signature_is_401_and_does_nothing()
  #[actix_web::test] async fn signature_without_a_stored_native_secret_is_401()
  #[actix_web::test] async fn bearer_secret_still_works_when_a_native_secret_exists()
  ```
- [ ] **Step 5:** implement the auth change. The tests pass, and the existing ones still pass.
- [ ] **Step 6: REST secret endpoints.**
  - Write the DB tests first:
    - Create returns a secret, and the DB value decrypts to it.
    - Rotate changes it.
    - Delete clears it.
    - Plain user gets 403.
    - The activity row has no value.
    - `GET` integration shows `hasNativeWebhookSecret`.
  - Then implement.
- [ ] **Step 7:**
  - Run `cargo fmt`, `cargo clippy --all-targets` and the full backend test suite.
  - Export the contracts, copy them to the baseline, and run the check.
  - Run `graphify update .`.
- [ ] **Step 8:** commit `feat(jira): accept HMAC-signed native Jira webhooks (JI-45)`. The message notes the contract baseline update.
- [ ] **Step 9: manual check.** Generate a native secret. Sign a fixture body with `openssl dgst -sha256 -hmac "$SECRET"` (secret from an environment variable), then curl it. Expect 200. Change one byte and expect 401. Record the results, not the secret.

## Done when

- A correctly signed native webhook is processed exactly like an Automation call. A bad signature is 401. The phase 1 secret still works.
- Step 0 findings are in the handoff log.
- All checks pass. Handoff log, `../HANDOFF.md` and the README table updated.

## Handoff log

2026-10-03, backend `088c2b2`:

Step 0 findings (Atlassian docs, read 2026-10-03):
- Jira Cloud admin webhooks, "Secret": header `X-Hub-Signature`, value `method=signature`, example `sha256=a4771c39...` (lowercase hex). The HMAC covers the payload with the webhook secret and the algorithm in `method`; the docs say to treat the payload as UTF-8 text, which is the raw body. This matches the brief. Source: https://developer.atlassian.com/cloud/jira/platform/webhooks/ (section "Secure admin webhooks"; WebSub signing, https://www.w3.org/TR/websub/#signing-content).
- Jira Data Center: webhooks can be secured with a secret token. "each request is signed via HMAC", default HMACSha256, header `X-Hub-Signature`, e.g. `x-hub-signature: sha256=c3383246...`. Source: https://confluence.atlassian.com/spaces/ADMINJIRASERVER/pages/938846912/Managing+webhooks (the DC 11.3 page). The page does not say which version added signing. Related: https://confluence.atlassian.com/spaces/ADMINJIRASERVER/pages/1299913153/Configuring+webhook+security (DC 9.12 to 11.3, names no header or format). So DC signs in the same format as Cloud, but the first version is unconfirmed. The code is the same for both. JI-50 should say "Data Center: check the Managing webhooks page for your version, else use Automation".

Implemented as specified.
- `logic/jira_signature.rs`: `signature_matches`, `SIGNATURE_HEADER`. Header must be `sha256=` plus exactly 64 hex digits (upper case accepted); compare through `hmac` `verify_slice` (constant time). `hmac 0.12` added.
- `receive_jira_event`: auth step is now `bearer_ok || native_signature_ok(req, integration, body)`. HMAC runs on the raw `web::Bytes` before JSON parsing. One 401 message for both failures. No stored secret or a decrypt failure (warn once per integration id, id only) counts as a failed check.
- `JiraIntegrationRepository::set_native_webhook_secret(_tx)(id, Option<String>)` (brief said `set_native_webhook_secret_tx` only; both exist, like the other setters).
- Logic `generate_native_webhook_secret_in_tx` / `remove_native_webhook_secret_in_tx` (`logic/jira_integration_tx.rs`); sealed with AAD = integration id bytes.
- REST: `POST /api/v1/jira-integrations/{id}/native-webhook-secret` returns `200 {"secret"}` (schema `JiraNativeWebhookSecretResponse`); `DELETE` returns 204. `encryption_key_missing` (400) when the key is unset. Activity `jira_integration_updated`, `changed_fields: ["native_webhook_secret"]`, plus `has_native_webhook_secret`; never a value. Both routes are in the policy route lists, so plain users get 403 through the existing policy tests.
- OpenAPI: new endpoints, `X-Hub-Signature` header parameter on the events route. Contract baseline updated.
- Test helper: `rest::jira_integration::tests` is now `pub(crate)` and `ensure_encryption_key` is `pub(crate)`, so the `jira_events` mock tests reuse the same process-wide test key.

Tests: 6 `jira_signature` unit tests; 5 handler mock tests (`signed_native_webhook_is_accepted_without_the_bearer_secret` using the Cloud fixture `cloud_webhook_status_change.json`, `bad_signature_is_401_and_does_nothing` (wrong secret and one-byte body change), `signature_without_a_stored_native_secret_is_401`, `bearer_secret_still_works_when_a_native_secret_exists`, `undecryptable_native_secret_counts_as_none`); 1 DB test `native_webhook_secret_create_rotate_delete` (create decrypts to the returned value, rotate changes it, delete clears it, GET flag, activity rows hold no value, 404 for unknown id). The missing-key 400 is not tested: the key is process-wide in tests.

Verified: `cargo fmt`; `cargo clippy --all-targets` (no warnings in touched files); `cargo test -p feature-toggle-backend`: 939 lib pass; integration 351 pass, 1 fail: `database::jira_writeback_capture_test::list_window_is_ascending_and_bounded` (JI-43 test; it reads a 30 s window 3 h in the past from the shared test DB and picks up stale `stage_*` rows from earlier runs; fails the same way alone; unrelated to JI-45). Contract check passes after the baseline update.

Manual check (step 9): not run. It needs an admin login to create an integration; no known credentials. Covered by the handler tests and JI-50's end-to-end run.
