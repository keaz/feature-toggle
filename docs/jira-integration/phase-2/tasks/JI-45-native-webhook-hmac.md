# JI-45: Native Jira webhook HMAC authentication (backend)

| Field | Value |
|---|---|
| Type | Feature |
| Status | Open |
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

(empty)
