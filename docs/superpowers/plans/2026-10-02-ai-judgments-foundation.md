# AI Judgments Foundation (AI-00, AI-01, AI-02) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the TypeSafe Jev foundation to FluxGate: a typed HTTP client and config, a persisted async judgment pipeline with retries, per-team AI toggles with a REST API, and a UI settings page plus a `useAiFeatures` hook. No user-visible AI feature yet.

**Architecture:** Backend gets a new `judgment` module (`types`, `client`, `service`) behind a `JudgmentClient` trait, a `database::ai` module with two repositories, a 60 s retry scheduler, and `rest::ai` with three endpoints. The subsystem exists only when env `TYPESAFE_API_KEY` is set; the settings API always works and reports `available`. The UI gets `api/ai.ts`, a shared-query hook, and an admin page at `/settings/ai`.

**Tech Stack:** Rust 2024, Actix-web 4, sqlx 0.8 (runtime queries only), reqwest 0.12 (rustls), tokio, mockall 0.14, utoipa 5, serial_test. UI: React 18, TypeScript, Vite, Vitest + Testing Library, Tailwind token classes, shadcn `Switch`, `sonner`, pnpm.

**Spec:** `feature-toggle/docs/ai-judgments/design.md` (approved 2026-10-01), with task files `docs/ai-judgments/tasks/AI-00-*.md`, `AI-01-*.md`, `AI-02-*.md`. Read design §1–4, §6, §8 and the three task files before starting.

## Global Constraints

- The API key comes only from env `TYPESAFE_API_KEY`. Never put it in a file, a log line, a test fixture, a commit, or an HTTP response. An empty or whitespace-only value counts as missing.
- `[typesafe]` defaults: `base_url = "https://api.typesafe.ai"`, `model = "jev-1.13.0"` (pinned; never `jev-latest`), `timeout_ms = 3000`, `max_in_flight = 16`.
- Endpoint: `POST {base_url}/v1/systemone`, header `Authorization: Bearer <key>`, body `{state, model, questions}`.
- Retries: at most 2 per call, only on HTTP 429, 529, connect errors, and timeouts. Backoff starts at 250 ms and doubles; honor `retry-after` seconds; cap 5 s. No retry on 401, 403, 422, or other statuses.
- Logging: one `info!` per call with question count, model, `input_tokens`, latency ms. Never log state, questions, reasons, response bodies, or the key.
- Limits enforced in question constructors: Choice 2 to 255 options, Score 2 to 10 levels.
- Use runtime `sqlx::query(...)` / `sqlx::query_as::<_, T>(...)` only. No `query!` macros, so no `.sqlx/` changes.
- New migration timestamp must sort after `20261002020000_users_admin_source.sql`. Use `20261002030000_ai_judgments.sql`. Never edit an applied migration.
- REST DTOs use `#[serde(rename_all = "camelCase")]` and derive `ToSchema`. Register every new path, schema, and the `AI` tag in `ApiDoc` (`rest/mod.rs`), then update the contract baseline.
- Do not add an HTTP mock crate.
- Edition 2024: `std::env::set_var` and `remove_var` are `unsafe`. Tests that touch env vars use `#[serial]` and restore the original value.
- UI work uses `pnpm`, never `npm`. Use design-token classes (for example `bg-muted`, `text-muted-foreground`, `border-border`, `bg-warning/10`); raw Tailwind palette classes and hex colors fail `designTokenGuard.test.ts`.
- Git: commit directly on `main` in each repo. Stage files by explicit path. Never stage unrelated changes that already exist: backend `.DS_Store`, `feature-edge-server/config.toml`; UI `src/pages/FeatureDetail.tsx`. Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- After code changes in a repo, run `graphify update .` in that repo root before the final commit of the task group.
- Paths: backend repo root is `feature-toggle/` (run cargo there); crate is `feature-toggle/feature-toggle-backend/`. UI repo root is `feature-toggle-ui/` (sibling of `feature-toggle/`).

## Review Focus

1. **Key set to an empty or whitespace string** (common in `.env` files): the server must treat the subsystem as off, log "disabled", and `/ai/status` returns `available: false`. Test: `normalize_api_key` cases in Task 3.
2. **`timeout_ms = 0` or `max_in_flight = 0` in TOML**: a zero-permit semaphore would hang every call forever; a zero timeout fails every call. Both fall back to defaults with a warning. Test: `typesafe_zero_values_use_defaults` in Task 1.
3. **Upstream error bodies echoing user text**: a 422 body can quote the request. The body snippet may go to the DB `error` column but never to a log line. Test: `log_label_never_contains_body` in Task 3.
4. **PUT settings for a team id that does not exist**: must return 404, not 500. Test: repository `upsert_unknown_team_is_not_found` in Task 5 and REST `put_unknown_team_returns_404` in Task 7.
5. **Settings page when the GET fails** (network or 500): must show a load error, not the misleading "not configured" message, and keep switches disabled. Test: `shows a load error instead of the not-configured message` in Task 9.

---

## File Structure

Backend (`feature-toggle/feature-toggle-backend/`):

| File | Action | Responsibility |
|---|---|---|
| `src/config.rs` | Modify | `TypesafeConfig`, `Config.typesafe`, parse-error log level |
| `config.toml` | Modify | Commented `[typesafe]` example |
| `src/judgment/mod.rs` | Create | Module root, `JudgmentKind`, `SubjectType`, key resolution, `build_client`, `AiRuntime` |
| `src/judgment/types.rs` | Create | Wire types, question builders, `Answers` helpers, `RequestParts` |
| `src/judgment/client.rs` | Create | `JudgmentClient` trait (automock), `HttpJudgmentClient`, retry helpers, `JudgmentError` |
| `src/judgment/service.rs` | Create | `JudgmentHandler`, `JudgmentService`, `input_hash`, retry tick |
| `src/database/ai.rs` | Create | `TeamAiSettingsRepository`, `AiJudgmentRepository`, row types |
| `src/database/mod.rs` | Modify | `pub mod ai;` |
| `src/scheduler/ai_judgment_retry.rs` | Create | 60 s retry sweep |
| `src/scheduler/mod.rs` | Modify | Export the scheduler |
| `src/rest/ai.rs` | Create | `/ai/status`, `/teams/{team_id}/ai-settings` GET and PUT |
| `src/rest/mod.rs` | Modify | `pub mod ai;`, `ApiDoc`, `configure` |
| `src/utils/activity_logger.rs` | Modify | `AI_SETTINGS_UPDATED` constant |
| `src/lib.rs` | Modify | `pub mod judgment;`, wiring |
| `migrations/20261002030000_ai_judgments.sql` | Create | Two tables |
| `tests/typesafe_live_test.rs` | Create | `#[ignore]` live smoke test |
| `tests/database/ai_test.rs` | Create | Repository tests against Postgres |
| `tests/database/mod.rs` | Modify | `mod ai_test;` |
| `contracts/baseline/contract-hashes.json` | Modify | New contract hashes |

UI (`feature-toggle-ui/src/`):

| File | Action | Responsibility |
|---|---|---|
| `api/ai.ts` | Create | Types and three API calls |
| `hooks/useAiFeatures.ts` | Create | Cached per-team feature flags |
| `hooks/useAiFeatures.test.tsx` | Create | Hook tests |
| `pages/AiSettingsPage.tsx` | Create | Admin page |
| `pages/__tests__/AiSettingsPage.test.tsx` | Create | Page tests |
| `layout/navConfig.ts` | Modify | Settings item "AI assistance" |
| `layout/__tests__/navConfig.test.ts` | Modify | Admin-only assertion |
| `routes/AppShellRoutes.tsx` | Modify | Route `/settings/ai` |
| `routes/routeModules.ts` | Modify | Lazy loader and prefetch map |

Docs (`feature-toggle/docs/ai-judgments/`): task Status rows, Handoff logs, README task table.

### Deliberate deviations from the task files (record them in the handoff logs)

- `JudgmentClient::model()` returns `String`, not `&str` (mockall cannot return a borrowed `str` cleanly).
- `JudgmentError` gains `Transport(String)` for connection failures that are not timeouts.
- `TypesafeConfig` is sanitized: zero `timeout_ms`/`max_in_flight` and an empty `base_url` fall back to defaults; trailing `/` is trimmed.
- Choice requires at least 2 options (the task only states the 255 maximum).
- `AiRuntime` gains `judgments: Option<Arc<JudgmentService>>` in AI-01 so later tasks reach the service through `web::Data<AiRuntime>`.
- Repository `mark_failed` does not overwrite a `done` row (`AND status <> 'done'`).
- `team_ai_settings.updated_by` has no foreign key, matching design §4.1, so system-client actors never fail the write.
- Settings PUT for an unknown team returns 404.

---

## Part A — Backend AI-00 (repo `feature-toggle/`)

### Task 1: `TypesafeConfig` and config parse-error log level

**Files:**
- Modify: `feature-toggle-backend/src/config.rs`
- Modify: `feature-toggle-backend/config.toml`

**Interfaces:**
- Produces: `crate::config::TypesafeConfig { base_url: String, model: String, timeout_ms: u64, max_in_flight: usize }`, `TypesafeConfig::default()`, `TypesafeConfig::sanitized(self) -> Self`, field `Config.typesafe: TypesafeConfig`, constants `DEFAULT_TYPESAFE_BASE_URL`, `DEFAULT_TYPESAFE_MODEL`.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `src/config.rs`:

```rust
    #[test]
    fn missing_typesafe_section_uses_defaults() {
        let cfg = Config::from_toml(BASE).unwrap();
        assert_eq!(cfg.typesafe, TypesafeConfig::default());
        assert_eq!(cfg.typesafe.base_url, "https://api.typesafe.ai");
        assert_eq!(cfg.typesafe.model, "jev-1.13.0");
        assert_eq!(cfg.typesafe.timeout_ms, 3000);
        assert_eq!(cfg.typesafe.max_in_flight, 16);
    }

    #[test]
    fn typesafe_section_overrides_defaults_per_key() {
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[typesafe]\nbase_url = \"https://example.test/\"\ntimeout_ms = 1500\n"
        ))
        .unwrap();
        assert_eq!(cfg.typesafe.base_url, "https://example.test");
        assert_eq!(cfg.typesafe.timeout_ms, 1500);
        assert_eq!(cfg.typesafe.model, "jev-1.13.0");
        assert_eq!(cfg.typesafe.max_in_flight, 16);
    }

    #[test]
    fn typesafe_zero_values_use_defaults() {
        let cfg = Config::from_toml(&format!(
            "{BASE}\n[typesafe]\nbase_url = \"  \"\ntimeout_ms = 0\nmax_in_flight = 0\n"
        ))
        .unwrap();
        assert_eq!(cfg.typesafe, TypesafeConfig::default());
    }

    #[test]
    fn shipped_config_file_has_default_typesafe_values() {
        let content = include_str!("../config.toml");
        let cfg = Config::from_toml(content).unwrap();
        assert_eq!(cfg.typesafe, TypesafeConfig::default());
    }
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run (from `feature-toggle/`): `cargo test -p feature-toggle-backend --lib config::tests`
Expected: compile error, `cannot find type TypesafeConfig`.

- [ ] **Step 3: Implement** — in `src/config.rs`:

Change the log import to `use log::{error, info, warn};`.

Add the field to `Config`, after `public_base_url`:

```rust
    /// TypeSafe Jev judgments. The subsystem is on only when env
    /// `TYPESAFE_API_KEY` is set; the key is never read from this file.
    #[serde(default)]
    pub typesafe: TypesafeConfig,
```

Add after the `impl AuthConfig` block:

```rust
pub const DEFAULT_TYPESAFE_BASE_URL: &str = "https://api.typesafe.ai";
/// Pinned on purpose: aliases such as `jev-latest` move and change answers.
pub const DEFAULT_TYPESAFE_MODEL: &str = "jev-1.13.0";
const DEFAULT_TYPESAFE_TIMEOUT_MS: u64 = 3000;
const DEFAULT_TYPESAFE_MAX_IN_FLIGHT: usize = 16;

/// TypeSafe System One client settings (`[typesafe]` section).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TypesafeConfig {
    pub base_url: String,
    pub model: String,
    /// Timeout per HTTP attempt.
    pub timeout_ms: u64,
    /// Concurrent requests allowed across all callers.
    pub max_in_flight: usize,
}

impl Default for TypesafeConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_TYPESAFE_BASE_URL.to_string(),
            model: DEFAULT_TYPESAFE_MODEL.to_string(),
            timeout_ms: DEFAULT_TYPESAFE_TIMEOUT_MS,
            max_in_flight: DEFAULT_TYPESAFE_MAX_IN_FLIGHT,
        }
    }
}

impl TypesafeConfig {
    /// Trims the base URL and replaces values that would break every call
    /// (empty URL or model, zero timeout, zero permits) with the defaults.
    pub fn sanitized(self) -> Self {
        let mut cfg = self;
        cfg.base_url = cfg.base_url.trim().trim_end_matches('/').to_string();
        if cfg.base_url.is_empty() {
            warn!("typesafe.base_url is empty; using {DEFAULT_TYPESAFE_BASE_URL}");
            cfg.base_url = DEFAULT_TYPESAFE_BASE_URL.to_string();
        }
        cfg.model = cfg.model.trim().to_string();
        if cfg.model.is_empty() {
            warn!("typesafe.model is empty; using {DEFAULT_TYPESAFE_MODEL}");
            cfg.model = DEFAULT_TYPESAFE_MODEL.to_string();
        }
        if cfg.timeout_ms == 0 {
            warn!("typesafe.timeout_ms = 0; using {DEFAULT_TYPESAFE_TIMEOUT_MS}");
            cfg.timeout_ms = DEFAULT_TYPESAFE_TIMEOUT_MS;
        }
        if cfg.max_in_flight == 0 {
            warn!("typesafe.max_in_flight = 0; using {DEFAULT_TYPESAFE_MAX_IN_FLIGHT}");
            cfg.max_in_flight = DEFAULT_TYPESAFE_MAX_IN_FLIGHT;
        }
        cfg
    }
}
```

In `impl Default for Config`, add `typesafe: TypesafeConfig::default(),` after `public_base_url: None,`.

In `Config::from_toml`, after `cfg.auth = cfg.auth.sanitized();` add `cfg.typesafe = cfg.typesafe.sanitized();`.

In `Config::load`, change only the parse-error arm from `warn!(` to `error!(`; keep the message text and arguments exactly:

```rust
                        Err(e) => {
                            error!(
                                "Failed to parse TOML configuration at {}: {}. Falling back to defaults.",
                                path_str, e
                            );
                        }
```

Append to `feature-toggle-backend/config.toml`:

```toml

# TypeSafe AI judgments (optional section; these are the defaults).
# The subsystem turns on only when env TYPESAFE_API_KEY is set. Never put the key
# in this file. Keep the model pinned to a versioned id: aliases such as
# jev-latest move and change answers. A value of 0 for timeout_ms or
# max_in_flight falls back to the default.
#[typesafe]
#base_url = "https://api.typesafe.ai"
#model = "jev-1.13.0"
#timeout_ms = 3000          # per HTTP attempt
#max_in_flight = 16         # concurrent requests across the server
```

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --lib config::tests`
Expected: all `config::tests` pass, including the 5 existing ones.

- [ ] **Step 5: Commit**

```bash
git add feature-toggle-backend/src/config.rs feature-toggle-backend/config.toml
git commit -m "feat(ai): add [typesafe] config section and log TOML parse errors as errors

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: Wire types and question builders

**Files:**
- Create: `feature-toggle-backend/src/judgment/mod.rs`
- Create: `feature-toggle-backend/src/judgment/types.rs`
- Modify: `feature-toggle-backend/src/lib.rs` (add `pub mod judgment;`)

**Interfaces:**
- Produces (in `crate::judgment::types`): `Question` enum (`Noul`, `Choice`, `Score`), `NoulCriteria`, `QuestionError`, `Question::noul(instructions) -> Question`, `Question::noul_with(instructions, when_true, when_false) -> Question`, `Question::choice(instructions, options) -> Result<Question, QuestionError>`, `Question::score(instructions, levels) -> Result<Question, QuestionError>`, `SystemOneRequest { state, model, questions }`, `RequestParts { state: Value, questions: BTreeMap<String, Question> }`, `Answer` enum, `ChoiceAnswer`, `ScoreAnswer`, `Answers(pub BTreeMap<String, Answer>)` with `noul(&str) -> Option<f64>`, `choice(&str) -> Option<&ChoiceAnswer>`, `score(&str) -> Option<&ScoreAnswer>`, `Usage { input_tokens: u32, output_tokens: u32 }`, `SystemOneResponse { model: String, answers: Answers, usage: Usage }`.

- [ ] **Step 1: Create the module root and register it**

`src/judgment/mod.rs`:

```rust
//! TypeSafe Jev judgments: typed questions about application state, answered
//! with probabilities. See `docs/ai-judgments/design.md`.

pub mod types;
```

In `src/lib.rs`, add `pub mod judgment;` after `pub mod grpc;`.

- [ ] **Step 2: Write the failing tests** — create `src/judgment/types.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_each_question_type() {
        let mut questions = BTreeMap::new();
        questions.insert(
            "is_urgent".to_string(),
            Question::noul_with("Does this convey urgency?", "Time-sensitive", "No urgency"),
        );
        questions.insert(
            "department".to_string(),
            Question::choice(
                "Which team should handle this?",
                [
                    ("billing", Some(json!("Payments, invoicing, refunds"))),
                    ("technical", None),
                ],
            )
            .unwrap(),
        );
        questions.insert(
            "frustration".to_string(),
            Question::score("How frustrated is the customer?", ["Calm", "Frustrated", "Very angry"])
                .unwrap(),
        );
        questions.insert("plain".to_string(), Question::noul("Is it plain?"));
        let request = SystemOneRequest {
            state: json!("Help! My payouts have been failing for 3 days."),
            model: "jev-1.13.0".to_string(),
            questions,
        };

        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({
                "state": "Help! My payouts have been failing for 3 days.",
                "model": "jev-1.13.0",
                "questions": {
                    "is_urgent": {
                        "type": "noul",
                        "instructions": "Does this convey urgency?",
                        "criteria": { "true": "Time-sensitive", "false": "No urgency" }
                    },
                    "department": {
                        "type": "choice",
                        "instructions": "Which team should handle this?",
                        "criteria": { "billing": "Payments, invoicing, refunds", "technical": null }
                    },
                    "frustration": {
                        "type": "score",
                        "instructions": "How frustrated is the customer?",
                        "criteria": ["Calm", "Frustrated", "Very angry"]
                    },
                    "plain": { "type": "noul", "instructions": "Is it plain?" }
                }
            })
        );
    }

    #[test]
    fn response_deserializes_design_examples() {
        let response: SystemOneResponse = serde_json::from_value(json!({
            "model": "jev-1.13.0",
            "answers": {
                "a": { "type": "noul", "noul": 0.95 },
                "b": { "type": "choice", "choice": "opt_a",
                       "probabilities": { "opt_a": 0.9, "opt_b": 0.1 }, "confidence": 0.8 },
                "c": { "type": "score", "score": 1.4, "legend": { "0": "low" },
                       "probabilities": { "0": 0.1 }, "confidence": 0.7 }
            },
            "usage": { "input_tokens": 300, "output_tokens": 20 }
        }))
        .unwrap();

        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.usage.input_tokens, 300);
        assert_eq!(response.answers.noul("a"), Some(0.95));
        let choice = response.answers.choice("b").unwrap();
        assert_eq!(choice.choice, "opt_a");
        assert_eq!(choice.probabilities["opt_b"], 0.1);
        assert_eq!(choice.confidence, 0.8);
        let score = response.answers.score("c").unwrap();
        assert_eq!(score.score, 1.4);
        assert_eq!(score.confidence, 0.7);
        assert_eq!(response.answers.noul("b"), None);
        assert_eq!(response.answers.noul("missing"), None);
    }

    #[test]
    fn choice_option_limits_are_enforced() {
        assert_eq!(
            Question::choice("q", [("only", None)]).unwrap_err(),
            QuestionError::ChoiceOptions(1)
        );
        let many: Vec<(String, Option<serde_json::Value>)> =
            (0..256).map(|i| (format!("o{i}"), None)).collect();
        assert_eq!(
            Question::choice("q", many).unwrap_err(),
            QuestionError::ChoiceOptions(256)
        );
        let max: Vec<(String, Option<serde_json::Value>)> =
            (0..255).map(|i| (format!("o{i}"), None)).collect();
        assert!(Question::choice("q", max).is_ok());
    }

    #[test]
    fn score_level_limits_are_enforced() {
        assert_eq!(
            Question::score("q", ["one"]).unwrap_err(),
            QuestionError::ScoreLevels(1)
        );
        let eleven: Vec<String> = (0..11).map(|i| format!("level {i}")).collect();
        assert_eq!(
            Question::score("q", eleven).unwrap_err(),
            QuestionError::ScoreLevels(11)
        );
        assert!(Question::score("q", ["a", "b"]).is_ok());
    }

    #[test]
    fn instructions_may_be_structured_objects() {
        let question = Question::noul(json!({
            "candidate_tag": "payments",
            "question": "Does the tag `candidate_tag` describe `feature`?"
        }));
        let value = serde_json::to_value(&question).unwrap();
        assert_eq!(value["instructions"]["candidate_tag"], "payments");
    }
}
```

- [ ] **Step 3: Run the tests and confirm they fail**

Run: `cargo test -p feature-toggle-backend --lib judgment::types`
Expected: compile errors, `cannot find type SystemOneRequest` (and others).

- [ ] **Step 4: Implement** — prepend to `src/judgment/types.rs` (above the test module):

```rust
//! Wire format of `POST /v1/systemone` (https://docs.typesafe.ai/api.md).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MIN_CHOICE_OPTIONS: usize = 2;
pub const MAX_CHOICE_OPTIONS: usize = 255;
pub const MIN_SCORE_LEVELS: usize = 2;
pub const MAX_SCORE_LEVELS: usize = 10;

/// What a yes and a no mean for a Noul question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub when_true: Value,
    #[serde(rename = "false")]
    pub when_false: Value,
}

/// One typed question. Build it with the constructors so limits are checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Option<Value>>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuestionError {
    #[error("a choice needs {MIN_CHOICE_OPTIONS} to {MAX_CHOICE_OPTIONS} options, got {0}")]
    ChoiceOptions(usize),
    #[error("a score needs {MIN_SCORE_LEVELS} to {MAX_SCORE_LEVELS} levels, got {0}")]
    ScoreLevels(usize),
}

impl Question {
    /// A yes/no question without criteria.
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    /// A yes/no question with descriptions of what yes and no mean.
    pub fn noul_with(
        instructions: impl Into<Value>,
        when_true: impl Into<Value>,
        when_false: impl Into<Value>,
    ) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: Some(NoulCriteria {
                when_true: when_true.into(),
                when_false: when_false.into(),
            }),
        }
    }

    /// Picks one option. `None` means the option needs no description.
    pub fn choice<I, K>(instructions: impl Into<Value>, options: I) -> Result<Self, QuestionError>
    where
        I: IntoIterator<Item = (K, Option<Value>)>,
        K: Into<String>,
    {
        let criteria: BTreeMap<String, Option<Value>> = options
            .into_iter()
            .map(|(key, description)| (key.into(), description))
            .collect();
        if !(MIN_CHOICE_OPTIONS..=MAX_CHOICE_OPTIONS).contains(&criteria.len()) {
            return Err(QuestionError::ChoiceOptions(criteria.len()));
        }
        Ok(Question::Choice {
            instructions: instructions.into(),
            criteria,
        })
    }

    /// Rates along ordered levels, lowest first.
    pub fn score<I, L>(instructions: impl Into<Value>, levels: I) -> Result<Self, QuestionError>
    where
        I: IntoIterator<Item = L>,
        L: Into<Value>,
    {
        let criteria: Vec<Value> = levels.into_iter().map(Into::into).collect();
        if !(MIN_SCORE_LEVELS..=MAX_SCORE_LEVELS).contains(&criteria.len()) {
            return Err(QuestionError::ScoreLevels(criteria.len()));
        }
        Ok(Question::Score {
            instructions: instructions.into(),
            criteria,
        })
    }
}

/// Request body. The model comes from config, so callers build `RequestParts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemOneRequest {
    pub state: Value,
    pub model: String,
    pub questions: BTreeMap<String, Question>,
}

/// State plus questions, built by a judgment handler.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestParts {
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    pub choice: String,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreAnswer {
    pub score: f64,
    #[serde(default)]
    pub legend: BTreeMap<String, Value>,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul { noul: f64 },
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

/// Answers keyed by question id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Answers(pub BTreeMap<String, Answer>);

impl Answers {
    pub fn noul(&self, id: &str) -> Option<f64> {
        match self.0.get(id)? {
            Answer::Noul { noul } => Some(*noul),
            _ => None,
        }
    }

    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        match self.0.get(id)? {
            Answer::Choice(answer) => Some(answer),
            _ => None,
        }
    }

    pub fn score(&self, id: &str) -> Option<&ScoreAnswer> {
        match self.0.get(id)? {
            Answer::Score(answer) => Some(answer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: Answers,
    #[serde(default)]
    pub usage: Usage,
}
```

- [ ] **Step 5: Run the tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --lib judgment::types`
Expected: 5 passed.

- [ ] **Step 6: Commit**

```bash
git add feature-toggle-backend/src/judgment/mod.rs feature-toggle-backend/src/judgment/types.rs feature-toggle-backend/src/lib.rs
git commit -m "feat(ai): add TypeSafe System One wire types and question builders

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 3: HTTP client, key resolution, `AiRuntime`, live smoke test

**Files:**
- Create: `feature-toggle-backend/src/judgment/client.rs`
- Modify: `feature-toggle-backend/src/judgment/mod.rs`
- Create: `feature-toggle-backend/tests/typesafe_live_test.rs`

**Interfaces:**
- Consumes: `crate::config::TypesafeConfig` (Task 1); `types::{Question, SystemOneRequest, SystemOneResponse}` (Task 2).
- Produces (in `crate::judgment::client`): `JudgmentError` (`Unavailable`, `Timeout`, `RateLimited`, `Transport(String)`, `Http(u16, String)`, `Decode(String)`) with `log_label(&self) -> String`; trait `JudgmentClient { async fn evaluate(&self, state: Value, questions: BTreeMap<String, Question>) -> Result<SystemOneResponse, JudgmentError>; fn model(&self) -> String; }` and `MockJudgmentClient` (unconditional `#[automock]`, so integration tests can use it); `HttpJudgmentClient::new(&TypesafeConfig, String) -> Result<Self, JudgmentError>`; pure helpers `should_retry_status(u16) -> bool`, `backoff(u32, Option<Duration>) -> Duration`, `parse_retry_after(Option<&str>) -> Option<Duration>`, `body_snippet(&str) -> String`.
- Produces (in `crate::judgment`): `API_KEY_ENV`, `normalize_api_key(Option<String>) -> Option<String>`, `api_key_from_env() -> Option<String>`, `build_client(&TypesafeConfig) -> Option<Arc<dyn JudgmentClient>>`, `AiRuntime { client: Option<Arc<dyn JudgmentClient>>, model: String }` with `AiRuntime::new(client, model)`, `available()`, `status_model() -> Option<String>`; re-exports `JudgmentClient`, `JudgmentError`, `HttpJudgmentClient`.

- [ ] **Step 1: Write the failing tests** — create `src/judgment/client.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_only_rate_limit_and_overload() {
        assert!(should_retry_status(429));
        assert!(should_retry_status(529));
        for status in [200, 400, 401, 403, 404, 422, 500, 502, 503] {
            assert!(!should_retry_status(status), "status {status}");
        }
    }

    #[test]
    fn backoff_doubles_from_250ms_and_caps_at_5s() {
        assert_eq!(backoff(0, None), Duration::from_millis(250));
        assert_eq!(backoff(1, None), Duration::from_millis(500));
        assert_eq!(backoff(2, None), Duration::from_millis(1000));
        assert_eq!(backoff(10, None), Duration::from_secs(5));
        assert_eq!(backoff(40, None), Duration::from_secs(5));
    }

    #[test]
    fn backoff_honors_retry_after_with_cap() {
        assert_eq!(backoff(0, Some(Duration::from_secs(2))), Duration::from_secs(2));
        assert_eq!(backoff(0, Some(Duration::from_secs(60))), Duration::from_secs(5));
    }

    #[test]
    fn parses_retry_after_seconds_only() {
        assert_eq!(parse_retry_after(Some("3")), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_after(Some(" 1 ")), Some(Duration::from_secs(1)));
        assert_eq!(parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(parse_retry_after(None), None);
    }

    #[test]
    fn body_snippet_cuts_at_500_bytes_on_a_char_boundary() {
        assert_eq!(body_snippet("short"), "short");
        let long = "a".repeat(499) + "é" + &"b".repeat(100);
        let snippet = body_snippet(&long);
        assert_eq!(snippet.len(), 499);
        assert!(snippet.chars().all(|c| c == 'a'));
    }

    #[test]
    fn log_label_never_contains_body() {
        let error = JudgmentError::Http(422, "state.reason: my secret text".to_string());
        assert_eq!(error.log_label(), "http_422");
        assert!(error.to_string().contains("my secret text"));
        assert_eq!(JudgmentError::Timeout.log_label(), "timeout");
        assert_eq!(JudgmentError::Transport("x".into()).log_label(), "transport");
    }

    #[test]
    fn new_client_targets_systemone_endpoint() {
        let cfg = TypesafeConfig {
            base_url: "https://example.test".into(),
            ..TypesafeConfig::default()
        };
        let client = HttpJudgmentClient::new(&cfg, "key".into()).unwrap();
        assert_eq!(client.endpoint, "https://example.test/v1/systemone");
        assert_eq!(client.model(), "jev-1.13.0");
    }
}
```

In `src/judgment/mod.rs`, add `pub mod client;` above `pub mod types;`, then append this test module at the bottom of the file:

```rust
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
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test -p feature-toggle-backend --lib judgment::`
Expected: compile errors (`should_retry_status`, `build_client`, `AiRuntime` not found).

- [ ] **Step 3: Implement the client** — prepend to `src/judgment/client.rs`:

```rust
//! HTTP client for TypeSafe System One. Callers depend on `JudgmentClient`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::{info, warn};
use mockall::automock;
use serde_json::Value;
use tokio::sync::Semaphore;

use super::types::{Question, SystemOneRequest, SystemOneResponse};
use crate::config::TypesafeConfig;

/// Retries after the first attempt.
pub const MAX_RETRIES: u32 = 2;
const BASE_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const BODY_SNIPPET_BYTES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JudgmentError {
    #[error("TypeSafe judgments are not configured")]
    Unavailable,
    #[error("TypeSafe request timed out")]
    Timeout,
    #[error("TypeSafe rate limit or overload")]
    RateLimited,
    #[error("TypeSafe connection error: {0}")]
    Transport(String),
    #[error("TypeSafe returned HTTP {0}: {1}")]
    Http(u16, String),
    #[error("Could not decode TypeSafe response: {0}")]
    Decode(String),
}

impl JudgmentError {
    /// A short label safe for log lines: never includes response bodies.
    pub fn log_label(&self) -> String {
        match self {
            JudgmentError::Unavailable => "unavailable".to_string(),
            JudgmentError::Timeout => "timeout".to_string(),
            JudgmentError::RateLimited => "rate_limited".to_string(),
            JudgmentError::Transport(_) => "transport".to_string(),
            JudgmentError::Http(status, _) => format!("http_{status}"),
            JudgmentError::Decode(_) => "decode".to_string(),
        }
    }
}

#[automock]
#[async_trait]
pub trait JudgmentClient: Send + Sync {
    async fn evaluate(
        &self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, JudgmentError>;

    /// The configured model id sent with every request.
    fn model(&self) -> String;
}

pub fn should_retry_status(status: u16) -> bool {
    matches!(status, 429 | 529)
}

/// Delay before retry number `attempt + 1`.
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exponential = BASE_BACKOFF.saturating_mul(1u32 << attempt.min(16));
    retry_after.unwrap_or(exponential).min(MAX_BACKOFF)
}

/// `retry-after` in whole seconds. HTTP-date values are ignored.
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    value?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

pub fn body_snippet(body: &str) -> String {
    if body.len() <= BODY_SNIPPET_BYTES {
        return body.to_string();
    }
    let mut end = BODY_SNIPPET_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_string()
}

enum AttemptError {
    Retryable(JudgmentError, Option<Duration>),
    Fatal(JudgmentError),
}

fn classify_transport_error(error: reqwest::Error) -> AttemptError {
    if error.is_timeout() {
        AttemptError::Retryable(JudgmentError::Timeout, None)
    } else if error.is_connect() {
        AttemptError::Retryable(
            JudgmentError::Transport(error.without_url().to_string()),
            None,
        )
    } else {
        AttemptError::Fatal(JudgmentError::Transport(error.without_url().to_string()))
    }
}

pub struct HttpJudgmentClient {
    http: reqwest::Client,
    api_key: String,
    pub(crate) endpoint: String,
    model: String,
    permits: Arc<Semaphore>,
}

impl HttpJudgmentClient {
    pub fn new(config: &TypesafeConfig, api_key: String) -> Result<Self, JudgmentError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1)))
            .build()
            .map_err(|e| JudgmentError::Transport(e.without_url().to_string()))?;
        Ok(Self {
            http,
            api_key,
            endpoint: format!("{}/v1/systemone", config.base_url.trim_end_matches('/')),
            model: config.model.clone(),
            permits: Arc::new(Semaphore::new(config.max_in_flight.max(1))),
        })
    }

    async fn send_once(
        &self,
        request: &SystemOneRequest,
    ) -> Result<SystemOneResponse, AttemptError> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(classify_transport_error)?;

        let status = response.status().as_u16();
        if response.status().is_success() {
            return response
                .json::<SystemOneResponse>()
                .await
                .map_err(|e| AttemptError::Fatal(JudgmentError::Decode(e.without_url().to_string())));
        }

        let retry_after = parse_retry_after(
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
        );
        let body = response.text().await.unwrap_or_default();
        if should_retry_status(status) {
            Err(AttemptError::Retryable(JudgmentError::RateLimited, retry_after))
        } else {
            Err(AttemptError::Fatal(JudgmentError::Http(status, body_snippet(&body))))
        }
    }
}

#[async_trait]
impl JudgmentClient for HttpJudgmentClient {
    async fn evaluate(
        &self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, JudgmentError> {
        let question_count = questions.len();
        let request = SystemOneRequest {
            state,
            model: self.model.clone(),
            questions,
        };
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| JudgmentError::Unavailable)?;
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            match self.send_once(&request).await {
                Ok(response) => {
                    info!(
                        "TypeSafe call ok: questions={} model={} input_tokens={} latency_ms={}",
                        question_count,
                        response.model,
                        response.usage.input_tokens,
                        started.elapsed().as_millis()
                    );
                    return Ok(response);
                }
                Err(AttemptError::Retryable(error, retry_after)) if attempt < MAX_RETRIES => {
                    let delay = backoff(attempt, retry_after);
                    warn!(
                        "TypeSafe call failed ({}); retry {} in {} ms",
                        error.log_label(),
                        attempt + 1,
                        delay.as_millis()
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(AttemptError::Retryable(error, _)) | Err(AttemptError::Fatal(error)) => {
                    warn!(
                        "TypeSafe call failed: questions={} model={} latency_ms={} error={}",
                        question_count,
                        self.model,
                        started.elapsed().as_millis(),
                        error.log_label()
                    );
                    return Err(error);
                }
            }
        }
    }

    fn model(&self) -> String {
        self.model.clone()
    }
}
```

- [ ] **Step 4: Implement key resolution and `AiRuntime`** — replace the non-test part of `src/judgment/mod.rs` with:

```rust
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
```

Keep the `#[cfg(test)] mod tests` block from Step 1 at the bottom of the file.

- [ ] **Step 5: Run the unit tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --lib judgment::`
Expected: all `judgment::types`, `judgment::client`, and `judgment::tests` pass.

- [ ] **Step 6: Write the live smoke test** — create `tests/typesafe_live_test.rs`:

```rust
//! Calls the live TypeSafe API. Run with:
//! cargo test -p feature-toggle-backend --test typesafe_live_test -- --ignored

use std::collections::BTreeMap;

use feature_toggle_backend::config::TypesafeConfig;
use feature_toggle_backend::judgment::build_client;
use feature_toggle_backend::judgment::types::Question;
use serde_json::json;

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn live_noul_detects_urgency() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live smoke test");
        return;
    };
    let mut questions = BTreeMap::new();
    questions.insert(
        "is_urgent".to_string(),
        Question::noul("Does this convey urgency?"),
    );

    let response = client
        .evaluate(
            json!("Help! My payouts have been failing for 3 days."),
            questions,
        )
        .await
        .expect("live TypeSafe call failed");

    assert_eq!(response.model, "jev-1.13.0");
    let urgent = response.answers.noul("is_urgent").expect("noul answer");
    assert!(urgent > 0.5, "expected urgency above 0.5, got {urgent}");
}
```

- [ ] **Step 7: Run the live test with the real key**

Run: `cargo test -p feature-toggle-backend --test typesafe_live_test -- --ignored --nocapture`
Expected: `live_noul_detects_urgency ... ok`. The key is already in the shell env; do not echo it.

- [ ] **Step 8: Commit**

```bash
git add feature-toggle-backend/src/judgment/client.rs feature-toggle-backend/src/judgment/mod.rs feature-toggle-backend/tests/typesafe_live_test.rs
git commit -m "feat(ai): add TypeSafe HTTP client with retries, key resolution and AiRuntime

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 4: `GET /api/v1/ai/status`, wiring, contract baseline, AI-00 docs

**Files:**
- Create: `feature-toggle-backend/src/rest/ai.rs`
- Modify: `feature-toggle-backend/src/rest/mod.rs`
- Modify: `feature-toggle-backend/src/lib.rs`
- Modify: `feature-toggle-backend/contracts/baseline/contract-hashes.json` (generated)
- Modify: `docs/ai-judgments/tasks/AI-00-typesafe-client-and-config.md`, `docs/ai-judgments/README.md`

**Interfaces:**
- Consumes: `crate::judgment::{AiRuntime, build_client}` (Task 3).
- Produces: `rest::ai::AiStatusResponse { available: bool, model: Option<String> }`, handler `rest::ai::get_ai_status`, `rest::ai::configure`. `web::Data<AiRuntime>` registered in `lib.rs::run`.

- [ ] **Step 1: Write the failing tests** — create `src/rest/ai.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use actix_web::{App, test, web};
    use serde_json::{Value, json};

    use crate::judgment::client::MockJudgmentClient;
    use crate::judgment::{AiRuntime, JudgmentClient};

    #[actix_web::test]
    async fn status_without_key_is_unavailable() {
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AiRuntime::new(None, "jev-1.13.0")))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get().uri("/api/v1/ai/status").to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body, json!({ "available": false, "model": null }));
    }

    #[actix_web::test]
    async fn status_with_client_reports_model() {
        let client: Arc<dyn JudgmentClient> = Arc::new(MockJudgmentClient::new());
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(AiRuntime::new(Some(client), "jev-1.13.0")))
                .service(web::scope("/api/v1").configure(super::configure)),
        )
        .await;
        let req = test::TestRequest::get().uri("/api/v1/ai/status").to_request();
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(body, json!({ "available": true, "model": "jev-1.13.0" }));
    }
}
```

In `src/rest/mod.rs`, add `pub mod ai;` at the top of the module list (alphabetical, before `pub mod approval;`).

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test -p feature-toggle-backend --lib rest::ai`
Expected: compile error, `cannot find function configure in super`.

- [ ] **Step 3: Implement the handler** — prepend to `src/rest/ai.rs`:

```rust
//! AI judgment endpoints: subsystem status (AI-00).

use actix_web::{HttpResponse, Responder, get, web};
use serde::Serialize;
use utoipa::ToSchema;

use crate::judgment::AiRuntime;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AiStatusResponse {
    /// True when the server has a TypeSafe API key.
    pub available: bool,
    /// Pinned model id; null when unavailable.
    pub model: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ai/status",
    responses(
        (status = 200, description = "AI judgment subsystem status", body = AiStatusResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[get("/ai/status")]
pub(crate) async fn get_ai_status(runtime: web::Data<AiRuntime>) -> impl Responder {
    HttpResponse::Ok().json(AiStatusResponse {
        available: runtime.available(),
        model: runtime.status_model(),
    })
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_ai_status);
}
```

- [ ] **Step 4: Register in `ApiDoc` and routes** — in `src/rest/mod.rs`:
  - Add `use crate::rest::ai::AiStatusResponse;` next to the other `use crate::rest::...` imports.
  - Add `ai::get_ai_status,` as the first entry after `health,` in `paths(...)`.
  - Add `AiStatusResponse,` after `ErrorResponse,` in `components(schemas(...))`.
  - Add `(name = "AI", description = "TypeSafe AI judgments: status and per-team settings"),` to the `tags(...)` list, before `(name = "Operational Safety", ...)`.
  - In `configure`, add `.configure(ai::configure)` after `.configure(operational_safety::configure)`.

- [ ] **Step 5: Wire `AiRuntime` in `lib.rs::run`** — after `let cfg = crate::config::Config::load();` add:

```rust
    // TypeSafe judgments: on only when TYPESAFE_API_KEY is set.
    let ai_client = judgment::build_client(&cfg.typesafe);
    if ai_client.is_some() {
        log::info!("TypeSafe judgments enabled (model {})", cfg.typesafe.model);
    } else {
        log::info!("TypeSafe judgments disabled (no TYPESAFE_API_KEY)");
    }
    let ai_runtime = judgment::AiRuntime::new(ai_client.clone(), cfg.typesafe.model.clone());
```

Inside the `HttpServer::new` closure, add `.app_data(web::Data::new(ai_runtime.clone()))` after `.app_data(web::Data::new(updates_tx.clone()))`.

- [ ] **Step 6: Run the tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --lib rest::ai`
Expected: 2 passed.

- [ ] **Step 7: Update the contract baseline** (from `feature-toggle/`)

```bash
./scripts/export-contracts.sh
cp feature-toggle-backend/contracts/generated/contract-hashes.json feature-toggle-backend/contracts/baseline/contract-hashes.json
./scripts/check-contract-compat.sh
```

Expected: the last command passes. Only `contracts/baseline/contract-hashes.json` is tracked; do not add `contracts/generated/`.

- [ ] **Step 8: Run the AI-00 quality gate**

```bash
cargo fmt
cargo clippy -p feature-toggle-backend --all-targets
cargo test -p feature-toggle-backend --lib
```

Expected: no clippy warnings in new files; all lib tests pass.

- [ ] **Step 9: Commit the code**

```bash
git add feature-toggle-backend/src/rest/ai.rs feature-toggle-backend/src/rest/mod.rs feature-toggle-backend/src/lib.rs feature-toggle-backend/contracts/baseline/contract-hashes.json
git commit -m "feat(ai): expose GET /api/v1/ai/status and wire AiRuntime

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 10: Update AI-00 docs** — in `docs/ai-judgments/tasks/AI-00-typesafe-client-and-config.md` set `| Status | Done in <short sha of the Step 9 commit> |` (get it with `git log -1 --format=%h`), tick the acceptance boxes you verified, and append:

```markdown
### 2026-10-02, Claude (plan 2026-10-02-ai-judgments-foundation)

- Changed: `[typesafe]` config (sanitized), `judgment::{types, client}`, `build_client`, `AiRuntime`, `GET /api/v1/ai/status`, contract baseline. TOML parse errors now log at error level.
- Verified: unit tests for wire types, retry/backoff helpers, key normalization, status endpoint; live smoke test passed against `jev-1.13.0`; contract check passes.
- Deviations: `JudgmentClient::model()` returns `String`; `JudgmentError::Transport` added; zero `timeout_ms`/`max_in_flight` fall back to defaults; Choice needs at least 2 options.
- Next: AI-01 uses `MockJudgmentClient` (unconditional `#[automock]`) and extends `AiRuntime` with `judgments`.
```

In `docs/ai-judgments/README.md`, change the AI-00 row from `| [ ] |` to `| [x] |`.

- [ ] **Step 11: Commit the docs**

```bash
git add docs/ai-judgments/tasks/AI-00-typesafe-client-and-config.md docs/ai-judgments/README.md
git commit -m "docs(ai): mark AI-00 done

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Part B — Backend AI-01 (repo `feature-toggle/`)

### Task 5: Migration and repositories

**Files:**
- Create: `feature-toggle-backend/migrations/20261002030000_ai_judgments.sql`
- Modify: `feature-toggle-backend/src/judgment/mod.rs` (kind enums)
- Create: `feature-toggle-backend/src/database/ai.rs`
- Modify: `feature-toggle-backend/src/database/mod.rs`
- Create: `feature-toggle-backend/tests/database/ai_test.rs`
- Modify: `feature-toggle-backend/tests/database/mod.rs`

**Interfaces:**
- Produces (in `crate::judgment`): `JudgmentKind { ApprovalRisk, Justification, FlagKind }` and `SubjectType { ApprovalRequest, Feature, Activity, FreezeWindow, ScheduledChange }`, each `Copy + Eq + Hash` with `as_str(self) -> &'static str` and `FromStr<Err = String>`.
- Produces (in `crate::database::ai`): `TeamAiSettings { approval_risk, justification_check, flag_kind, nl_search: bool }` (Serialize, Default) with `is_enabled(AiFeature) -> bool`; `AiFeature { ApprovalRisk, JustificationCheck, FlagKind, NlSearch }`; `StoredTeamAiSettings { settings: TeamAiSettings, updated_at: Option<DateTime<Utc>>, updated_by: Option<Uuid> }` (Default); trait `TeamAiSettingsRepository { get(Uuid) -> Result<StoredTeamAiSettings, Error>; upsert(Uuid, TeamAiSettings, Option<Uuid>) -> Result<StoredTeamAiSettings, Error>; clone_box }` + `MockTeamAiSettingsRepository` + `team_ai_settings_repository(PgPool)`; `AiJudgment` row; `NewJudgment { team_id, kind: JudgmentKind, subject_type: SubjectType, subject_id, input: Value, input_hash: String }`; `JudgmentResult { model: String, raw_answers: Value, derived: Value, input_tokens: Option<i32> }`; trait `AiJudgmentRepository { upsert_pending(NewJudgment) -> Result<AiJudgment>; mark_done(Uuid, String, JudgmentResult) -> Result<bool>; mark_failed(Uuid, String, String) -> Result<bool>; get_for_subject(SubjectType, Uuid, JudgmentKind) -> Result<Option<AiJudgment>>; get_for_subjects(SubjectType, Vec<Uuid>, JudgmentKind) -> Result<Vec<AiJudgment>>; list_retryable(i64) -> Result<Vec<AiJudgment>>; clone_box }` + `MockAiJudgmentRepository` + `ai_judgment_repository(PgPool)`; constants `MAX_ATTEMPTS = 3`, `RETRY_BATCH = 50`.

- [ ] **Step 1: Write the migration** — `migrations/20261002030000_ai_judgments.sql`:

```sql
-- TypeSafe AI judgments (docs/ai-judgments/design.md §4.1, §4.2).
-- A missing team_ai_settings row means every AI feature is off for the team.
CREATE TABLE team_ai_settings (
  team_id              UUID PRIMARY KEY REFERENCES teams(id) ON DELETE CASCADE,
  approval_risk        BOOLEAN NOT NULL DEFAULT FALSE,
  justification_check  BOOLEAN NOT NULL DEFAULT FALSE,
  flag_kind            BOOLEAN NOT NULL DEFAULT FALSE,
  nl_search            BOOLEAN NOT NULL DEFAULT FALSE,
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_by           UUID NULL
);

CREATE TABLE ai_judgments (
  id            UUID PRIMARY KEY,
  team_id       UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  subject_type  TEXT NOT NULL CHECK (subject_type IN
                  ('approval_request','feature','activity','freeze_window','scheduled_change')),
  subject_id    UUID NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('approval_risk','justification','flag_kind')),
  status        TEXT NOT NULL CHECK (status IN ('pending','done','failed')),
  attempts      INT  NOT NULL DEFAULT 0,
  input         JSONB NOT NULL,
  input_hash    TEXT NOT NULL,
  model         TEXT NULL,
  raw_answers   JSONB NULL,
  derived       JSONB NULL,
  input_tokens  INT NULL,
  error         TEXT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  completed_at  TIMESTAMPTZ NULL,
  UNIQUE (subject_type, subject_id, kind)
);

CREATE INDEX ai_judgments_retry_idx ON ai_judgments (status, created_at);
```

- [ ] **Step 2: Apply it to the local DB** (from `feature-toggle/`; the DB already holds data, which covers "applies on a DB with existing data")

Run: `sqlx migrate run --database-url "$DATABASE_URL" --source feature-toggle-backend/migrations`
Expected: `Applied 20261002030000/migrate ai judgments`.

- [ ] **Step 3: Add the kind enums** — in `src/judgment/mod.rs`, after the `pub use client::...` line:

```rust
use std::str::FromStr;

/// What a judgment decides. Stored in `ai_judgments.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JudgmentKind {
    ApprovalRisk,
    Justification,
    FlagKind,
}

impl JudgmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JudgmentKind::ApprovalRisk => "approval_risk",
            JudgmentKind::Justification => "justification",
            JudgmentKind::FlagKind => "flag_kind",
        }
    }
}

impl FromStr for JudgmentKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "approval_risk" => Ok(JudgmentKind::ApprovalRisk),
            "justification" => Ok(JudgmentKind::Justification),
            "flag_kind" => Ok(JudgmentKind::FlagKind),
            other => Err(format!("unknown judgment kind: {other}")),
        }
    }
}

/// The row a judgment is about. Stored in `ai_judgments.subject_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectType {
    ApprovalRequest,
    Feature,
    Activity,
    FreezeWindow,
    ScheduledChange,
}

impl SubjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectType::ApprovalRequest => "approval_request",
            SubjectType::Feature => "feature",
            SubjectType::Activity => "activity",
            SubjectType::FreezeWindow => "freeze_window",
            SubjectType::ScheduledChange => "scheduled_change",
        }
    }
}

impl FromStr for SubjectType {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "approval_request" => Ok(SubjectType::ApprovalRequest),
            "feature" => Ok(SubjectType::Feature),
            "activity" => Ok(SubjectType::Activity),
            "freeze_window" => Ok(SubjectType::FreezeWindow),
            "scheduled_change" => Ok(SubjectType::ScheduledChange),
            other => Err(format!("unknown subject type: {other}")),
        }
    }
}
```

Add to the `mod tests` block in the same file:

```rust
    #[test]
    fn kind_and_subject_round_trip_through_strings() {
        for kind in [JudgmentKind::ApprovalRisk, JudgmentKind::Justification, JudgmentKind::FlagKind] {
            assert_eq!(kind.as_str().parse::<JudgmentKind>(), Ok(kind));
        }
        for subject in [
            SubjectType::ApprovalRequest,
            SubjectType::Feature,
            SubjectType::Activity,
            SubjectType::FreezeWindow,
            SubjectType::ScheduledChange,
        ] {
            assert_eq!(subject.as_str().parse::<SubjectType>(), Ok(subject));
        }
        assert!("nope".parse::<JudgmentKind>().is_err());
    }
```

- [ ] **Step 4: Write the failing repository tests** — create `tests/database/ai_test.rs`:

```rust
use feature_toggle_backend::Error;
use feature_toggle_backend::database::ai::{
    AiFeature, JudgmentResult, NewJudgment, TeamAiSettings, ai_judgment_repository,
    team_ai_settings_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::judgment::{JudgmentKind, SubjectType};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("AI test team {team_id}"))
        .bind("ai repository test")
        .execute(pool)
        .await
        .expect("failed to insert team");
    team_id
}

async fn delete_team(pool: &PgPool, team_id: Uuid) {
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("failed to delete team");
}

fn new_judgment(team_id: Uuid, subject_id: Uuid, hash: &str) -> NewJudgment {
    NewJudgment {
        team_id,
        kind: JudgmentKind::FlagKind,
        subject_type: SubjectType::Feature,
        subject_id,
        input: json!({ "feature": { "key": "checkout-v2" } }),
        input_hash: hash.to_string(),
    }
}

fn result() -> JudgmentResult {
    JudgmentResult {
        model: "jev-1.13.0".to_string(),
        raw_answers: json!({ "kind": { "type": "noul", "noul": 0.9 } }),
        derived: json!({ "level": "low" }),
        input_tokens: Some(120),
    }
}

#[tokio::test]
async fn settings_default_to_all_off_and_upsert_persists() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = team_ai_settings_repository(pool.clone());

    let empty = repo.get(team_id).await.unwrap();
    assert_eq!(empty.settings, TeamAiSettings::default());
    assert!(empty.updated_at.is_none());

    let actor = Uuid::new_v4();
    let wanted = TeamAiSettings {
        approval_risk: true,
        justification_check: false,
        flag_kind: true,
        nl_search: false,
    };
    let saved = repo.upsert(team_id, wanted.clone(), Some(actor)).await.unwrap();
    assert_eq!(saved.settings, wanted);
    assert_eq!(saved.updated_by, Some(actor));
    assert!(saved.updated_at.is_some());

    let loaded = repo.get(team_id).await.unwrap();
    assert_eq!(loaded.settings, wanted);
    assert!(loaded.settings.is_enabled(AiFeature::FlagKind));
    assert!(!loaded.settings.is_enabled(AiFeature::NlSearch));

    let off = repo.upsert(team_id, TeamAiSettings::default(), None).await.unwrap();
    assert_eq!(off.settings, TeamAiSettings::default());

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn upsert_unknown_team_is_not_found() {
    let pool = init_pg_pool().await;
    let repo = team_ai_settings_repository(pool);
    let missing = Uuid::new_v4();
    let error = repo
        .upsert(missing, TeamAiSettings::default(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound(id) if id == missing), "{error:?}");
}

#[tokio::test]
async fn upsert_pending_resets_an_existing_row() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());
    let subject_id = Uuid::new_v4();

    let first = repo.upsert_pending(new_judgment(team_id, subject_id, "h1")).await.unwrap();
    assert_eq!(first.status, "pending");
    assert!(repo.mark_failed(first.id, "h1".into(), "boom".into()).await.unwrap());

    let second = repo.upsert_pending(new_judgment(team_id, subject_id, "h2")).await.unwrap();
    assert_eq!(second.id, first.id, "the unique key keeps one row per subject and kind");
    assert_eq!(second.status, "pending");
    assert_eq!(second.attempts, 0);
    assert_eq!(second.input_hash, "h2");
    assert!(second.error.is_none());
    assert!(second.completed_at.is_none());

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn mark_done_with_stale_hash_changes_nothing() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());
    let subject_id = Uuid::new_v4();
    let row = repo.upsert_pending(new_judgment(team_id, subject_id, "new")).await.unwrap();

    assert!(!repo.mark_done(row.id, "old".into(), result()).await.unwrap());
    let unchanged = repo
        .get_for_subject(SubjectType::Feature, subject_id, JudgmentKind::FlagKind)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.status, "pending");
    assert!(unchanged.derived.is_none());

    assert!(repo.mark_done(row.id, "new".into(), result()).await.unwrap());
    let done = repo
        .get_for_subject(SubjectType::Feature, subject_id, JudgmentKind::FlagKind)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "done");
    assert_eq!(done.model.as_deref(), Some("jev-1.13.0"));
    assert_eq!(done.derived, Some(json!({ "level": "low" })));
    assert_eq!(done.input_tokens, Some(120));
    assert!(done.completed_at.is_some());

    // A late failure never overwrites a finished judgment.
    assert!(!repo.mark_failed(row.id, "new".into(), "late".into()).await.unwrap());

    let many = repo
        .get_for_subjects(SubjectType::Feature, vec![subject_id, Uuid::new_v4()], JudgmentKind::FlagKind)
        .await
        .unwrap();
    assert_eq!(many.len(), 1);

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn list_retryable_picks_old_pending_and_failed_under_three_attempts() {
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());

    let fresh_pending = repo.upsert_pending(new_judgment(team_id, Uuid::new_v4(), "a")).await.unwrap();
    let old_pending = repo.upsert_pending(new_judgment(team_id, Uuid::new_v4(), "b")).await.unwrap();
    let failed_twice = repo.upsert_pending(new_judgment(team_id, Uuid::new_v4(), "c")).await.unwrap();
    let failed_thrice = repo.upsert_pending(new_judgment(team_id, Uuid::new_v4(), "d")).await.unwrap();
    let done = repo.upsert_pending(new_judgment(team_id, Uuid::new_v4(), "e")).await.unwrap();

    sqlx::query("UPDATE ai_judgments SET created_at = NOW() - INTERVAL '5 minutes' WHERE id = $1")
        .bind(old_pending.id)
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..2 {
        repo.mark_failed(failed_twice.id, "c".into(), "x".into()).await.unwrap();
    }
    for _ in 0..3 {
        repo.mark_failed(failed_thrice.id, "d".into(), "x".into()).await.unwrap();
    }
    repo.mark_done(done.id, "e".into(), result()).await.unwrap();

    let ids: Vec<Uuid> = repo
        .list_retryable(1000)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert!(ids.contains(&old_pending.id));
    assert!(ids.contains(&failed_twice.id));
    assert!(!ids.contains(&fresh_pending.id));
    assert!(!ids.contains(&failed_thrice.id));
    assert!(!ids.contains(&done.id));

    delete_team(&pool, team_id).await;
}
```

Add `mod ai_test;` as the first line of `tests/database/mod.rs`.

- [ ] **Step 5: Run the tests and confirm they fail**

Run: `cargo test -p feature-toggle-backend --test integration_test ai_test`
Expected: compile error, `could not find ai in database`.

- [ ] **Step 6: Implement the repositories** — create `src/database/ai.rs`:

```rust
//! Persistence for TypeSafe judgments and per-team AI toggles.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mockall::automock;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::database::{Error, handle_error};
use crate::judgment::{JudgmentKind, SubjectType};

/// Total attempts (first run plus sweeps) before a failed judgment stays failed.
pub const MAX_ATTEMPTS: i32 = 3;
/// Rows the retry sweep takes per tick.
pub const RETRY_BATCH: i64 = 50;

const TEAM_FK_CONSTRAINT: &str = "team_ai_settings_team_id_fkey";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFeature {
    ApprovalRisk,
    JustificationCheck,
    FlagKind,
    NlSearch,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TeamAiSettings {
    pub approval_risk: bool,
    pub justification_check: bool,
    pub flag_kind: bool,
    pub nl_search: bool,
}

impl TeamAiSettings {
    pub fn is_enabled(&self, feature: AiFeature) -> bool {
        match feature {
            AiFeature::ApprovalRisk => self.approval_risk,
            AiFeature::JustificationCheck => self.justification_check,
            AiFeature::FlagKind => self.flag_kind,
            AiFeature::NlSearch => self.nl_search,
        }
    }
}

/// Settings plus audit fields. `updated_at` is `None` when no row exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTeamAiSettings {
    pub settings: TeamAiSettings,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct TeamAiSettingsRow {
    approval_risk: bool,
    justification_check: bool,
    flag_kind: bool,
    nl_search: bool,
    updated_at: DateTime<Utc>,
    updated_by: Option<Uuid>,
}

impl From<TeamAiSettingsRow> for StoredTeamAiSettings {
    fn from(row: TeamAiSettingsRow) -> Self {
        Self {
            settings: TeamAiSettings {
                approval_risk: row.approval_risk,
                justification_check: row.justification_check,
                flag_kind: row.flag_kind,
                nl_search: row.nl_search,
            },
            updated_at: Some(row.updated_at),
            updated_by: row.updated_by,
        }
    }
}

#[automock]
#[async_trait]
pub trait TeamAiSettingsRepository: Send + Sync {
    /// All features off when the team has no row.
    async fn get(&self, team_id: Uuid) -> Result<StoredTeamAiSettings, Error>;
    /// `Error::NotFound(team_id)` when the team does not exist.
    async fn upsert(
        &self,
        team_id: Uuid,
        settings: TeamAiSettings,
        updated_by: Option<Uuid>,
    ) -> Result<StoredTeamAiSettings, Error>;
    fn clone_box(&self) -> Box<dyn TeamAiSettingsRepository>;
}

impl Clone for Box<dyn TeamAiSettingsRepository> {
    fn clone(&self) -> Box<dyn TeamAiSettingsRepository> {
        self.clone_box()
    }
}

pub fn team_ai_settings_repository(pool: PgPool) -> Box<dyn TeamAiSettingsRepository> {
    Box::new(PgTeamAiSettingsRepository { pool })
}

#[derive(Clone)]
pub struct PgTeamAiSettingsRepository {
    pool: PgPool,
}

const SETTINGS_COLUMNS: &str =
    "approval_risk, justification_check, flag_kind, nl_search, updated_at, updated_by";

#[async_trait]
impl TeamAiSettingsRepository for PgTeamAiSettingsRepository {
    async fn get(&self, team_id: Uuid) -> Result<StoredTeamAiSettings, Error> {
        let result = sqlx::query_as::<_, TeamAiSettingsRow>(&format!(
            "SELECT {SETTINGS_COLUMNS} FROM team_ai_settings WHERE team_id = $1"
        ))
        .bind(team_id)
        .fetch_optional(&self.pool)
        .await;
        Ok(handle_error(None, result)?
            .map(StoredTeamAiSettings::from)
            .unwrap_or_default())
    }

    async fn upsert(
        &self,
        team_id: Uuid,
        settings: TeamAiSettings,
        updated_by: Option<Uuid>,
    ) -> Result<StoredTeamAiSettings, Error> {
        let result = sqlx::query_as::<_, TeamAiSettingsRow>(&format!(
            r#"
            INSERT INTO team_ai_settings
                (team_id, approval_risk, justification_check, flag_kind, nl_search, updated_at, updated_by)
            VALUES ($1, $2, $3, $4, $5, NOW(), $6)
            ON CONFLICT (team_id) DO UPDATE SET
                approval_risk = EXCLUDED.approval_risk,
                justification_check = EXCLUDED.justification_check,
                flag_kind = EXCLUDED.flag_kind,
                nl_search = EXCLUDED.nl_search,
                updated_at = NOW(),
                updated_by = EXCLUDED.updated_by
            RETURNING {SETTINGS_COLUMNS}
            "#
        ))
        .bind(team_id)
        .bind(settings.approval_risk)
        .bind(settings.justification_check)
        .bind(settings.flag_kind)
        .bind(settings.nl_search)
        .bind(updated_by)
        .fetch_one(&self.pool)
        .await;

        match result {
            Err(sqlx::Error::Database(db_error))
                if db_error.constraint() == Some(TEAM_FK_CONSTRAINT) =>
            {
                Err(Error::NotFound(team_id))
            }
            other => handle_error(Some(team_id), other).map(StoredTeamAiSettings::from),
        }
    }

    fn clone_box(&self) -> Box<dyn TeamAiSettingsRepository> {
        Box::new(self.clone())
    }
}

/// One row of `ai_judgments`. `kind`, `subject_type`, `status` are the stored strings.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AiJudgment {
    pub id: Uuid,
    pub team_id: Uuid,
    pub subject_type: String,
    pub subject_id: Uuid,
    pub kind: String,
    pub status: String,
    pub attempts: i32,
    pub input: Value,
    pub input_hash: String,
    pub model: Option<String>,
    pub raw_answers: Option<Value>,
    pub derived: Option<Value>,
    pub input_tokens: Option<i32>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewJudgment {
    pub team_id: Uuid,
    pub kind: JudgmentKind,
    pub subject_type: SubjectType,
    pub subject_id: Uuid,
    pub input: Value,
    pub input_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentResult {
    pub model: String,
    pub raw_answers: Value,
    pub derived: Value,
    pub input_tokens: Option<i32>,
}

#[automock]
#[async_trait]
pub trait AiJudgmentRepository: Send + Sync {
    /// Inserts or resets the row for (subject_type, subject_id, kind) to `pending`.
    async fn upsert_pending(&self, judgment: NewJudgment) -> Result<AiJudgment, Error>;
    /// False when `input_hash` no longer matches (a newer submission won).
    async fn mark_done(
        &self,
        id: Uuid,
        input_hash: String,
        result: JudgmentResult,
    ) -> Result<bool, Error>;
    /// False when the hash is stale or the row is already done. Increments `attempts`.
    async fn mark_failed(&self, id: Uuid, input_hash: String, error: String)
    -> Result<bool, Error>;
    async fn get_for_subject(
        &self,
        subject_type: SubjectType,
        subject_id: Uuid,
        kind: JudgmentKind,
    ) -> Result<Option<AiJudgment>, Error>;
    async fn get_for_subjects(
        &self,
        subject_type: SubjectType,
        subject_ids: Vec<Uuid>,
        kind: JudgmentKind,
    ) -> Result<Vec<AiJudgment>, Error>;
    /// Pending rows older than 2 minutes and failed rows under `MAX_ATTEMPTS`, oldest first.
    async fn list_retryable(&self, limit: i64) -> Result<Vec<AiJudgment>, Error>;
    fn clone_box(&self) -> Box<dyn AiJudgmentRepository>;
}

impl Clone for Box<dyn AiJudgmentRepository> {
    fn clone(&self) -> Box<dyn AiJudgmentRepository> {
        self.clone_box()
    }
}

pub fn ai_judgment_repository(pool: PgPool) -> Box<dyn AiJudgmentRepository> {
    Box::new(PgAiJudgmentRepository { pool })
}

#[derive(Clone)]
pub struct PgAiJudgmentRepository {
    pool: PgPool,
}

const JUDGMENT_COLUMNS: &str = "id, team_id, subject_type, subject_id, kind, status, attempts, \
     input, input_hash, model, raw_answers, derived, input_tokens, error, created_at, completed_at";

#[async_trait]
impl AiJudgmentRepository for PgAiJudgmentRepository {
    async fn upsert_pending(&self, judgment: NewJudgment) -> Result<AiJudgment, Error> {
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            r#"
            INSERT INTO ai_judgments
                (id, team_id, subject_type, subject_id, kind, status, attempts, input, input_hash)
            VALUES ($1, $2, $3, $4, $5, 'pending', 0, $6, $7)
            ON CONFLICT (subject_type, subject_id, kind) DO UPDATE SET
                team_id = EXCLUDED.team_id,
                status = 'pending',
                attempts = 0,
                input = EXCLUDED.input,
                input_hash = EXCLUDED.input_hash,
                model = NULL,
                raw_answers = NULL,
                derived = NULL,
                input_tokens = NULL,
                error = NULL,
                created_at = NOW(),
                completed_at = NULL
            RETURNING {JUDGMENT_COLUMNS}
            "#
        ))
        .bind(Uuid::new_v4())
        .bind(judgment.team_id)
        .bind(judgment.subject_type.as_str())
        .bind(judgment.subject_id)
        .bind(judgment.kind.as_str())
        .bind(&judgment.input)
        .bind(&judgment.input_hash)
        .fetch_one(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn mark_done(
        &self,
        id: Uuid,
        input_hash: String,
        result: JudgmentResult,
    ) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments
            SET status = 'done', model = $3, raw_answers = $4, derived = $5,
                input_tokens = $6, error = NULL, completed_at = NOW()
            WHERE id = $1 AND input_hash = $2
            "#,
        )
        .bind(id)
        .bind(&input_hash)
        .bind(&result.model)
        .bind(&result.raw_answers)
        .bind(&result.derived)
        .bind(result.input_tokens)
        .execute(&self.pool)
        .await;
        Ok(handle_error(Some(id), outcome)?.rows_affected() > 0)
    }

    async fn mark_failed(
        &self,
        id: Uuid,
        input_hash: String,
        error: String,
    ) -> Result<bool, Error> {
        let outcome = sqlx::query(
            r#"
            UPDATE ai_judgments
            SET status = 'failed', error = $3, attempts = attempts + 1
            WHERE id = $1 AND input_hash = $2 AND status <> 'done'
            "#,
        )
        .bind(id)
        .bind(&input_hash)
        .bind(&error)
        .execute(&self.pool)
        .await;
        Ok(handle_error(Some(id), outcome)?.rows_affected() > 0)
    }

    async fn get_for_subject(
        &self,
        subject_type: SubjectType,
        subject_id: Uuid,
        kind: JudgmentKind,
    ) -> Result<Option<AiJudgment>, Error> {
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            "SELECT {JUDGMENT_COLUMNS} FROM ai_judgments \
             WHERE subject_type = $1 AND subject_id = $2 AND kind = $3"
        ))
        .bind(subject_type.as_str())
        .bind(subject_id)
        .bind(kind.as_str())
        .fetch_optional(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn get_for_subjects(
        &self,
        subject_type: SubjectType,
        subject_ids: Vec<Uuid>,
        kind: JudgmentKind,
    ) -> Result<Vec<AiJudgment>, Error> {
        if subject_ids.is_empty() {
            return Ok(Vec::new());
        }
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            "SELECT {JUDGMENT_COLUMNS} FROM ai_judgments \
             WHERE subject_type = $1 AND subject_id = ANY($2) AND kind = $3"
        ))
        .bind(subject_type.as_str())
        .bind(&subject_ids)
        .bind(kind.as_str())
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    async fn list_retryable(&self, limit: i64) -> Result<Vec<AiJudgment>, Error> {
        let result = sqlx::query_as::<_, AiJudgment>(&format!(
            r#"
            SELECT {JUDGMENT_COLUMNS} FROM ai_judgments
            WHERE (status = 'pending' AND created_at < NOW() - INTERVAL '2 minutes')
               OR (status = 'failed' AND attempts < $1)
            ORDER BY created_at ASC
            LIMIT $2
            "#
        ))
        .bind(MAX_ATTEMPTS)
        .bind(limit)
        .fetch_all(&self.pool)
        .await;
        handle_error(None, result)
    }

    fn clone_box(&self) -> Box<dyn AiJudgmentRepository> {
        Box::new(self.clone())
    }
}
```

Add `pub mod ai;` to `src/database/mod.rs` right after `pub mod activity_log;`.

- [ ] **Step 7: Run the tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --test integration_test ai_test` and `cargo test -p feature-toggle-backend --lib judgment::tests`
Expected: 5 integration tests pass; the enum round-trip test passes. If the FK constraint name differs, check with `psql "$DATABASE_URL" -c '\d team_ai_settings'` and update `TEAM_FK_CONSTRAINT`.

- [ ] **Step 8: Commit**

```bash
git add feature-toggle-backend/migrations/20261002030000_ai_judgments.sql feature-toggle-backend/src/judgment/mod.rs feature-toggle-backend/src/database/ai.rs feature-toggle-backend/src/database/mod.rs feature-toggle-backend/tests/database/ai_test.rs feature-toggle-backend/tests/database/mod.rs
git commit -m "feat(ai): add ai_judgments and team_ai_settings tables with repositories

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 6: `JudgmentService`, retry scheduler, wiring

**Files:**
- Create: `feature-toggle-backend/src/judgment/service.rs`
- Modify: `feature-toggle-backend/src/judgment/mod.rs` (`pub mod service;`, `AiRuntime.judgments`)
- Create: `feature-toggle-backend/src/scheduler/ai_judgment_retry.rs`
- Modify: `feature-toggle-backend/src/scheduler/mod.rs`
- Modify: `feature-toggle-backend/src/lib.rs`

**Interfaces:**
- Consumes: `JudgmentClient`, `MockJudgmentClient`, `types::{Answers, RequestParts, Question, SystemOneResponse, Answer, Usage}`, `JudgmentKind`, `SubjectType` (Tasks 2, 3, 5); `database::ai::{AiJudgment, AiJudgmentRepository, TeamAiSettingsRepository, AiFeature, NewJudgment, JudgmentResult, RETRY_BATCH}` and their mocks (Task 5).
- Produces (in `crate::judgment::service`): trait `JudgmentHandler { fn kind(&self) -> JudgmentKind; fn build(&self, input: &Value) -> RequestParts; fn derive(&self, input: &Value, answers: &Answers) -> Value; async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error>; }`; `RunOutcome { Applied, Stale, Failed }`; `input_hash(&Value) -> String`; `JudgmentService::new(Arc<dyn JudgmentClient>, Box<dyn AiJudgmentRepository>, Box<dyn TeamAiSettingsRepository>) -> Self`, `with_handler(self, Arc<dyn JudgmentHandler>) -> Self`, `async team_enabled(&self, Uuid, AiFeature) -> bool`, `async submit(self: &Arc<Self>, team_id: Uuid, kind: JudgmentKind, subject_type: SubjectType, subject_id: Uuid, input: Value) -> Result<Uuid, crate::Error>`, `async run(&self, AiJudgment) -> RunOutcome`, `async retry_tick(&self) -> usize`. `AiRuntime.judgments: Option<Arc<JudgmentService>>` and `AiRuntime::with_judgments(self, Option<Arc<JudgmentService>>) -> Self`. `scheduler::AiJudgmentRetryScheduler::new(Arc<JudgmentService>, Duration)` with `start(self)`.

- [ ] **Step 1: Write the failing tests** — create `src/judgment/service.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use tokio::sync::Notify;
    use uuid::Uuid;

    use super::*;
    use crate::database::ai::{
        AiFeature, AiJudgment, MockAiJudgmentRepository, MockTeamAiSettingsRepository,
        StoredTeamAiSettings, TeamAiSettings,
    };
    use crate::judgment::client::{JudgmentError, MockJudgmentClient};
    use crate::judgment::types::{Answer, Answers, Question, RequestParts, SystemOneResponse, Usage};
    use crate::judgment::{JudgmentKind, SubjectType};

    struct FakeHandler {
        applied: Arc<AtomicUsize>,
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl JudgmentHandler for FakeHandler {
        fn kind(&self) -> JudgmentKind {
            JudgmentKind::FlagKind
        }

        fn build(&self, input: &Value) -> RequestParts {
            RequestParts {
                state: input.clone(),
                questions: BTreeMap::from([("q".to_string(), Question::noul("Is it?"))]),
            }
        }

        fn derive(&self, _input: &Value, answers: &Answers) -> Value {
            json!({ "q": answers.noul("q") })
        }

        async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error> {
            assert_eq!(judgment.status, "done");
            assert_eq!(judgment.derived, Some(json!({ "q": 0.9 })));
            self.applied.fetch_add(1, Ordering::SeqCst);
            self.notify.notify_one();
            Ok(())
        }
    }

    fn row(hash: &str) -> AiJudgment {
        AiJudgment {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            subject_type: "feature".into(),
            subject_id: Uuid::new_v4(),
            kind: "flag_kind".into(),
            status: "pending".into(),
            attempts: 0,
            input: json!({ "feature": { "key": "checkout-v2" } }),
            input_hash: hash.into(),
            model: None,
            raw_answers: None,
            derived: None,
            input_tokens: None,
            error: None,
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    fn ok_response() -> SystemOneResponse {
        SystemOneResponse {
            model: "jev-1.13.0".into(),
            answers: Answers(BTreeMap::from([("q".to_string(), Answer::Noul { noul: 0.9 })])),
            usage: Usage { input_tokens: 42, output_tokens: 1 },
        }
    }

    fn service(
        client: MockJudgmentClient,
        repo: MockAiJudgmentRepository,
        settings: MockTeamAiSettingsRepository,
    ) -> (Arc<JudgmentService>, Arc<AtomicUsize>, Arc<Notify>) {
        let applied = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(Notify::new());
        let handler = FakeHandler { applied: applied.clone(), notify: notify.clone() };
        let service = JudgmentService::new(Arc::new(client), Box::new(repo), Box::new(settings))
            .with_handler(Arc::new(handler));
        (Arc::new(service), applied, notify)
    }

    #[test]
    fn input_hash_ignores_key_order() {
        let a: Value = serde_json::from_str(r#"{"b":1,"a":{"y":2,"x":3}}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":{"x":3,"y":2},"b":1}"#).unwrap();
        assert_eq!(input_hash(&a), input_hash(&b));
        assert_ne!(input_hash(&a), input_hash(&json!({ "b": 2 })));
        assert_eq!(input_hash(&a).len(), 64);
    }

    #[tokio::test]
    async fn success_writes_done_and_applies_once() {
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(1).returning(|_, _| Ok(ok_response()));
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_done()
            .withf(|_, hash, result| {
                hash == "h1"
                    && result.model == "jev-1.13.0"
                    && result.derived == json!({ "q": 0.9 })
                    && result.input_tokens == Some(42)
            })
            .times(1)
            .returning(|_, _, _| Ok(true));
        repo.expect_mark_failed().times(0);
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.run(row("h1")).await, RunOutcome::Applied);
        assert_eq!(applied.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn client_error_writes_failed_and_skips_apply() {
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(1).returning(|_, _| Err(JudgmentError::Timeout));
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_done().times(0);
        repo.expect_mark_failed()
            .withf(|_, hash, error| hash == "h1" && error.contains("timed out"))
            .times(1)
            .returning(|_, _, _| Ok(true));
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.run(row("h1")).await, RunOutcome::Failed);
        assert_eq!(applied.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn newer_submission_makes_old_run_stale() {
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(1).returning(|_, _| Ok(ok_response()));
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_done().times(1).returning(|_, _, _| Ok(false));
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.run(row("old")).await, RunOutcome::Stale);
        assert_eq!(applied.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unknown_kind_is_marked_failed_without_a_call() {
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(0);
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_mark_failed()
            .withf(|_, _, error| error.contains("no handler"))
            .times(1)
            .returning(|_, _, _| Ok(true));
        let (service, _, _) = service(client, repo, MockTeamAiSettingsRepository::new());
        let mut judgment = row("h1");
        judgment.kind = "approval_risk".into();

        assert_eq!(service.run(judgment).await, RunOutcome::Failed);
    }

    #[tokio::test]
    async fn submit_persists_then_runs_in_background() {
        let pending = row("ignored");
        let pending_id = pending.id;
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_upsert_pending()
            .withf(|new| new.kind == JudgmentKind::FlagKind && new.input_hash.len() == 64)
            .times(1)
            .returning(move |new| {
                let mut stored = pending.clone();
                stored.input = new.input;
                stored.input_hash = new.input_hash;
                Ok(stored)
            });
        repo.expect_mark_done().times(1).returning(|_, _, _| Ok(true));
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(1).returning(|_, _| Ok(ok_response()));
        let (service, applied, notify) =
            service(client, repo, MockTeamAiSettingsRepository::new());

        let id = service
            .submit(
                Uuid::new_v4(),
                JudgmentKind::FlagKind,
                SubjectType::Feature,
                Uuid::new_v4(),
                json!({ "feature": { "key": "checkout-v2" } }),
            )
            .await
            .unwrap();
        assert_eq!(id, pending_id);
        tokio::time::timeout(Duration::from_secs(2), notify.notified())
            .await
            .expect("background run did not apply");
        assert_eq!(applied.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn team_enabled_reads_settings_and_fails_closed() {
        let mut settings = MockTeamAiSettingsRepository::new();
        let on = Uuid::new_v4();
        settings.expect_get().returning(move |team_id| {
            if team_id == on {
                Ok(StoredTeamAiSettings {
                    settings: TeamAiSettings { flag_kind: true, ..TeamAiSettings::default() },
                    ..StoredTeamAiSettings::default()
                })
            } else {
                Err(crate::Error::InvalidInput("db down".into()))
            }
        });
        let (service, _, _) =
            service(MockJudgmentClient::new(), MockAiJudgmentRepository::new(), settings);

        assert!(service.team_enabled(on, AiFeature::FlagKind).await);
        assert!(!service.team_enabled(on, AiFeature::NlSearch).await);
        assert!(!service.team_enabled(Uuid::new_v4(), AiFeature::FlagKind).await);
    }

    #[tokio::test]
    async fn retry_tick_runs_each_retryable_row() {
        let rows = vec![row("a"), row("b")];
        let mut repo = MockAiJudgmentRepository::new();
        repo.expect_list_retryable()
            .withf(|limit| *limit == crate::database::ai::RETRY_BATCH)
            .times(1)
            .returning(move |_| Ok(rows.clone()));
        repo.expect_mark_done().times(2).returning(|_, _, _| Ok(true));
        let mut client = MockJudgmentClient::new();
        client.expect_evaluate().times(2).returning(|_, _| Ok(ok_response()));
        let (service, applied, _) = service(client, repo, MockTeamAiSettingsRepository::new());

        assert_eq!(service.retry_tick().await, 2);
        assert_eq!(applied.load(Ordering::SeqCst), 2);
    }
}
```

Add `pub mod service;` to `src/judgment/mod.rs` after `pub mod client;`.

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test -p feature-toggle-backend --lib judgment::service`
Expected: compile errors (`JudgmentService`, `JudgmentHandler`, `input_hash` not found).

- [ ] **Step 3: Implement the service** — prepend to `src/judgment/service.rs`:

```rust
//! Runs async judgments: persist input, call Jev, store answers, apply.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use log::{error, warn};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::client::JudgmentClient;
use super::types::{Answers, RequestParts};
use super::{JudgmentKind, SubjectType};
use crate::database::ai::{
    AiFeature, AiJudgment, AiJudgmentRepository, JudgmentResult, NewJudgment, RETRY_BATCH,
    TeamAiSettingsRepository,
};

/// Kind-specific logic. `build` and `derive` are pure so thresholds can be
/// re-derived from stored `raw_answers` without new API calls.
#[async_trait]
pub trait JudgmentHandler: Send + Sync {
    fn kind(&self) -> JudgmentKind;
    /// Build state and questions from the stored input snapshot.
    fn build(&self, input: &Value) -> RequestParts;
    /// Turn raw answers into the derived result.
    fn derive(&self, input: &Value, answers: &Answers) -> Value;
    /// Side effects after `done`. Must check that its subject is still fresh.
    async fn apply(&self, judgment: &AiJudgment) -> Result<(), crate::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Stored as done and `apply` ran.
    Applied,
    /// A newer submission replaced the input; nothing stored or applied.
    Stale,
    /// Stored as failed (or could not be stored); the sweep may retry it.
    Failed,
}

/// SHA-256 hex of the input JSON. `serde_json::Value` objects are
/// `BTreeMap`-backed, so key order does not change the hash.
pub fn input_hash(input: &Value) -> String {
    let bytes = serde_json::to_vec(input).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct JudgmentService {
    client: Arc<dyn JudgmentClient>,
    judgments: Box<dyn AiJudgmentRepository>,
    settings: Box<dyn TeamAiSettingsRepository>,
    handlers: HashMap<JudgmentKind, Arc<dyn JudgmentHandler>>,
}

impl JudgmentService {
    pub fn new(
        client: Arc<dyn JudgmentClient>,
        judgments: Box<dyn AiJudgmentRepository>,
        settings: Box<dyn TeamAiSettingsRepository>,
    ) -> Self {
        Self {
            client,
            judgments,
            settings,
            handlers: HashMap::new(),
        }
    }

    /// Registers the handler for its kind. Call before wrapping in `Arc`.
    pub fn with_handler(mut self, handler: Arc<dyn JudgmentHandler>) -> Self {
        self.handlers.insert(handler.kind(), handler);
        self
    }

    /// Whether `feature` is on for the team. Fails closed on a read error.
    pub async fn team_enabled(&self, team_id: Uuid, feature: AiFeature) -> bool {
        match self.settings.get(team_id).await {
            Ok(stored) => stored.settings.is_enabled(feature),
            Err(err) => {
                warn!("Could not read AI settings for team {team_id}: {err}");
                false
            }
        }
    }

    /// Persists a pending judgment and runs it in the background. Returns the
    /// row id right after the write; never waits for the API.
    pub async fn submit(
        self: &Arc<Self>,
        team_id: Uuid,
        kind: JudgmentKind,
        subject_type: SubjectType,
        subject_id: Uuid,
        input: Value,
    ) -> Result<Uuid, crate::Error> {
        let input_hash = input_hash(&input);
        let row = self
            .judgments
            .upsert_pending(NewJudgment {
                team_id,
                kind,
                subject_type,
                subject_id,
                input,
                input_hash,
            })
            .await?;
        let id = row.id;
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.run(row).await;
        });
        Ok(id)
    }

    pub async fn run(&self, row: AiJudgment) -> RunOutcome {
        let handler = row
            .kind
            .parse::<JudgmentKind>()
            .ok()
            .and_then(|kind| self.handlers.get(&kind).cloned());
        let Some(handler) = handler else {
            self.fail(&row, format!("no handler registered for kind {}", row.kind))
                .await;
            return RunOutcome::Failed;
        };

        let parts = handler.build(&row.input);
        let response = match self.client.evaluate(parts.state, parts.questions).await {
            Ok(response) => response,
            Err(err) => {
                self.fail(&row, err.to_string()).await;
                return RunOutcome::Failed;
            }
        };

        let derived = handler.derive(&row.input, &response.answers);
        let raw_answers = serde_json::to_value(&response.answers).unwrap_or(Value::Null);
        let input_tokens = i32::try_from(response.usage.input_tokens).ok();
        let result = JudgmentResult {
            model: response.model.clone(),
            raw_answers: raw_answers.clone(),
            derived: derived.clone(),
            input_tokens,
        };

        match self
            .judgments
            .mark_done(row.id, row.input_hash.clone(), result)
            .await
        {
            Ok(true) => {
                let done = AiJudgment {
                    status: "done".to_string(),
                    model: Some(response.model),
                    raw_answers: Some(raw_answers),
                    derived: Some(derived),
                    input_tokens,
                    error: None,
                    completed_at: Some(Utc::now()),
                    ..row
                };
                if let Err(err) = handler.apply(&done).await {
                    error!(
                        "AI judgment {} ({}) apply step failed: {err}",
                        done.id, done.kind
                    );
                }
                RunOutcome::Applied
            }
            Ok(false) => RunOutcome::Stale,
            Err(err) => {
                error!("Could not store AI judgment {} result: {err}", row.id);
                RunOutcome::Failed
            }
        }
    }

    /// Re-runs retryable rows one by one. Returns how many it ran.
    pub async fn retry_tick(&self) -> usize {
        let rows = match self.judgments.list_retryable(RETRY_BATCH).await {
            Ok(rows) => rows,
            Err(err) => {
                error!("Could not list retryable AI judgments: {err}");
                return 0;
            }
        };
        let count = rows.len();
        for row in rows {
            self.run(row).await;
        }
        count
    }

    /// Stores the error text in the row (never in a log line).
    async fn fail(&self, row: &AiJudgment, message: String) {
        match self
            .judgments
            .mark_failed(row.id, row.input_hash.clone(), message)
            .await
        {
            Ok(_) => warn!("AI judgment {} ({}) failed", row.id, row.kind),
            Err(err) => error!("Could not mark AI judgment {} failed: {err}", row.id),
        }
    }
}
```

- [ ] **Step 4: Extend `AiRuntime`** — in `src/judgment/mod.rs`, change `AiRuntime` and its `impl` to:

```rust
/// Shared with REST handlers through `web::Data<AiRuntime>`.
#[derive(Clone)]
pub struct AiRuntime {
    pub client: Option<Arc<dyn JudgmentClient>>,
    pub model: String,
    /// Present only when `client` is: the async judgment pipeline.
    pub judgments: Option<Arc<service::JudgmentService>>,
}

impl AiRuntime {
    pub fn new(client: Option<Arc<dyn JudgmentClient>>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
            judgments: None,
        }
    }

    pub fn with_judgments(mut self, judgments: Option<Arc<service::JudgmentService>>) -> Self {
        self.judgments = judgments;
        self
    }

    pub fn available(&self) -> bool {
        self.client.is_some()
    }

    /// The model id for status responses; `None` when the subsystem is off.
    pub fn status_model(&self) -> Option<String> {
        self.available().then(|| self.model.clone())
    }
}
```

- [ ] **Step 5: Run the tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --lib judgment::`
Expected: all judgment tests pass, including 8 in `judgment::service`.

- [ ] **Step 6: Add the scheduler** — create `src/scheduler/ai_judgment_retry.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use log::info;
use tokio::time;

use crate::judgment::service::JudgmentService;

/// Re-runs pending judgments lost on restart and failed ones under the attempt limit.
pub struct AiJudgmentRetryScheduler {
    service: Arc<JudgmentService>,
    interval: Duration,
}

impl AiJudgmentRetryScheduler {
    pub fn new(service: Arc<JudgmentService>, interval: Duration) -> Self {
        Self { service, interval }
    }

    pub async fn start(self) {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            let retried = self.service.retry_tick().await;
            if retried > 0 {
                info!("AI judgment retry sweep re-ran {retried} judgment(s)");
            }
        }
    }
}
```

In `src/scheduler/mod.rs` add `pub mod ai_judgment_retry;` as the first `pub mod` line and `pub use ai_judgment_retry::AiJudgmentRetryScheduler;` as the first `pub use` line.

- [ ] **Step 7: Wire in `lib.rs::run`** — replace the AI block added in Task 4 Step 5 with:

```rust
    // TypeSafe judgments: on only when TYPESAFE_API_KEY is set.
    let ai_client = judgment::build_client(&cfg.typesafe);
    if ai_client.is_some() {
        log::info!("TypeSafe judgments enabled (model {})", cfg.typesafe.model);
    } else {
        log::info!("TypeSafe judgments disabled (no TYPESAFE_API_KEY)");
    }
```

Then place the following just before the comment `// Start kill switch rollback scheduler`:

```rust
    let team_ai_settings_repository = database::ai::team_ai_settings_repository(db_pool.clone());
    let judgment_service = ai_client.clone().map(|client| {
        Arc::new(judgment::service::JudgmentService::new(
            client,
            database::ai::ai_judgment_repository(db_pool.clone()),
            team_ai_settings_repository.clone_box(),
        ))
    });
    if let Some(service) = judgment_service.clone() {
        let ai_retry_scheduler =
            scheduler::AiJudgmentRetryScheduler::new(service, Duration::from_secs(60));
        tokio::spawn(async move {
            ai_retry_scheduler.start().await;
        });
    }
    let ai_runtime = judgment::AiRuntime::new(ai_client.clone(), cfg.typesafe.model.clone())
        .with_judgments(judgment_service);
```

Keep `.app_data(web::Data::new(ai_runtime.clone()))` in the closure, and add `.app_data(web::Data::new(team_ai_settings_repository.clone_box()))` right after it.

- [ ] **Step 8: Build and run the lib tests**

Run: `cargo build -p feature-toggle-backend && cargo test -p feature-toggle-backend --lib`
Expected: build succeeds; all lib tests pass.

- [ ] **Step 9: Commit**

```bash
git add feature-toggle-backend/src/judgment/service.rs feature-toggle-backend/src/judgment/mod.rs feature-toggle-backend/src/scheduler/ai_judgment_retry.rs feature-toggle-backend/src/scheduler/mod.rs feature-toggle-backend/src/lib.rs
git commit -m "feat(ai): add JudgmentService with background runs and 60s retry sweep

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 7: Team AI settings REST API, contract baseline, AI-01 docs

**Files:**
- Modify: `feature-toggle-backend/src/rest/ai.rs`
- Modify: `feature-toggle-backend/src/rest/mod.rs`
- Modify: `feature-toggle-backend/src/utils/activity_logger.rs`
- Modify: `feature-toggle-backend/contracts/baseline/contract-hashes.json`
- Modify: `docs/ai-judgments/tasks/AI-01-judgment-store-and-team-settings.md`, `docs/ai-judgments/README.md`

**Interfaces:**
- Consumes: `AiRuntime` (Task 6), `TeamAiSettingsRepository`, `TeamAiSettings`, `StoredTeamAiSettings` (Task 5), `utils::activity_logger::log_team_activity`, `database::activity_log::ActivityLogRepository`.
- Produces: `GET /api/v1/teams/{team_id}/ai-settings` and `PUT` (system admin only), DTOs `TeamAiSettingsResponse { available, approvalRisk, justificationCheck, flagKind, nlSearch, updatedAt: string | null }` and `UpdateTeamAiSettingsRequest { approvalRisk, justificationCheck, flagKind, nlSearch }`; activity type `ai_settings_updated` (entity `team`, metadata `{old, new}`).

- [ ] **Step 1: Add the activity type** — in `src/utils/activity_logger.rs`, inside `pub mod activity_types`, add after `TEAM_UPDATED`:

```rust
    pub const AI_SETTINGS_UPDATED: &str = "ai_settings_updated";
```

- [ ] **Step 2: Write the failing tests** — add to the `mod tests` block of `src/rest/ai.rs` (keep the two status tests):

```rust
    use actix_web::HttpMessage;
    use actix_web::http::StatusCode;
    use chrono::Utc;
    use uuid::Uuid;

    use crate::JwtUser;
    use crate::database::activity_log::{ActivityLogRepository, MockActivityLogRepository};
    use crate::database::ai::{
        MockTeamAiSettingsRepository, StoredTeamAiSettings, TeamAiSettings,
        TeamAiSettingsRepository,
    };

    fn jwt(is_admin: bool) -> JwtUser {
        JwtUser {
            id: Uuid::new_v4(),
            username: if is_admin { "admin" } else { "dev" }.to_string(),
            is_admin,
            roles: vec![],
            team_id: None,
            token_hash: "hash".to_string(),
        }
    }

    macro_rules! settings_app {
        ($settings:expr, $activity:expr) => {
            test::init_service(
                App::new()
                    .app_data(web::Data::new(AiRuntime::new(None, "jev-1.13.0")))
                    .app_data(web::Data::new(
                        Box::new($settings) as Box<dyn TeamAiSettingsRepository>
                    ))
                    .app_data(web::Data::new(
                        Box::new($activity) as Box<dyn ActivityLogRepository>
                    ))
                    .service(web::scope("/api/v1").configure(super::configure)),
            )
            .await
        };
    }

    #[actix_web::test]
    async fn get_without_row_returns_all_off() {
        let team_id = Uuid::new_v4();
        let mut settings = MockTeamAiSettingsRepository::new();
        settings
            .expect_get()
            .withf(move |id| *id == team_id)
            .returning(|_| Ok(StoredTeamAiSettings::default()));
        let app = settings_app!(settings, MockActivityLogRepository::new());

        let req = test::TestRequest::get()
            .uri(&format!("/api/v1/teams/{team_id}/ai-settings"))
            .to_request();
        req.extensions_mut().insert(jwt(false));
        let body: Value = test::call_and_read_body_json(&app, req).await;
        assert_eq!(
            body,
            json!({
                "available": false,
                "approvalRisk": false,
                "justificationCheck": false,
                "flagKind": false,
                "nlSearch": false,
                "updatedAt": null
            })
        );
    }

    #[actix_web::test]
    async fn get_with_bad_team_id_returns_400() {
        let app = settings_app!(MockTeamAiSettingsRepository::new(), MockActivityLogRepository::new());
        let req = test::TestRequest::get()
            .uri("/api/v1/teams/not-a-uuid/ai-settings")
            .to_request();
        req.extensions_mut().insert(jwt(true));
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn put_as_non_admin_returns_403() {
        let mut settings = MockTeamAiSettingsRepository::new();
        settings.expect_upsert().times(0);
        let app = settings_app!(settings, MockActivityLogRepository::new());
        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/teams/{}/ai-settings", Uuid::new_v4()))
            .set_json(json!({
                "approvalRisk": true, "justificationCheck": false,
                "flagKind": false, "nlSearch": false
            }))
            .to_request();
        req.extensions_mut().insert(jwt(false));
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn put_as_admin_persists_and_logs_activity() {
        let team_id = Uuid::new_v4();
        let admin = jwt(true);
        let admin_id = admin.id;
        let now = Utc::now();
        let mut settings = MockTeamAiSettingsRepository::new();
        settings
            .expect_get()
            .returning(|_| Ok(StoredTeamAiSettings::default()));
        settings
            .expect_upsert()
            .withf(move |id, wanted, by| {
                *id == team_id
                    && *by == Some(admin_id)
                    && *wanted
                        == TeamAiSettings {
                            approval_risk: true,
                            justification_check: false,
                            flag_kind: true,
                            nl_search: false,
                        }
            })
            .times(1)
            .returning(move |_, wanted, by| {
                Ok(StoredTeamAiSettings {
                    settings: wanted,
                    updated_at: Some(now),
                    updated_by: by,
                })
            });
        let mut activity = MockActivityLogRepository::new();
        activity
            .expect_create_activity()
            .withf(move |entry| {
                entry.activity_type == "ai_settings_updated"
                    && entry.entity_type == "team"
                    && entry.entity_id == team_id.to_string()
                    && entry.metadata.as_ref().is_some_and(|m| {
                        m["old"]["approval_risk"] == false && m["new"]["approval_risk"] == true
                    })
            })
            .times(1)
            .returning(|_| Err(sqlx::Error::RowNotFound));
        let app = settings_app!(settings, activity);

        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/teams/{team_id}/ai-settings"))
            .set_json(json!({
                "approvalRisk": true, "justificationCheck": false,
                "flagKind": true, "nlSearch": false
            }))
            .to_request();
        req.extensions_mut().insert(admin);
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = test::read_body_json(resp).await;
        assert_eq!(body["approvalRisk"], true);
        assert_eq!(body["flagKind"], true);
        assert_eq!(body["nlSearch"], false);
        assert_eq!(body["available"], false);
        assert_eq!(body["updatedAt"], now.to_rfc3339());
    }

    #[actix_web::test]
    async fn put_unknown_team_returns_404() {
        let team_id = Uuid::new_v4();
        let mut settings = MockTeamAiSettingsRepository::new();
        settings
            .expect_get()
            .returning(|_| Ok(StoredTeamAiSettings::default()));
        settings
            .expect_upsert()
            .returning(move |_, _, _| Err(crate::Error::NotFound(team_id)));
        let mut activity = MockActivityLogRepository::new();
        activity.expect_create_activity().times(0);
        let app = settings_app!(settings, activity);

        let req = test::TestRequest::put()
            .uri(&format!("/api/v1/teams/{team_id}/ai-settings"))
            .set_json(json!({
                "approvalRisk": false, "justificationCheck": false,
                "flagKind": false, "nlSearch": false
            }))
            .to_request();
        req.extensions_mut().insert(jwt(true));
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
```

- [ ] **Step 3: Run the tests and confirm they fail**

Run: `cargo test -p feature-toggle-backend --lib rest::ai`
Expected: the 5 new tests fail with 404 Not Found (routes are not registered) or fail to compile on missing DTOs.

- [ ] **Step 4: Implement the handlers** — in `src/rest/ai.rs`, update the module doc comment to `//! AI judgment endpoints: subsystem status (AI-00) and per-team settings (AI-01).`, replace the imports with:

```rust
use actix_web::{HttpMessage, HttpRequest, HttpResponse, Responder, get, put, web};
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::JwtUser;
use crate::database::activity_log::ActivityLogRepository;
use crate::database::ai::{StoredTeamAiSettings, TeamAiSettings, TeamAiSettingsRepository};
use crate::judgment::AiRuntime;
use crate::rest::error::RestError;
use crate::utils::activity_logger::{activity_types, log_team_activity};
```

Add after `get_ai_status`:

```rust
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamAiSettingsResponse {
    /// False when the server has no TypeSafe API key; toggles then have no effect.
    pub available: bool,
    pub approval_risk: bool,
    pub justification_check: bool,
    pub flag_kind: bool,
    pub nl_search: bool,
    /// RFC 3339; null when the team never saved settings.
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamAiSettingsRequest {
    pub approval_risk: bool,
    pub justification_check: bool,
    pub flag_kind: bool,
    pub nl_search: bool,
}

impl From<UpdateTeamAiSettingsRequest> for TeamAiSettings {
    fn from(value: UpdateTeamAiSettingsRequest) -> Self {
        Self {
            approval_risk: value.approval_risk,
            justification_check: value.justification_check,
            flag_kind: value.flag_kind,
            nl_search: value.nl_search,
        }
    }
}

fn settings_response(runtime: &AiRuntime, stored: &StoredTeamAiSettings) -> TeamAiSettingsResponse {
    TeamAiSettingsResponse {
        available: runtime.available(),
        approval_risk: stored.settings.approval_risk,
        justification_check: stored.settings.justification_check,
        flag_kind: stored.settings.flag_kind,
        nl_search: stored.settings.nl_search,
        updated_at: stored.updated_at.map(|at| at.to_rfc3339()),
    }
}

fn parse_team_id(raw: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(raw).map_err(|_| RestError::invalid_input("Invalid team_id"))
}

fn ensure_admin(req: &HttpRequest) -> Result<JwtUser, RestError> {
    let jwt = req
        .extensions()
        .get::<JwtUser>()
        .cloned()
        .ok_or_else(|| RestError::unauthorized("User authentication not found"))?;
    if !jwt.is_admin {
        return Err(RestError::forbidden(
            "Only system administrators can manage AI settings",
        ));
    }
    Ok(jwt)
}

#[utoipa::path(
    get,
    path = "/api/v1/teams/{team_id}/ai-settings",
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Team AI settings", body = TeamAiSettingsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[get("/teams/{team_id}/ai-settings")]
pub(crate) async fn get_team_ai_settings(
    runtime: web::Data<AiRuntime>,
    settings: web::Data<Box<dyn TeamAiSettingsRepository>>,
    team_id: web::Path<String>,
) -> Result<impl Responder, RestError> {
    let team_id = parse_team_id(&team_id)?;
    let stored = settings.get(team_id).await.map_err(RestError::from)?;
    Ok(HttpResponse::Ok().json(settings_response(&runtime, &stored)))
}

#[utoipa::path(
    put,
    path = "/api/v1/teams/{team_id}/ai-settings",
    request_body = UpdateTeamAiSettingsRequest,
    params(("team_id" = String, Path, description = "Team ID")),
    responses(
        (status = 200, description = "Updated team AI settings", body = TeamAiSettingsResponse),
        (status = 400, description = "Invalid input", body = crate::rest::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::rest::error::ErrorResponse),
        (status = 403, description = "Forbidden", body = crate::rest::error::ErrorResponse),
        (status = 404, description = "Team not found", body = crate::rest::error::ErrorResponse)
    ),
    tag = "AI"
)]
#[put("/teams/{team_id}/ai-settings")]
pub(crate) async fn update_team_ai_settings(
    runtime: web::Data<AiRuntime>,
    settings: web::Data<Box<dyn TeamAiSettingsRepository>>,
    activity: web::Data<Box<dyn ActivityLogRepository>>,
    req: HttpRequest,
    team_id: web::Path<String>,
    payload: web::Json<UpdateTeamAiSettingsRequest>,
) -> Result<impl Responder, RestError> {
    let jwt = ensure_admin(&req)?;
    let team_id = parse_team_id(&team_id)?;
    let previous = settings.get(team_id).await.map_err(RestError::from)?;
    let stored = settings
        .upsert(team_id, payload.into_inner().into(), Some(jwt.id))
        .await
        .map_err(RestError::from)?;

    if let Err(err) = log_team_activity(
        activity.get_ref(),
        activity_types::AI_SETTINGS_UPDATED,
        &team_id.to_string(),
        Some(jwt.id),
        Some(jwt.username.clone()),
        "AI settings updated".to_string(),
        Some(json!({ "old": previous.settings, "new": stored.settings })),
    )
    .await
    {
        warn!("Could not record AI settings activity for team {team_id}: {err}");
    }

    Ok(HttpResponse::Ok().json(settings_response(&runtime, &stored)))
}
```

Change `configure` to:

```rust
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(get_ai_status)
        .service(get_team_ai_settings)
        .service(update_team_ai_settings);
}
```

- [ ] **Step 5: Register in `ApiDoc`** — in `src/rest/mod.rs`, extend the import to `use crate::rest::ai::{AiStatusResponse, TeamAiSettingsResponse, UpdateTeamAiSettingsRequest};`, add `ai::get_team_ai_settings, ai::update_team_ai_settings,` after `ai::get_ai_status,` in `paths(...)`, and add `TeamAiSettingsResponse, UpdateTeamAiSettingsRequest,` after `AiStatusResponse,` in `components(schemas(...))`.

- [ ] **Step 6: Run the tests and confirm they pass**

Run: `cargo test -p feature-toggle-backend --lib rest::ai`
Expected: 7 passed.

- [ ] **Step 7: Update the contract baseline**

```bash
./scripts/export-contracts.sh
cp feature-toggle-backend/contracts/generated/contract-hashes.json feature-toggle-backend/contracts/baseline/contract-hashes.json
./scripts/check-contract-compat.sh
```

Expected: the compatibility check passes.

- [ ] **Step 8: Run the full backend gate** (DB must be migrated and seeded)

```bash
cargo fmt
cargo clippy -p feature-toggle-backend --all-targets
cargo test -p feature-toggle-backend
```

Expected: clippy clean for new code; all tests pass. If a test unrelated to AI fails, do not fix it here: record the test name and the failure line in the handoff log and tell the user.

- [ ] **Step 9: Update AI-01 docs** — in `docs/ai-judgments/tasks/AI-01-judgment-store-and-team-settings.md` set `| Status | Done in <sha of this task's code commit> |`, tick the verified acceptance boxes, and append:

```markdown
### 2026-10-02, Claude (plan 2026-10-02-ai-judgments-foundation)

- Changed: migration `20261002030000_ai_judgments.sql`; `database::ai` (`TeamAiSettingsRepository`, `AiJudgmentRepository`, unconditional automocks); `judgment::service` (`JudgmentHandler`, `JudgmentService`, `RunOutcome`, `input_hash`); `AiJudgmentRetryScheduler` (60 s, started only with a key); `GET`/`PUT /api/v1/teams/{team_id}/ai-settings` (PUT is system-admin only, logs `ai_settings_updated`); contract baseline.
- Verified: repository tests against Postgres, service tests with `MockJudgmentClient`, REST tests; full `cargo test -p feature-toggle-backend`.
- Deviations: `AiRuntime.judgments` holds `Option<Arc<JudgmentService>>`; register handlers with `JudgmentService::with_handler` before wrapping in `Arc` (in `lib.rs::run`). `mark_failed` never overwrites `done`. `updated_by` has no FK. PUT for an unknown team returns 404. Repository string args are owned (`String`) for mockall.
- Next: feature tasks build a handler, register it in `lib.rs::run`, and call `runtime.judgments` / `team_enabled(team_id, AiFeature::...)` before `submit`.
```

In `docs/ai-judgments/README.md`, tick the AI-01 row.

- [ ] **Step 10: Commit code, then docs, then refresh graphify**

```bash
git add feature-toggle-backend/src/rest/ai.rs feature-toggle-backend/src/rest/mod.rs feature-toggle-backend/src/utils/activity_logger.rs feature-toggle-backend/contracts/baseline/contract-hashes.json
git commit -m "feat(ai): add per-team AI settings API with audit trail

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git add docs/ai-judgments/tasks/AI-01-judgment-store-and-team-settings.md docs/ai-judgments/README.md
git commit -m "docs(ai): mark AI-01 done

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
graphify update .
```

`graphify-out/` is untracked in this repo; do not commit it.

---

## Part C — UI AI-02 (repo `feature-toggle-ui/`)

### Task 8: `api/ai.ts` and `useAiFeatures`

**Files:**
- Create: `src/api/ai.ts`
- Create: `src/hooks/useAiFeatures.ts`
- Test: `src/hooks/useAiFeatures.test.tsx`

**Interfaces:**
- Consumes: backend DTOs from Tasks 4 and 7; `apiJson` (`src/api/client.ts`); `useSharedQuery`, `invalidateSharedQuery`, `clearSharedQueryCache` (`src/hooks/useSharedQuery.ts`); test helper `renderHook`, `flushMicrotasks` (`src/test-utils/renderHook.tsx`).
- Produces: `AiStatus`, `TeamAiSettings`, `UpdateTeamAiSettingsInput`, `getAiStatus()`, `getTeamAiSettings(teamId)`, `updateTeamAiSettings(teamId, input)`; `AiFeatures`, `useAiFeatures(teamId?: string): AiFeatures`, `invalidateAiFeatures(teamId: string)`, `aiFeaturesKey(teamId: string)`.

- [ ] **Step 1: Write the API module** — create `src/api/ai.ts`:

```ts
import { apiJson } from './client';

export interface AiStatus {
  available: boolean;
  model: string | null;
}

export interface TeamAiSettings {
  available: boolean;
  approvalRisk: boolean;
  justificationCheck: boolean;
  flagKind: boolean;
  nlSearch: boolean;
  updatedAt: string | null;
}

export interface UpdateTeamAiSettingsInput {
  approvalRisk: boolean;
  justificationCheck: boolean;
  flagKind: boolean;
  nlSearch: boolean;
}

const teamAiSettingsPath = (teamId: string) =>
  `/teams/${encodeURIComponent(teamId)}/ai-settings`;

export const getAiStatus = async (): Promise<AiStatus> => {
  return apiJson<AiStatus>('/ai/status');
};

export const getTeamAiSettings = async (teamId: string): Promise<TeamAiSettings> => {
  return apiJson<TeamAiSettings>(teamAiSettingsPath(teamId));
};

export const updateTeamAiSettings = async (
  teamId: string,
  input: UpdateTeamAiSettingsInput,
): Promise<TeamAiSettings> => {
  return apiJson<TeamAiSettings>(teamAiSettingsPath(teamId), {
    method: 'PUT',
    body: input,
  });
};
```

- [ ] **Step 2: Write the failing hook tests** — create `src/hooks/useAiFeatures.test.tsx`:

```tsx
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook, flushMicrotasks } from '@/test-utils/renderHook';
import { clearSharedQueryCache } from '@/hooks/useSharedQuery';
import { getTeamAiSettings } from '@/api/ai';
import { useAiFeatures } from './useAiFeatures';

vi.mock('@/api/ai', () => ({
  getTeamAiSettings: vi.fn(),
}));

const allOff = {
  loading: false,
  available: false,
  approvalRisk: false,
  justificationCheck: false,
  flagKind: false,
  nlSearch: false,
};

describe('useAiFeatures', () => {
  const mockedGet = vi.mocked(getTeamAiSettings);

  beforeEach(() => {
    clearSharedQueryCache();
    mockedGet.mockReset();
  });

  it('maps the team settings when the subsystem is available', async () => {
    mockedGet.mockResolvedValue({
      available: true,
      approvalRisk: true,
      justificationCheck: false,
      flagKind: true,
      nlSearch: false,
      updatedAt: '2026-10-02T00:00:00Z',
    });

    const { result, unmount } = await renderHook(() => useAiFeatures('team-1'));
    await flushMicrotasks();
    await flushMicrotasks();

    expect(mockedGet).toHaveBeenCalledWith('team-1');
    expect(result.current).toEqual({
      loading: false,
      available: true,
      approvalRisk: true,
      justificationCheck: false,
      flagKind: true,
      nlSearch: false,
    });
    await unmount();
  });

  it('turns every feature off when the server has no key', async () => {
    mockedGet.mockResolvedValue({
      available: false,
      approvalRisk: true,
      justificationCheck: true,
      flagKind: true,
      nlSearch: true,
      updatedAt: null,
    });

    const { result, unmount } = await renderHook(() => useAiFeatures('team-2'));
    await flushMicrotasks();
    await flushMicrotasks();

    expect(result.current).toEqual(allOff);
    await unmount();
  });

  it('returns all false on a request error', async () => {
    mockedGet.mockRejectedValue(new Error('boom'));

    const { result, unmount } = await renderHook(() => useAiFeatures('team-3'));
    await flushMicrotasks();
    await flushMicrotasks();

    expect(result.current).toEqual(allOff);
    await unmount();
  });

  it('does not fetch without a team', async () => {
    const { result, unmount } = await renderHook(() => useAiFeatures(undefined));
    await flushMicrotasks();

    expect(mockedGet).not.toHaveBeenCalled();
    expect(result.current).toEqual(allOff);
    await unmount();
  });
});
```

- [ ] **Step 3: Run the tests and confirm they fail**

Run (from `feature-toggle-ui/`): `pnpm vitest run src/hooks/useAiFeatures.test.tsx`
Expected: FAIL, `Failed to resolve import "./useAiFeatures"`.

- [ ] **Step 4: Implement the hook** — create `src/hooks/useAiFeatures.ts`:

```ts
import { useCallback } from 'react';
import { getTeamAiSettings } from '@/api/ai';
import { invalidateSharedQuery, useSharedQuery } from '@/hooks/useSharedQuery';

export interface AiFeatures {
  loading: boolean;
  available: boolean;
  approvalRisk: boolean;
  justificationCheck: boolean;
  flagKind: boolean;
  nlSearch: boolean;
}

const AI_SETTINGS_STALE_TIME_MS = 60_000;

const ALL_OFF: Omit<AiFeatures, 'loading'> = {
  available: false,
  approvalRisk: false,
  justificationCheck: false,
  flagKind: false,
  nlSearch: false,
};

export const aiFeaturesKey = (teamId: string) => `ai-settings:${teamId}`;

/**
 * Whether each AI feature is on for the team. A feature counts as on only when
 * the server has a TypeSafe key and the team toggle is on. Any error turns
 * everything off so AI UI hides.
 */
export function useAiFeatures(teamId?: string): AiFeatures {
  const queryFn = useCallback(() => getTeamAiSettings(teamId ?? ''), [teamId]);
  const { data, error, loading } = useSharedQuery({
    key: aiFeaturesKey(teamId ?? 'none'),
    queryFn,
    enabled: Boolean(teamId),
    staleTime: AI_SETTINGS_STALE_TIME_MS,
  });

  if (!teamId || error || !data) {
    return { loading: Boolean(teamId) && !error && loading, ...ALL_OFF };
  }

  const on = data.available;
  return {
    loading: false,
    available: on,
    approvalRisk: on && data.approvalRisk,
    justificationCheck: on && data.justificationCheck,
    flagKind: on && data.flagKind,
    nlSearch: on && data.nlSearch,
  };
}

export const invalidateAiFeatures = (teamId: string) => {
  invalidateSharedQuery(aiFeaturesKey(teamId));
};
```

- [ ] **Step 5: Run the tests and confirm they pass**

Run: `pnpm vitest run src/hooks/useAiFeatures.test.tsx`
Expected: 4 passed.

- [ ] **Step 6: Commit** (UI repo)

```bash
git add src/api/ai.ts src/hooks/useAiFeatures.ts src/hooks/useAiFeatures.test.tsx
git commit -m "feat(ai): add AI settings API client and useAiFeatures hook

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 9: AI settings page, route, nav

**Files:**
- Create: `src/pages/AiSettingsPage.tsx`
- Test: `src/pages/__tests__/AiSettingsPage.test.tsx`
- Modify: `src/layout/navConfig.ts`, `src/layout/__tests__/navConfig.test.ts`
- Modify: `src/routes/AppShellRoutes.tsx`, `src/routes/routeModules.ts`

**Interfaces:**
- Consumes: `getTeamAiSettings`, `updateTeamAiSettings`, `UpdateTeamAiSettingsInput`, `TeamAiSettings` (Task 8); `invalidateAiFeatures` (Task 8); `useAuth().isAdmin` (`@/hooks/useAuth`); `useTeamContext().selectedTeam` (`@/contexts/TeamContext`); `PageHeader`, `Button`, `Switch`, `Label` (`@/components/ui/*`).
- Produces: default export `AiSettingsPage`; route `/settings/ai`; nav item id `ai`, label `AI assistance`, gate `admin`.

- [ ] **Step 1: Write the failing page tests** — create `src/pages/__tests__/AiSettingsPage.test.tsx`:

```tsx
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ButtonHTMLAttributes, InputHTMLAttributes, LabelHTMLAttributes, ReactNode } from 'react';
import AiSettingsPage from '../AiSettingsPage';

const {
  mockGetTeamAiSettings,
  mockUpdateTeamAiSettings,
  mockInvalidateAiFeatures,
  mockUseAuth,
  mockUseTeamContext,
} = vi.hoisted(() => ({
  mockGetTeamAiSettings: vi.fn(),
  mockUpdateTeamAiSettings: vi.fn(),
  mockInvalidateAiFeatures: vi.fn(),
  mockUseAuth: vi.fn(),
  mockUseTeamContext: vi.fn(),
}));

function forwardMock<A extends unknown[], R>(mock: (...args: A) => R) {
  return (...args: A) => mock(...args);
}

vi.mock('@/api/ai', () => ({
  getTeamAiSettings: forwardMock(mockGetTeamAiSettings),
  updateTeamAiSettings: forwardMock(mockUpdateTeamAiSettings),
}));

vi.mock('@/hooks/useAiFeatures', () => ({
  invalidateAiFeatures: forwardMock(mockInvalidateAiFeatures),
}));

vi.mock('@/hooks/useAuth', () => ({
  useAuth: () => mockUseAuth(),
}));

vi.mock('@/contexts/TeamContext', () => ({
  useTeamContext: () => mockUseTeamContext(),
}));

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock('@/components/ui/page-header', () => ({
  PageHeader: ({ title, description }: { title: string; description?: string }) => (
    <div>
      <h1>{title}</h1>
      <p>{description}</p>
    </div>
  ),
}));

vi.mock('@/components/ui/button', () => ({
  Button: ({ children, ...props }: { children?: ReactNode } & ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}));

vi.mock('@/components/ui/label', () => ({
  Label: ({ children, ...props }: { children?: ReactNode } & LabelHTMLAttributes<HTMLLabelElement>) => (
    <label {...props}>{children}</label>
  ),
}));

vi.mock('@/components/ui/switch', () => ({
  Switch: ({
    checked,
    onCheckedChange,
    ...props
  }: { checked?: boolean; onCheckedChange?: (checked: boolean) => void } & InputHTMLAttributes<HTMLInputElement>) => (
    <input
      type="checkbox"
      checked={!!checked}
      onChange={(event) => onCheckedChange?.(event.target.checked)}
      {...props}
    />
  ),
}));

const settings = (overrides: Record<string, unknown> = {}) => ({
  available: true,
  approvalRisk: false,
  justificationCheck: false,
  flagKind: false,
  nlSearch: false,
  updatedAt: null,
  ...overrides,
});

describe('AiSettingsPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockUseAuth.mockReturnValue({ isAdmin: true });
    mockUseTeamContext.mockReturnValue({ selectedTeam: { id: 'team-1', name: 'Payments' } });
    mockGetTeamAiSettings.mockResolvedValue(settings());
  });

  it('shows the data notice', async () => {
    render(<AiSettingsPage />);
    expect(
      await screen.findByText(/FluxGate sends flag keys, descriptions, purposes, tags, change diffs, and free-text reasons to TypeSafe/),
    ).toBeInTheDocument();
  });

  it('disables the switches and explains when the server has no key', async () => {
    mockGetTeamAiSettings.mockResolvedValue(settings({ available: false }));
    render(<AiSettingsPage />);

    expect(
      await screen.findByText('AI judgments are not configured on this server (TYPESAFE_API_KEY not set)'),
    ).toBeInTheDocument();
    expect(screen.getByLabelText('Approval risk triage')).toBeDisabled();
    expect(screen.getByLabelText('Natural-language search')).toBeDisabled();
    expect(screen.getByRole('button', { name: /save/i })).toBeDisabled();
  });

  it('saves the toggled settings and refreshes the shared cache', async () => {
    mockUpdateTeamAiSettings.mockResolvedValue(settings({ approvalRisk: true }));
    render(<AiSettingsPage />);

    const approvalRisk = await screen.findByLabelText('Approval risk triage');
    await waitFor(() => expect(approvalRisk).not.toBeDisabled());
    fireEvent.click(approvalRisk);
    fireEvent.click(screen.getByRole('button', { name: /save/i }));

    await waitFor(() =>
      expect(mockUpdateTeamAiSettings).toHaveBeenCalledWith('team-1', {
        approvalRisk: true,
        justificationCheck: false,
        flagKind: false,
        nlSearch: false,
      }),
    );
    await waitFor(() => expect(mockInvalidateAiFeatures).toHaveBeenCalledWith('team-1'));
  });

  it('shows a load error instead of the not-configured message', async () => {
    mockGetTeamAiSettings.mockRejectedValue(new Error('Server error'));
    render(<AiSettingsPage />);

    expect(await screen.findByText('Could not load AI settings for this team.')).toBeInTheDocument();
    expect(
      screen.queryByText('AI judgments are not configured on this server (TYPESAFE_API_KEY not set)'),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText('Approval risk triage')).toBeDisabled();
  });

  it('asks for a team when none is selected', async () => {
    mockUseTeamContext.mockReturnValue({ selectedTeam: null });
    render(<AiSettingsPage />);

    expect(await screen.findByText('Select a team to manage its AI settings.')).toBeInTheDocument();
    expect(mockGetTeamAiSettings).not.toHaveBeenCalled();
  });

  it('denies non-admins like the notification settings page', () => {
    mockUseAuth.mockReturnValue({ isAdmin: false });
    render(<AiSettingsPage />);

    expect(screen.getByText('Access Denied')).toBeInTheDocument();
    expect(screen.getByText('Only system administrators can manage AI settings.')).toBeInTheDocument();
    expect(mockGetTeamAiSettings).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `pnpm vitest run src/pages/__tests__/AiSettingsPage.test.tsx`
Expected: FAIL, `Failed to resolve import "../AiSettingsPage"`.

- [ ] **Step 3: Implement the page** — create `src/pages/AiSettingsPage.tsx`:

```tsx
import { useEffect, useState } from 'react';
import { Info, Save, ShieldAlert, Sparkles } from 'lucide-react';
import { toast } from 'sonner';

import { useAuth } from '@/hooks/useAuth';
import { useTeamContext } from '@/contexts/TeamContext';
import { PageHeader } from '@/components/ui/page-header';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import {
  getTeamAiSettings,
  updateTeamAiSettings,
  type TeamAiSettings,
  type UpdateTeamAiSettingsInput,
} from '@/api/ai';
import { invalidateAiFeatures } from '@/hooks/useAiFeatures';

type FeatureKey = keyof UpdateTeamAiSettingsInput;

const FEATURES: { key: FeatureKey; label: string; description: string }[] = [
  {
    key: 'approvalRisk',
    label: 'Approval risk triage',
    description: 'Score each approval request so reviewers see risky production changes first.',
  },
  {
    key: 'justificationCheck',
    label: 'Reason quality check',
    description: 'Warn when an override, freeze, schedule, or cleanup reason is vague. Never blocks.',
  },
  {
    key: 'flagKind',
    label: 'Flag kind classification',
    description: 'Classify flags as release, experiment, ops, permission, or config.',
  },
  {
    key: 'nlSearch',
    label: 'Natural-language search',
    description: 'Find flags with a plain-English query in the command palette.',
  },
];

const DATA_NOTICE =
  'When on, FluxGate sends flag keys, descriptions, purposes, tags, change diffs, and free-text reasons to TypeSafe (api.typesafe.ai) for evaluation. Natural-language search also sends the team\'s tag and owner names. TypeSafe does not train on this data.';

const NOT_CONFIGURED = 'AI judgments are not configured on this server (TYPESAFE_API_KEY not set)';

const ALL_OFF: UpdateTeamAiSettingsInput = {
  approvalRisk: false,
  justificationCheck: false,
  flagKind: false,
  nlSearch: false,
};

const toInput = (settings: TeamAiSettings): UpdateTeamAiSettingsInput => ({
  approvalRisk: settings.approvalRisk,
  justificationCheck: settings.justificationCheck,
  flagKind: settings.flagKind,
  nlSearch: settings.nlSearch,
});

const errorMessage = (error: unknown, fallback: string) =>
  error instanceof Error ? error.message : fallback;

export default function AiSettingsPage() {
  const { isAdmin } = useAuth();
  const { selectedTeam } = useTeamContext();
  const teamId = selectedTeam?.id;

  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState(false);
  const [saving, setSaving] = useState(false);
  const [available, setAvailable] = useState(false);
  const [values, setValues] = useState<UpdateTeamAiSettingsInput>(ALL_OFF);
  const [saved, setSaved] = useState<UpdateTeamAiSettingsInput>(ALL_OFF);

  useEffect(() => {
    if (!isAdmin || !teamId) {
      setLoading(false);
      return;
    }
    let active = true;
    setLoading(true);
    setLoadError(false);
    getTeamAiSettings(teamId)
      .then((settings) => {
        if (!active) return;
        const next = toInput(settings);
        setAvailable(settings.available);
        setValues(next);
        setSaved(next);
      })
      .catch((error: unknown) => {
        if (!active) return;
        setLoadError(true);
        setAvailable(false);
        toast.error('Failed to load AI settings', {
          description: errorMessage(error, 'Request failed'),
        });
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [isAdmin, teamId]);

  const dirty = FEATURES.some(({ key }) => values[key] !== saved[key]);
  const editable = available && !loadError && !loading && !saving;

  const handleSave = async () => {
    if (!teamId) return;
    setSaving(true);
    try {
      const settings = await updateTeamAiSettings(teamId, values);
      const next = toInput(settings);
      setAvailable(settings.available);
      setValues(next);
      setSaved(next);
      invalidateAiFeatures(teamId);
      toast.success('AI settings saved');
    } catch (error) {
      toast.error('Failed to save AI settings', {
        description: errorMessage(error, 'Request failed'),
      });
    } finally {
      setSaving(false);
    }
  };

  if (!isAdmin) {
    return (
      <div className="page-container">
        <div className="content-card flex flex-col items-center justify-center py-16">
          <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-lg border border-destructive/20 bg-destructive/10 text-destructive">
            <ShieldAlert className="h-7 w-7" />
          </div>
          <h1 className="text-2xl font-semibold text-foreground mb-2">Access Denied</h1>
          <p className="text-muted-foreground">Only system administrators can manage AI settings.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="page-container">
      <PageHeader
        title="AI assistance"
        description={
          selectedTeam
            ? `TypeSafe Jev judgments for ${selectedTeam.name}. Each feature is off until you turn it on.`
            : 'TypeSafe Jev judgments per team. Each feature is off until you turn it on.'
        }
        icon={<Sparkles className="w-6 h-6" />}
        iconGradient="purple"
      />

      <div className="space-y-6">
        <div className="content-card flex gap-3 border border-border bg-muted/40">
          <Info className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
          <p className="text-sm text-muted-foreground">{DATA_NOTICE}</p>
        </div>

        {!teamId ? (
          <div className="content-card text-muted-foreground">Select a team to manage its AI settings.</div>
        ) : (
          <div className="content-card space-y-5">
            {loading && <p className="text-sm text-muted-foreground">Loading AI settings...</p>}
            {!loading && loadError && (
              <p className="rounded-md border border-destructive/20 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                Could not load AI settings for this team.
              </p>
            )}
            {!loading && !loadError && !available && (
              <p className="rounded-md border border-warning/25 bg-warning/10 px-3 py-2 text-sm text-warning">
                {NOT_CONFIGURED}
              </p>
            )}

            <ul className="divide-y divide-border">
              {FEATURES.map(({ key, label, description }) => {
                const id = `ai-setting-${key}`;
                return (
                  <li key={key} className="flex items-start justify-between gap-4 py-4 first:pt-0 last:pb-0">
                    <div className="space-y-1">
                      <Label htmlFor={id} className="text-sm font-medium text-foreground">
                        {label}
                      </Label>
                      <p className="text-sm text-muted-foreground">{description}</p>
                    </div>
                    <Switch
                      id={id}
                      aria-label={label}
                      checked={values[key]}
                      disabled={!editable}
                      onCheckedChange={(checked: boolean) =>
                        setValues((previous) => ({ ...previous, [key]: checked }))
                      }
                    />
                  </li>
                );
              })}
            </ul>

            <div className="flex justify-end">
              <Button onClick={handleSave} disabled={!editable || !dirty}>
                <Save className="mr-2 h-4 w-4" />
                {saving ? 'Saving...' : 'Save'}
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run the page tests and confirm they pass**

Run: `pnpm vitest run src/pages/__tests__/AiSettingsPage.test.tsx`
Expected: 6 passed. If `getByLabelText` finds two elements (the `<label htmlFor>` and `aria-label` both match), keep `aria-label` and the label: Testing Library returns the single input both point at.

- [ ] **Step 5: Register the route** — in `src/routes/routeModules.ts`, add `aiSettingsPage: () => import('@/pages/AiSettingsPage'),` after the `notificationSettingsPage` loader, and `'/settings/ai': ['aiSettingsPage'],` after the `'/settings/notifications'` entry. In `src/routes/AppShellRoutes.tsx`, add `const AiSettingsPage = lazyRoute('aiSettingsPage');` after the `NotificationSettingsPage` line and `<Route path="/settings/ai" element={<AiSettingsPage />} />` after the `/settings/notifications` route.

- [ ] **Step 6: Add the nav item and its test** — in `src/layout/navConfig.ts`, add `Sparkles,` to the `lucide-react` import list (keep it alphabetical) and add this item to the Settings group after the `notifications` item:

```ts
      { id: 'ai', path: '/settings/ai', label: 'AI assistance', icon: Sparkles, gate: 'admin' },
```

In `src/layout/__tests__/navConfig.test.ts`, in the admin test that asserts `expect.arrayContaining(['Roles', 'Notifications', 'Single Sign-On'])`, change the array to `['Roles', 'Notifications', 'AI assistance', 'Single Sign-On']`. In the team-managers test (the one titled `'team managers see governance and settings but not admin-only items'`), add:

```ts
    expect(visible).not.toContain('AI assistance');
```

(Read the test first; `visible` is the label list it already builds.)

- [ ] **Step 7: Run the UI gate** (from `feature-toggle-ui/`)

```bash
pnpm lint
pnpm build
pnpm test:run
```

Expected: lint clean, type check and build succeed, all tests pass including `designTokenGuard.test.ts` and `routeModules.test.ts`.

- [ ] **Step 8: Commit** (UI repo; do not stage `src/pages/FeatureDetail.tsx`)

```bash
git add src/pages/AiSettingsPage.tsx src/pages/__tests__/AiSettingsPage.test.tsx src/layout/navConfig.ts src/layout/__tests__/navConfig.test.ts src/routes/AppShellRoutes.tsx src/routes/routeModules.ts
git commit -m "feat(ai): add AI assistance settings page at /settings/ai

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
graphify update .
```

### Task 10: End-to-end check against a running backend and AI-02 docs

**Files:**
- Modify: `feature-toggle/docs/ai-judgments/tasks/AI-02-ui-ai-foundation-and-settings.md`, `feature-toggle/docs/ai-judgments/README.md`

- [ ] **Step 1: Start the backend without a key on spare ports** (from `feature-toggle/`). Write a scratch config to the session scratchpad, not the repo:

```bash
SCRATCH=/private/tmp/claude-501/-Users-kasunranasinghe-Projects-FeatureToggle/2cdd6f2b-5bd8-4a2f-9d8c-ecad5e9895dd/scratchpad
cat > "$SCRATCH/ai-e2e.toml" <<'EOF'
allowed_origin = "http://localhost:5173"
http_addr = "127.0.0.1:18180"
grpc_addr = "127.0.0.1:15151"
EOF
env -u TYPESAFE_API_KEY FEATURE_TOGGLE_CONFIG="$SCRATCH/ai-e2e.toml" cargo run -p feature-toggle-backend > "$SCRATCH/backend-nokey.log" 2>&1
```

Run this command as a background job (the Bash tool's `run_in_background`), not with a trailing `&`.

Wait until `curl -s http://127.0.0.1:18180/api/v1/health` returns 200. Check `grep "TypeSafe judgments disabled" "$SCRATCH/backend-nokey.log"`.

- [ ] **Step 2: Call the endpoints as the seed admin**

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:18180/api/v1/auth/login -H 'Content-Type: application/json' \
  -d '{"username":"api-test-admin","password":"password123"}' | jq -r '.token')
curl -s http://127.0.0.1:18180/api/v1/ai/status -H "Authorization: Bearer $TOKEN"
TEAM=$(curl -s http://127.0.0.1:18180/api/v1/teams -H "Authorization: Bearer $TOKEN" | jq -r '.[0].id')
curl -s http://127.0.0.1:18180/api/v1/teams/$TEAM/ai-settings -H "Authorization: Bearer $TOKEN"
```

Expected: `{"available":false,"model":null}` and settings with `"available":false`. If the seed admin login fails on the local DB, stop and ask the user for credentials; do not create users.

- [ ] **Step 3: Restart with the key and verify persistence**

Stop the first process (`kill %1` or by PID). Start again without `env -u` (the key is in the shell env) writing to `backend-key.log`. Then:

```bash
grep "TypeSafe judgments enabled (model jev-1.13.0)" "$SCRATCH/backend-key.log"
grep -c "$TYPESAFE_API_KEY" "$SCRATCH/backend-key.log"   # must print 0
curl -s http://127.0.0.1:18180/api/v1/ai/status -H "Authorization: Bearer $TOKEN"
curl -s -X PUT http://127.0.0.1:18180/api/v1/teams/$TEAM/ai-settings -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"approvalRisk":true,"justificationCheck":false,"flagKind":false,"nlSearch":false}'
curl -s http://127.0.0.1:18180/api/v1/teams/$TEAM/ai-settings -H "Authorization: Bearer $TOKEN"
```

Expected: status `{"available":true,"model":"jev-1.13.0"}`; GET after PUT shows `approvalRisk: true` and a non-null `updatedAt`. Then reset the team with a PUT of all `false`, so the local DB is left as found. Stop the backend.

- [ ] **Step 4: Optional UI smoke** — if the user wants it, run the UI dev server against the backend on 18180 and open `/settings/ai` in the browser; otherwise skip and say so in the handoff log.

- [ ] **Step 5: Update AI-02 docs** — set `| Status | Done in <UI commit sha> (feature-toggle-ui) |`, tick verified acceptance boxes (`pnpm lint`, `pnpm build`, `pnpm test:run` replace the npm commands), and append:

```markdown
### 2026-10-02, Claude (plan 2026-10-02-ai-judgments-foundation)

- Changed (UI repo): `api/ai.ts`, `hooks/useAiFeatures.ts` (feature on only when `available` and the toggle are both true; errors mean all off), `pages/AiSettingsPage.tsx` (system-admin only, team from `TeamContext`, data notice, not-configured and load-error states), route `/settings/ai`, Settings nav item "AI assistance" (admin gate).
- Verified: `pnpm lint`, `pnpm build`, `pnpm test:run`; backend run without a key returns `available: false`; with a key, toggles persist across requests.
- Note: UI commands use pnpm, not npm.
- Next: AI-12, AI-21, AI-32, AI-41 call `useAiFeatures(selectedTeam?.id)` and hide their UI when the flag is false.
```

Tick AI-02 in `README.md`.

- [ ] **Step 6: Commit docs** (backend repo)

```bash
git add docs/ai-judgments/tasks/AI-02-ui-ai-foundation-and-settings.md docs/ai-judgments/README.md
git commit -m "docs(ai): mark AI-02 done

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
