# Design: AI judgments in FluxGate (TypeSafe Jev)

| Field | Value |
|---|---|
| Status | Approved design, not implemented |
| Date | 2026-10-01 |
| Repos | `feature-toggle/` (backend, this repo) and `feature-toggle-ui/` (separate git repo) |
| Code baseline | backend `fc50086`, UI `bae1962` (branch `feat/rtk-redesign`) |
| External service | TypeSafe System One API, model `jev-1.13.0` (https://docs.typesafe.ai) |

## 1. Goal

Add four control-plane features that use TypeSafe's Jev model for narrow, typed judgments:

1. **Approval risk triage**: score each approval request and, by per-policy choice, block auto-approval or require one extra approver for high-risk changes.
2. **Justification check**: warn when an override, freeze, schedule, or cleanup reason is vague. Never block.
3. **Flag kind classification**: classify flags as release, experiment, ops, permission, or config. Permanent kinds skip the evaluation-based stale rules.
4. **Natural-language search**: map a plain-English query in the command palette to existing list filters, then rerank by relevance.

### What Jev is, and the limits this design respects

Jev takes a JSON `state` plus typed questions and returns typed answers with probabilities:

- **Noul**: probability that a yes/no condition holds.
- **Choice**: one option from a set, plus a probability for each option and a confidence.
- **Score**: a probability-weighted position on ordered levels, plus a confidence.

Jev does **not** generate text, is weak at arithmetic, counting, and date comparison, and loses accuracy when the state holds irrelevant detail. User text in the state can try to steer it. Therefore:

- Code computes all numbers and buckets. The model never sees raw counts.
- Code owns every threshold and policy. The model only supplies probabilities.
- Reasons shown to users are fixed strings chosen by code, never model output.
- The model can only **add friction** (warn, block auto-approve, add an approver). It can never approve, enable, or skip a check.
- Every feature **fails open**: if the API is unavailable, the product behaves exactly as it does today.

### Non-goals

- No AI on the flag evaluation path (evaluation engine, edge server, OFREP). Evaluation stays deterministic and offline-capable.
- No generated text (change descriptions, release notes). Jev cannot generate.
- No changes to `proto/evaluation.proto` or the `FeatureUpdate` broadcast.
- No per-team API keys or billing. One server-wide key.

## 2. Decisions (made with the maintainer, 2026-10-01)

| # | Decision |
|---|---|
| D1 | Scope: all four features above. |
| D2 | Approval risk action is configured per approval policy: `ai_risk_mode = off \| advisory \| gate_auto_approve \| require_extra_approver`, default `advisory`. |
| D3 | Jev outage: fail open. Auto-approve proceeds as today; the UI shows "assessment unavailable". |
| D4 | Enablement: env `TYPESAFE_API_KEY` turns the subsystem on server-wide; a per-team settings row turns each feature on per team. All team toggles default to off. |
| D5 | Justification check warns and allows submit. The result is stored with the audit trail. |
| D6 | Flag kind is a column. AI fills it when empty; a user choice wins and AI never overwrites it. |
| D7 | Approval risk runs asynchronously after the request is created. Request creation latency does not change. |
| D8 | NL search is a backend endpoint (filters + rerank). The API key never reaches the browser. |

## 3. Architecture

```
                         feature-toggle-backend
 REST handlers ──► logic (approval, feature, operational_safety)
                        │  submit(kind, subject, input)
                        ▼
                 judgment::JudgmentService ──► ai_judgments table (pending → done/failed)
                        │  tokio::spawn                 ▲
                        ▼                               │ retry sweep (60 s scheduler)
                 judgment::JudgmentClient (trait, automock)
                        │  reqwest, timeout, retries, semaphore(16)
                        ▼
               POST https://api.typesafe.ai/v1/systemone
```

Two call styles:

- **Async judgments** (approval risk, post-submit justification, flag kind): `JudgmentService::submit` writes a `pending` row with the input snapshot, spawns the call, then writes `done` with raw answers and derived result, and runs a kind-specific `apply` step (for example set `flag_kind`). Failures write `failed`. A scheduler retries.
- **Sync judgments** (justification pre-check, feature suggestions, NL search): the REST handler calls `JudgmentClient` directly with the configured timeout. On any error it returns HTTP 200 with `available: false` so the UI degrades quietly.

### 3.1 Module layout (new)

```
feature-toggle-backend/src/judgment/
  mod.rs            // pub use; JudgmentKind, SubjectType enums
  config.rs         // TypesafeConfig (+ env key resolution)
  client.rs         // JudgmentClient trait (#[automock]) + HttpJudgmentClient
  types.rs          // SystemOneRequest, Question (Noul/Choice/Score), answers
  service.rs        // JudgmentService, JudgmentHandler trait, retry sweep fn
  approval_risk.rs  // AI-10
  justification.rs  // AI-20
  flag_kind.rs      // AI-30
  nl_search.rs      // AI-40
src/database/ai.rs  // AiJudgmentRepository, TeamAiSettingsRepository (automock)
src/rest/ai.rs      // /ai/status, /teams/{id}/ai-settings, sync AI endpoints
src/scheduler/ai_judgment_retry.rs
```

Follow existing conventions: `#[automock] #[async_trait]` repository traits with `clone_box`, factory functions `x_repository(pool) -> Box<dyn XRepository>`, wiring by hand in `lib.rs::run`, sharing through `web::Data`, and runtime `sqlx::query(...)` (no new `query!` macros, so no `.sqlx` changes).

### 3.2 Configuration

New optional TOML section, read by `Config::load()` (`src/config.rs`):

```toml
[typesafe]
base_url = "https://api.typesafe.ai"   # default
model = "jev-1.13.0"                   # pinned; do not use jev-latest (alias moves)
timeout_ms = 3000                      # default
max_in_flight = 16                     # default
```

- Field: `#[serde(default)] pub typesafe: TypesafeConfig` on `Config`, with `Default`, the same way `cluster` works.
- The API key comes **only** from env `TYPESAFE_API_KEY`. It is never read from TOML, never logged, never returned by an endpoint.
- No key means the subsystem is unavailable. `JudgmentService` and the client are not constructed; every AI path is skipped; `GET /ai/status` returns `{available: false}`.
- Existing problem: a TOML parse error silently replaces the whole config with defaults (`config.rs`, `Err(e) => warn!(...)`). A new section makes a typo more likely. AI-00 changes that log to `error!` and includes the parse error. It does not change the fallback behavior.

### 3.3 TypeSafe client

`trait JudgmentClient { async fn evaluate(&self, req: SystemOneRequest) -> Result<SystemOneResponse, JudgmentError>; }`

- HTTP: `POST {base_url}/v1/systemone`, `Authorization: Bearer <key>`, JSON body `{state, model, questions}`.
- Timeout per attempt: `timeout_ms`. Retries: at most 2, only on 429, 529, and connection errors. Exponential backoff starting at 250 ms, honoring a `retry-after` header (cap 5 s). No retry on 401 or 422.
- Concurrency: a `tokio::sync::Semaphore` with `max_in_flight` permits, shared by all callers.
- `JudgmentError` variants: `Unavailable` (no key), `Timeout`, `RateLimited`, `Http(status, body_snippet)`, `Decode(String)`.
- Logging (log crate): info line per call with question count, model, `input_tokens`, latency. Never log state content, reasons, or the key.
- The response's `model` field is stored on each judgment row.

Wire types (subset of https://docs.typesafe.ai/api.md):

```jsonc
// request
{ "state": { ... }, "model": "jev-1.13.0",
  "questions": {
    "q_id": { "type": "noul",   "instructions": "...", "criteria": { "true": "...", "false": "..." } },
    "q_id": { "type": "choice", "instructions": "...", "criteria": { "opt_a": "...", "opt_b": null } },
    "q_id": { "type": "score",  "instructions": "...", "criteria": ["level 0", "level 1", "level 2"] } } }
// response
{ "model": "jev-1.13.0",
  "answers": {
    "q_id": { "type": "noul", "noul": 0.95 },
    "q_id": { "type": "choice", "choice": "opt_a", "probabilities": { "opt_a": 0.9, "opt_b": 0.1 }, "confidence": 0.8 },
    "q_id": { "type": "score", "score": 1.4, "legend": { "0": "..." }, "probabilities": { "0": 0.1 }, "confidence": 0.7 } },
  "usage": { "input_tokens": 300, "output_tokens": 20 } }
```

Limits that builders must respect: at most 255 options per Choice, 2 to 10 levels per Score, 32k tokens for state plus the longest question, 64k tokens per request. Instructions may be a JSON object; refer to state paths in backticks, for example `` `change.diff` ``.

### 3.4 JudgmentService

```rust
pub enum JudgmentKind { ApprovalRisk, Justification, FlagKind }
pub enum SubjectType { ApprovalRequest, Feature, Activity, FreezeWindow, ScheduledChange }

#[async_trait]
pub trait JudgmentHandler: Send + Sync {
    fn kind(&self) -> JudgmentKind;
    /// Build state + questions from the stored input snapshot. Pure.
    fn build(&self, input: &serde_json::Value) -> SystemOneRequestParts;
    /// Turn raw answers into the derived result. Pure.
    fn derive(&self, input: &serde_json::Value, answers: &Answers) -> serde_json::Value;
    /// Side effects after `done` (may be a no-op). Must check freshness.
    async fn apply(&self, judgment: &AiJudgment) -> Result<(), Error>;
}
```

- `submit(team_id, kind, subject_type, subject_id, input)`: upsert the row on `(subject_type, subject_id, kind)` with `status = pending`, `attempts = 0`, new `input` and `input_hash` (SHA-256 of canonical input JSON, `sha2` is already a dependency). Then `tokio::spawn` the run.
- Run: call the client; on success write `raw_answers`, `derived`, `model`, `input_tokens`, `status = done`, `completed_at` with `WHERE id = $1 AND input_hash = $2` (a newer submission wins), then call `apply`. On error write `status = failed`, `error`, `attempts + 1` with the same guard.
- Retry sweep (`scheduler/ai_judgment_retry.rs`, every 60 s, started in `lib.rs::run` like the other schedulers): rows with `status = pending AND created_at < now() - 2 min` or `status = failed AND attempts < 3`, oldest first, at most 50 per tick. This also recovers work lost on restart.
- Pure `build`/`derive` functions make unit tests easy and let a threshold change be re-derived from stored `raw_answers` without new API calls.

## 4. Data model

All new migrations use the repo convention `migrations/YYYYMMDDHHMMSS_snake_name.sql` with timestamps after `20260610000000`. Never edit an applied migration.

### 4.1 `team_ai_settings` (AI-01)

```sql
CREATE TABLE team_ai_settings (
  team_id              UUID PRIMARY KEY REFERENCES teams(id) ON DELETE CASCADE,
  approval_risk        BOOLEAN NOT NULL DEFAULT FALSE,
  justification_check  BOOLEAN NOT NULL DEFAULT FALSE,
  flag_kind            BOOLEAN NOT NULL DEFAULT FALSE,
  nl_search            BOOLEAN NOT NULL DEFAULT FALSE,
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_by           UUID NULL
);
```

A missing row means all false. Check the actual key type of `teams.id` and of user ids in existing migrations and match them.

### 4.2 `ai_judgments` (AI-01)

```sql
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

### 4.3 Approval columns (AI-10, AI-11)

```sql
ALTER TABLE approval_policies ADD COLUMN ai_risk_mode TEXT NOT NULL DEFAULT 'advisory'
  CHECK (ai_risk_mode IN ('off','advisory','gate_auto_approve','require_extra_approver'));   -- AI-10
ALTER TABLE approval_requests ADD COLUMN required_approvers_override INT NULL;               -- AI-11
```

### 4.4 Feature columns (AI-30)

```sql
ALTER TABLE features
  ADD COLUMN flag_kind TEXT NULL
    CHECK (flag_kind IN ('release','experiment','ops','permission','config')),
  ADD COLUMN flag_kind_source TEXT NULL CHECK (flag_kind_source IN ('ai','user')),
  ADD COLUMN flag_kind_confidence REAL NULL;
```

These columns do not affect evaluation, so no broadcast and no proto change.

## 5. Feature specs

Thresholds below are **starting values**. Each backend task adds a labelled fixture set and a live `#[ignore]` test to tune them (section 7). Change a threshold only through a named constant.

### 5.1 Approval risk triage (AI-10, AI-11, AI-12)

**Trigger.** In `ApprovalLogicImpl::maybe_create_stage_change_request` (`logic/approval.rs`), right after `create_request` returns `Ok`, if the subsystem is available, the team setting `approval_risk` is on, and `policy.ai_risk_mode != 'off'`: call `JudgmentService::submit(team, ApprovalRisk, ApprovalRequest, request.id, input)`. This is today the only place approval requests are created (`change_type = "stage_change"`).

**Input snapshot** (built by code from `change_payload`, the feature, and the stage):

```jsonc
{
  "feature": { "key": "...", "description": "...", "purpose": "...", "tags": ["..."], "flag_kind": "ops|null" },
  "change": {
    "type": "stage_change",
    "environment_name": "prod-eu",
    "environment_type": "production|development",
    "from_status": "...", "to_status": "...",
    "diff": [ { "path": "...", "change_type": "added|removed|changed", "before": "...", "after": "..." } ]
  },
  "impact": {
    "risk_level": "low|medium|high",              // from the existing blast radius
    "risk_markers": ["..."],
    "affected_environment_types": ["production"],
    "client_count": "none|few|several|many",      // 0 | 1-3 | 4-10 | >10
    "evaluation_volume": "none|low|medium|high",  // 7d: 0 | <1k | <100k | >=100k
    "dependent_flags": "none|some|many"           // 0 | 1-3 | >3
  }
}
```

Rules: keep at most 30 diff entries (in order), and truncate `before`/`after` values to 200 characters each. Do not include owner, user ids, or emails.

**Questions** (one request):

| id | type | instructions | criteria |
|---|---|---|---|
| `widens_prod_exposure` | noul | Does `change` make `feature` reach more end users in a production environment than before? Count it as yes when targeting rules are removed or loosened, a rollout percentage goes up, or the feature is turned on in production. | true: "More production end users get the feature after this change." false: "Production exposure stays the same or goes down, or the change is not in production." |
| `disables_safety_control` | noul | Does `change` turn off, bypass, or weaken a safety control, such as a kill switch, circuit breaker, rate limit, fraud check, or permission check? | true: "A safety control is turned off, bypassed, or weakened." false: "No safety control is weakened." |
| `touches_sensitive_domain` | noul | Does `feature` control behavior in a sensitive area: payments or billing, authentication or authorization, security, personal data or privacy, or data deletion? | true: "The feature controls one of these sensitive areas." false: "The feature controls none of these areas." |
| `overall_risk` | score | If this change turns out to be wrong, how serious is the harm to end users? Use `change` and `impact`. | 0 "No end-user impact: internal tooling, a development environment only, or a cosmetic change." 1 "Limited impact: a small or opt-in audience, or a problem that is easy to notice and revert." 2 "Broad impact: many end users in production see changed behavior." 3 "Critical impact: money, access, security, or data integrity for many users could be affected." |

**Derivation** (pure function, constants in `approval_risk.rs`):

- `high` if `overall_risk.score >= 2.2`, or `disables_safety_control >= 0.7`, or (`widens_prod_exposure >= 0.7` and `touches_sensitive_domain >= 0.6`).
- else `medium` if `overall_risk.score >= 1.3`, or `widens_prod_exposure >= 0.7`, or `touches_sensitive_domain >= 0.7`, or `overall_risk.confidence < 0.3`.
- else `low`.
- `reasons`: fixed strings, one per signal that crossed its threshold:
  - "Widens production exposure"
  - "Weakens a safety control"
  - "Sensitive area (payments, auth, security, or personal data)"
  - "High potential user impact"
  - "Assessment uncertain"
- `derived = { "level": "...", "reasons": [...], "signals": { "widens_prod_exposure": 0.0, "disables_safety_control": 0.0, "touches_sensitive_domain": 0.0, "overall_risk": 0.0, "overall_risk_confidence": 0.0 } }`.

**Apply step.**

- Write an activity log entry `approval_risk_assessed` (new constant in `utils/activity_logger.rs`) with metadata `{approval_request_id, feature_id, level, reasons, model}`.
- Only in AI-11, and only if the request is still `pending`, `policy.ai_risk_mode = 'require_extra_approver'`, and `level = high`:
  `UPDATE approval_requests SET required_approvers_override = <policy.required_approvers + 1> WHERE id = $1 AND status = 'pending' AND required_approvers_override IS NULL`.

**Policy enforcement (AI-11).**

- `gate_auto_approve`: `list_requests_due_for_auto_approval` (`database/approval.rs`) excludes a request when a `done` approval-risk judgment for it has `derived->>'level' = 'high'` and the team's `approval_risk` setting is on. A missing, pending, or failed judgment does **not** exclude it (fail open, D3).
- `require_extra_approver`: `apply_vote` and `apply_vote_tx` (`logic/approval.rs`) pass `request.required_approvers_override.unwrap_or(policy.required_approvers)` as the `required_approvers` argument of `record_vote` (`database/approval.rs`). The SQL there does not change.
- An assessment never reopens a request that is already approved, rejected, or cancelled.
- Admin override behavior does not change.

**REST.**

- `ApprovalRequestResponse` (`rest/approval.rs`) gains `ai_risk: Option<AiRiskSummary>` and `required_approvals_effective: i32`. `AiRiskSummary = { status: pending|done|failed, level?, reasons, signals?, model?, assessed_at? }`. It is `None` when no judgment row exists. Populate it in `map_request_with_policy`, which is also used by `rest/stream.rs`.
- Policy DTOs (`ApprovalPolicyResponse`, `CreateApprovalPolicyRequest`, `UpdateApprovalPolicyRequest`) gain `ai_risk_mode`. Validate the value; reject unknown values with 400.
- Follow each DTO's existing serde casing (the UI reads camelCase).

### 5.2 Justification check (AI-20, AI-21)

**Reason kinds and where they are submitted today:**

| `reason_kind` | Handler / logic | Activity written today |
|---|---|---|
| `emergency_disable` | `emergency_disable_feature` (`rest/feature.rs`) → `emergency_disable_feature_in_tx` (`logic/feature_tx.rs`) | `KILL_SWITCH_ACTIVATED`, metadata has `reason` |
| `emergency_enable` | `emergency_enable_feature` → `emergency_enable_feature_in_tx` | `KILL_SWITCH_DEACTIVATED`, metadata has `reason` |
| `freeze_override` | `enforce_freeze_for_feature_environment` / `enforce_freeze_for_stage` → `log_freeze_attempt` (`rest/operational_safety.rs`) | `freeze_override`, metadata has `override_reason` |
| `scheduled_change` | `create_scheduled_change` and the reschedule handler (`rest/operational_safety.rs`) | `scheduled_change_created` (create only) |
| `archive_cleanup` | `update_feature_in_tx` (`logic/feature_tx.rs`) when lifecycle moves to archived | `FEATURE_LIFECYCLE_UPDATED`, metadata **lacks** `cleanup_reason` |
| `freeze_window` | `create_freeze_window` / `update_freeze_window` (`rest/operational_safety.rs`) | none |

**Sync pre-check endpoint.** `POST /api/v1/teams/{team_id}/ai/justification-check`

```jsonc
// request
{ "reasonKind": "emergency_disable", "reason": "...", "featureKey": "checkout-v2" /* optional */ }
// response 200
{ "available": true, "verdict": "ok|weak", "probability": 0.82, "hints": ["..."], "source": "rule|model" }
// unavailable (no key, team setting off, timeout, API error): 200
{ "available": false }
```

**Logic** (`judgment/justification.rs`):

1. Rule check first, no API call: if `reason` matches a ticket key `\b[A-Z][A-Z0-9]+-\d+\b`, a URL `https?://\S+`, an issue reference `#\d+`, or an incident id `\bINC\d+\b`, return `verdict = ok`, `source = rule`.
2. Otherwise one request. State: `{ "action": "<fixed description of reason_kind>", "feature_key": "...", "reason": "..." }`. Fixed action descriptions live in code, for example `emergency_disable` maps to "Turn off a feature flag in an emergency (kill switch)".
3. Questions:

| id | type | instructions | criteria |
|---|---|---|---|
| `concrete_cause` | noul | Does `reason` state a specific cause or purpose for `action`, such as an incident, a bug, a ticket, customer impact, a release plan, or a deadline? | true: "The reason names a specific cause or purpose that another engineer could check or act on." false: "The reason is generic, contains no facts, or only repeats the action." |
| `placeholder_text` | noul | Is `reason` placeholder or filler text with no information, such as "test", "asdf", "n/a", "urgent", "fix", or "as requested"? | true: "The reason is placeholder or filler text." false: "The reason contains real information." |

4. `verdict = weak` if `placeholder_text >= 0.6` or `concrete_cause < 0.4`, else `ok`. `probability = concrete_cause`. Fixed hints:
   - "This looks like placeholder text." when the placeholder signal fired.
   - "Add the cause: an incident, ticket, bug, or customer impact." when the concrete signal fired.

**Post-submit recording (async).** After each path in the table commits, if the team setting `justification_check` is on, submit a `Justification` judgment. Input: `{reason_kind, reason, feature_key}`.

- Subject is the activity row (`activity`, activity id) where one exists. `create_activity` and `create_activity_tx` return the row; thread the id out.
- Freeze windows use (`freeze_window`, id). Reschedules without an activity row use (`scheduled_change`, id).
- `archive_cleanup`: add `cleanup_reason` to the `FEATURE_LIFECYCLE_UPDATED` metadata when the new stage is archived, then use that activity row.
- Apply step: for `activity` subjects, merge `{"ai_justification": {"verdict", "probability", "model"}}` into the activity row's metadata with `jsonb_set` (new repository method `merge_activity_metadata(id, key, value)` in `database/activity_log.rs`).
- Submission must never delay or fail the user's request. Emergency paths stay as fast as today.
- The rule check also applies: a rule pass is recorded as `done` with `derived.source = "rule"` and no API call.

### 5.3 Flag kind (AI-30, AI-31, AI-32)

**Kinds** (Choice options, with an extra `unknown` that is never stored):

| option | criteria |
|---|---|
| `release` | Temporary flag that hides or gradually rolls out new or changed functionality. Removed after the rollout finishes. |
| `experiment` | Temporary flag for an A/B test or experiment that compares variants and measures results. |
| `ops` | Long-lived operational control: kill switch, circuit breaker, maintenance mode, load shedding, or fallback toggle. |
| `permission` | Long-lived entitlement: turns functionality on for specific plans, customers, roles, or beta programs. |
| `config` | Long-lived configuration value that tunes behavior, such as a limit, timeout, or text, instead of gating a feature. |
| `unknown` | The key, description, purpose, and tags are not enough to tell. |

State: `{ "feature": { "key", "description", "purpose", "tags", "feature_type" } }`. Instructions: "What kind of feature flag is `feature`, based on its key, description, purpose, and tags?"

**Async classification.**

- Trigger: after `create_feature_in_tx` and `update_feature_in_tx` commit (handlers `create_feature` and `update_feature` in `rest/feature.rs`), when the team setting `flag_kind` is on, `flag_kind_source` is not `user`, and the content hash of `{key, description, purpose, tags}` differs from the hash of the last judgment input.
- Apply: only if the answer is not `unknown`, the confidence is at least 0.5, and the feature's current content hash still equals the judgment's input hash. Then `UPDATE features SET flag_kind = $kind, flag_kind_source = 'ai', flag_kind_confidence = $conf WHERE id = $id AND (flag_kind_source IS NULL OR flag_kind_source = 'ai')`.

**User choice.** `CreateFeatureRequest` and `UpdateFeatureRequest` (`rest/feature/types.rs`) gain optional `flag_kind`.

- When the field is present and differs from the stored value, store it with `flag_kind_source = 'user'` and `flag_kind_confidence = NULL`.
- When it is absent or equal to the stored value, leave all three columns unchanged. This matters because `PATCH /features/{id}` is a full-body replace and the UI resends current values.
- An explicit JSON `null` from the user clears the kind and sets the source to `user`. Use `Option<Option<_>>` the way `model::UpdateFeatureInput` does.

**Sync suggestions.** `POST /api/v1/teams/{team_id}/ai/feature-suggestions` with body `{key, description, purpose, tags}` returns `{available, kind: {value, probabilities, confidence} | null, tags: [{tag, probability}]}`. Rules:

- Kind: the same Choice question. Return `null` when the answer is `unknown` or confidence is below 0.5.
- Tags: the team's distinct existing tags not already on the feature, the 50 most used. One Noul per candidate in the same request: instructions object `{ "candidate_tag": "<tag>", "question": "Does the tag `candidate_tag` describe `feature`?" }`. Return at most 5 with probability at least 0.6, sorted descending.

**Backfill.** `POST /api/v1/teams/{team_id}/ai/flag-kind/backfill` (admin only) submits judgments for up to 500 features with `flag_kind IS NULL AND archived_at IS NULL`, then returns `{queued}`. The client semaphore bounds concurrency.

**List filter.** `FeatureListQuery` gains `flag_kind` (one of the kinds, or `unclassified` for `IS NULL`). `push_feature_filters` (`database/feature.rs`) adds the clause.

**Stale rules (AI-31).**

- In `FeatureLogicImpl::stale_reasons` (`logic/feature.rs`) and `stale_predicate_sql` (`database/feature.rs`), features with `flag_kind IN ('ops','permission','config')` skip the three rules "No recent evaluations", "No evaluations in 90 days", and "Disabled for 90+ days".
- "Expired" still applies.
- This applies whatever the source (AI or user).
- The two implementations must stay equivalent.

### 5.4 Natural-language search (AI-40, AI-41)

`POST /api/v1/teams/{team_id}/features/nl-search` with body `{ "query": "...", "limit": 20 }` (limit at most 20; query 3 to 300 characters).

Response:

```jsonc
{ "available": true,
  "filtersApplied": { "lifecycleStage": "active", "stale": true, "tag": "payments" },
  "results": [ { "feature": { /* same item shape as GET /teams/{id}/features */ }, "relevance": 0.91 } ] }
```

If unavailable, the response is `{ "available": false }`.

**Call 1: map the query to filters.** State `{ "query": "..." }`. One Choice per filter. Every Choice has an `unspecified` option meaning "The query does not say".

| id | options |
|---|---|
| `lifecycle_stage` | draft, active, deprecated, archived, unspecified |
| `stale` | stale, not_stale, unspecified |
| `expired` | expired, not_expired, unspecified |
| `feature_type` | simple, contextual, unspecified |
| `dependency_status` | has_dependencies, blocked_by_dependencies, independent, unspecified |
| `approval_status` | the values accepted by the existing `approval_status` list filter (approval request statuses), unspecified |
| `flag_kind` | the five kinds, unclassified, unspecified (only if AI-30 is merged) |
| `tag` | the team's distinct tags (the 254 most used) plus `none` |
| `owner` | the team's distinct owners (the 254 most used) plus `none` |

Also a Noul `has_topic`: "Does `query` describe what the flags are about, such as a product area, feature, or behavior, beyond status filters like stale, archived, owner, or tag?"

Code applies a filter only when the chosen option is not `unspecified` or `none` and its confidence is at least 0.6. Write instructions per Choice, for example "Which lifecycle stage does `query` ask for?"

**Candidates.** Run the existing list query (`get_features_windowed` path) with the applied filters, ordered by `evaluation_count_30d` descending, taking at most 50.

**Call 2: rerank** (only if `has_topic > 0.5` and there is at least one candidate).

- State: `{ "query": "...", "candidates": [ { "key", "description", "purpose", "tags" } ] }`.
- One Noul per candidate: `c0..cN`, "Is `candidates[i]` a flag that `query` is looking for?"
- Keep candidates with relevance at least 0.3, sorted descending, truncated to `limit`.

If `has_topic <= 0.5`, return the first `limit` candidates with `relevance = null`.

**Privacy.** Call 1 sends the team's tag and owner values to TypeSafe. The settings page notice (AI-02) says so.

## 6. REST surface summary

All paths are under `/api/v1` and follow the existing auth and team-membership checks of neighboring team routes. Every new endpoint and schema must be registered in `ApiDoc` (`rest/mod.rs`). Then update the contract baseline: run `scripts/export-contracts.sh` and copy `contracts/generated/contract-hashes.json` to `contracts/baseline/`.

| Method | Path | Task | Notes |
|---|---|---|---|
| GET | `/ai/status` | AI-00 | `{available, model}` |
| GET | `/teams/{team_id}/ai-settings` | AI-01 | settings plus `available` |
| PUT | `/teams/{team_id}/ai-settings` | AI-01 | admin only |
| POST | `/teams/{team_id}/ai/justification-check` | AI-20 | sync |
| POST | `/teams/{team_id}/ai/feature-suggestions` | AI-30 | sync |
| POST | `/teams/{team_id}/ai/flag-kind/backfill` | AI-30 | admin only |
| POST | `/teams/{team_id}/features/nl-search` | AI-40 | sync |
| changed | approval request and policy DTOs | AI-10, AI-11 | `aiRisk`, `requiredApprovalsEffective`, `aiRiskMode` |
| changed | feature DTOs and list query | AI-30 | `flagKind`, `flagKindSource`, `flagKindConfidence`, `flagKind` filter |

Sync AI endpoints return 200 with `available: false` when the subsystem is off, the team setting is off, or the call fails. They return 400 only for invalid input.

## 7. Testing and tuning

- **Unit (no network)**: each builder gets a JSON snapshot test of the request it produces, and each `derive` gets table tests on hand-written answers. `MockJudgmentClient` drives service and handler tests. The repo has no HTTP mock crate; do not add one.
- **Integration (`tests/`)**: follow `tests/auto_approval_scheduler_test.rs` (mock repos and logic) for the scheduler and policy paths.
- **API tests (`api-tests/`)**: with no key configured, assert that every new endpoint returns `available: false` and that existing flows are unchanged.
- **Live tuning (`#[ignore]`, needs `TYPESAFE_API_KEY`)**: one fixture file per kind at `feature-toggle-backend/tests/fixtures/ai/<kind>.json`, at least 20 labelled cases, covering clear positives, clear negatives, and borderline cases. The test prints per-case answers and a confusion summary. It asserts only a minimum accuracy agreed in the task. Record the tuned thresholds and the result in the task's handoff log.
- **UI**: Vitest with Testing Library. Mock `api/ai.ts`. Test hidden-when-unavailable, pending, done, and failed states.

## 8. Security and privacy

- The API key exists only in the backend process environment.
- Data sent to TypeSafe: feature key, description, purpose, tags, feature type, flag kind, change diffs (truncated), bucketed impact data, free-text reasons, and (NL search only) the team's tag and owner values. Nothing else.
- TypeSafe states it does not train on customer requests (https://docs.typesafe.ai/models.md). ZDR is an enterprise option.
- Prompt-injection risk: user text can steer answers. Mitigations: the model can only add friction; reasons are fixed strings; nothing auto-executes on a model answer.
- Feature toggles default off per team, and the settings page shows a data notice before enabling.

## 9. Rollout

1. Merge AI-00 and AI-01. Nothing changes for users: no key, all toggles off.
2. Set `TYPESAFE_API_KEY` in a staging environment. Enable features per team.
3. Run the live tuning tests and adjust constants.
4. Turn on approval risk in `advisory` mode first. Use `gate_auto_approve` and `require_extra_approver` only after a period of advisory data.

## 10. Open items for later (not in scope)

- Per-team rate limits or budgets for sync endpoints.
- Showing the justification verdict in the activity log UI.
- Training a classical model on stored `raw_answers` and outcomes (see the TypeSafe feature-discovery cookbook).
