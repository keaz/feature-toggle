# AI-00: TypeSafe config, client, wire types, and `/ai/status`

| Field | Value |
|---|---|
| Type | Feature (foundation) |
| Status | Not started |
| Repo | backend (`feature-toggle/`) |
| Crate | `feature-toggle-backend` |
| Depends on | — |
| Behavior change | No. Without `TYPESAFE_API_KEY` nothing new runs. One log level changes from warn to error. |
| Design | [design.md §3.1–3.3](../design.md#3-architecture) |

## Goal

Add a typed, testable client for `POST https://api.typesafe.ai/v1/systemone` and the configuration that turns it on. Later tasks depend on the `JudgmentClient` trait, not on HTTP.

## Current code (verify first)

| What | Where |
|---|---|
| `struct Config` and its `Default` | `feature-toggle-backend/src/config.rs` (`pub struct Config`, `impl Default for Config`) |
| `Config::load()`: on parse error it logs `warn!` and falls back to all defaults | `config.rs`, the `Err(e) => warn!("Failed to parse TOML ...")` arm |
| `mod config;` is private | `src/lib.rs` |
| App wiring and `web::Data` registration | `src/lib.rs::run` (`.app_data(web::Data::new(...))` block) |
| `reqwest` 0.12 with `json`, `rustls-tls` | `feature-toggle-backend/Cargo.toml` |
| `mockall`, `async-trait` usage patterns | `src/database/approval.rs` (`#[automock] #[async_trait] pub trait ApprovalRepository`) |
| REST module and OpenAPI registration | `src/rest/mod.rs` (`ApiDoc`, `paths(...)`, `components(schemas(...))`) |

## Changes

1. **Config** (`src/config.rs`)
   - Add `TypesafeConfig { base_url: String, model: String, timeout_ms: u64, max_in_flight: usize }`.
     - Defaults: `https://api.typesafe.ai`, `jev-1.13.0`, `3000`, `16`.
     - Derive `Deserialize` with `#[serde(default)]`.
   - Add `#[serde(default)] pub typesafe: TypesafeConfig` to `Config` and to `impl Default`.
   - Change the parse-error arm in `Config::load()` from `warn!` to `error!`. Keep the message, the error text, and the fallback behavior.
   - Add the commented example section to `feature-toggle-backend/config.toml`, with no key.
2. **Module** `src/judgment/` (add `pub mod judgment;` in `lib.rs`)
   - `types.rs`: serde types for the wire format in design §3.3.
     - `SystemOneRequest { state: Value, model: String, questions: BTreeMap<String, Question> }`.
     - `Question` is an enum tagged by `type`: `Noul { instructions: Value, criteria: Option<NoulCriteria> }`, `Choice { instructions: Value, criteria: BTreeMap<String, Option<Value>> }`, `Score { instructions: Value, criteria: Vec<Value> }`.
     - Answers: `NoulAnswer { noul: f64 }`, `ChoiceAnswer { choice, probabilities, confidence }`, `ScoreAnswer { score, legend, probabilities, confidence }`, enum `Answer` tagged by `type`.
     - `SystemOneResponse { model, answers: BTreeMap<String, Answer>, usage: Usage }`.
     - Helpers: `Answers::noul(id) -> Option<f64>`, `choice(id)`, `score(id)`.
     - Builder helpers `noul(...)`, `choice(...)`, `score(...)` so later tasks write questions concisely.
     - Validate in a constructor: Choice has at most 255 options, Score has 2 to 10 levels.
   - `client.rs`
     - `#[automock] #[async_trait] pub trait JudgmentClient: Send + Sync { async fn evaluate(&self, state: Value, questions: BTreeMap<String, Question>) -> Result<SystemOneResponse, JudgmentError>; fn model(&self) -> &str; }`.
     - `HttpJudgmentClient` holds a `reqwest::Client` (timeout from config), the key, base URL, model, and an `Arc<Semaphore>`.
     - Retries: at most 2, only on 429, 529, and connect/timeout errors. Backoff starts at 250 ms and doubles; honor `retry-after` seconds, capped at 5 s.
     - Map 401/403/422/other statuses to `JudgmentError::Http(status, first 500 bytes of body)`.
     - Log one `info!` per call: question count, model, `input_tokens`, latency in ms. Never log state, questions, or the key.
   - `JudgmentError` (`thiserror` if the crate already uses it, otherwise a manual `Display`): `Unavailable`, `Timeout`, `RateLimited`, `Http(u16, String)`, `Decode(String)`.
   - `mod.rs`: `pub fn build_client(cfg: &TypesafeConfig) -> Option<Arc<dyn JudgmentClient>>`. It reads `std::env::var("TYPESAFE_API_KEY")`, treats an empty value as missing, and returns `None` when the key is missing.
3. **Wiring** (`lib.rs::run`)
   - Build `Option<Arc<dyn JudgmentClient>>` once.
   - Register it as `web::Data<AiRuntime>`, with `struct AiRuntime { client: Option<Arc<dyn JudgmentClient>>, model: String }`.
   - Log once at startup: "TypeSafe judgments enabled (model …)" or "disabled (no TYPESAFE_API_KEY)".
4. **Endpoint** `src/rest/ai.rs`
   - `GET /api/v1/ai/status` returns `{ "available": bool, "model": string | null }`.
   - Use the same auth as other authenticated routes.
   - Register it in `ApiDoc` and update the contract baseline (see the README rules).

## Tests

- `types.rs`: serialize a request with one question of each type and compare to a JSON literal. Deserialize the three example answers from design §3.3.
- `client.rs`: test the retry decision and backoff as pure functions (`fn should_retry(status) -> bool`, `fn backoff(attempt, retry_after) -> Duration`). Do not add an HTTP mock crate.
- `config.rs`: TOML with and without `[typesafe]` parses; defaults are as listed.
- `mod.rs`: `build_client` returns `None` when the key is unset or empty. Use `serial_test` because env vars are process-global.
- `#[ignore]` live smoke test, `tests/typesafe_live_test.rs`: runs only when `TYPESAFE_API_KEY` is set. Send one Noul ("Does this convey urgency?" on "Help! My payouts have been failing for 3 days.") and assert `noul > 0.5`.

## Acceptance criteria

- [ ] With no key, the server starts, logs "disabled", and `GET /api/v1/ai/status` returns `{"available": false, "model": null}`.
- [ ] With a key, `/ai/status` returns `available: true` and model `jev-1.13.0`.
- [ ] The live smoke test passes with a real key.
- [ ] `MockJudgmentClient` is usable from other modules' tests.
- [ ] The contract compatibility check passes after the baseline update.
- [ ] No key appears in any log or committed file.

## Out of scope

Database tables, team settings, and any feature-specific question (AI-01 and later).

## Handoff log

_No entries yet._ Format: `### YYYY-MM-DD, <agent or person>`, then what changed, what you verified, open questions, and next step.
