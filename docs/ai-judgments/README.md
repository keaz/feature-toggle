# AI judgments (TypeSafe Jev): work packages

This folder holds the design and the task breakdown for adding TypeSafe Jev judgments to FluxGate. Read [`design.md`](design.md) once, then [`HANDOFF.md`](HANDOFF.md) for the current state and what the next tasks must know. Then pick one task file under [`tasks/`](tasks/). Each task file says what to change, where, how to test it, and when it is done. One agent can finish one task without reading the other task files.

Features:

1. Approval risk triage
2. Justification check on free-text reasons
3. Flag kind classification and stale-rule exclusion
4. Natural-language search in the command palette

## Rules for agents

1. **One task per branch and PR.** Branch name: `feat/<id>-<short-name>`, for example `feat/ai-10-approval-risk`.
   - Backend tasks go in this repo (`feature-toggle/`).
   - UI tasks go in `../feature-toggle-ui/`, which is a separate git repo. Branch from its current main line; ask if unsure which branch that is.
2. **Check "Depends on"** in the task header. Do not start until every dependency is merged. If a dependency is only on a branch, stop and ask.
3. **Re-check the code first.** Line numbers come from backend `fc50086` and UI `bae1962` and will drift. Find code by symbol name (`grep -n "fn <name>"`). If the code no longer matches the task, stop and report in the handoff log.
4. **Stay in scope.** Change only what the task lists. If you need a visible behavior change that the task does not list, stop and ask.
5. **Tasks marked "needs sign-off"** (AI-31 and AI-11 signed off 2026-10-02) need a maintainer's yes before you merge. You may write the code and the PR, but say in the PR that it is waiting for sign-off.
6. **Write the failing test first**, then the change. Each task lists its tests.
7. **Run before you finish:**
   - Backend:
     ```bash
     cargo fmt
     cargo clippy --all-targets
     cargo test -p feature-toggle-backend      # needs migrated + seeded DB, see /CLAUDE.md
     ./scripts/check-contract-compat.sh        # when REST DTOs or endpoints changed
     ```
   - UI (in `../feature-toggle-ui/`):
     ```bash
     pnpm lint
     pnpm build           # also the type check
     pnpm test:run        # use pnpm, never npm
     ```
8. **Repo gotchas:**
   - Never edit an applied migration. Add a new timestamped one: `migrations/YYYYMMDDHHMMSS_name.sql`, later than every existing file.
   - Prefer runtime `sqlx::query(...)`. If you add a `query!`/`query_as!` macro, run `cargo sqlx prepare -- --all-targets` in `feature-toggle-backend/` and commit `.sqlx/`.
   - New REST endpoints and DTO fields: register them in `ApiDoc` (`rest/mod.rs`). Then run `scripts/export-contracts.sh` and copy `contracts/generated/contract-hashes.json` to `contracts/baseline/`. Say so in the PR.
   - Adding a field to `model::Feature` breaks struct literals in many tests. Fix them all in the same change.
   - The UI fails tests on raw Tailwind palette classes or hex colors (`__tests__/designTokenGuard.test.ts`). Use token classes such as `bg-warning/10` or `text-destructive`.
   - After changing code, run `graphify update .` in the repo you changed.
9. **Never** put `TYPESAFE_API_KEY` in a file, a log line, a test fixture, or a commit.
10. **When done:**
    - Set the task's `Status` row to `Done in <PR/commit>`.
    - Append a dated entry to the task's **Handoff log**: what changed, what you verified, open questions, and anything the next task must know.
    - Tick the task in the table below.
11. **If you stop before done**, append a Handoff log entry with exactly where you stopped (branch, last commit, failing test, next step). The next agent continues from that entry.

## Decisions (2026-10-01)

| # | Decision |
|---|---|
| D1 | All four features are in scope. |
| D2 | Approval risk action per policy: `ai_risk_mode = off \| advisory \| gate_auto_approve \| require_extra_approver`, default `advisory`. |
| D3 | Jev outage fails open: today's behavior, and the UI shows "assessment unavailable". |
| D4 | The env key `TYPESAFE_API_KEY` turns the subsystem on. Per-team toggles turn each feature on, and default off. |
| D5 | The justification check warns only, never blocks. The result is stored with the audit trail. |
| D6 | `features.flag_kind` column: AI fills it when empty; a user choice wins. |
| D7 | Approval risk runs async after the request is created. |
| D8 | NL search is a backend endpoint with filters plus rerank. |

## Tasks

| Done | ID | Title | Repo | Behavior change | Depends on |
|---|---|---|---|---|---|
| [x] | [AI-00](tasks/AI-00-typesafe-client-and-config.md) | TypeSafe config, client, wire types, `/ai/status` | backend | No | — |
| [x] | [AI-01](tasks/AI-01-judgment-store-and-team-settings.md) | `ai_judgments`, `team_ai_settings`, settings API, `JudgmentService`, retry sweep | backend | No (all off) | AI-00 |
| [x] | [AI-02](tasks/AI-02-ui-ai-foundation-and-settings.md) | UI: `api/ai.ts`, `useAiFeatures`, AI settings page | UI | New page | AI-01 |
| [x] | [AI-10](tasks/AI-10-approval-risk-assessment.md) | Approval risk assessment (advisory) | backend | Additive fields | AI-01 |
| [x] | [AI-11](tasks/AI-11-approval-risk-enforcement.md) | Risk enforcement: gate auto-approve, extra approver | backend | **Yes, needs sign-off** | AI-10 |
| [x] | [AI-12](tasks/AI-12-ui-approval-risk.md) | UI: risk panel, badge, policy mode | UI | New UI | AI-10, AI-02 (AI-11 for the extra-approver count) |
| [x] | [AI-20](tasks/AI-20-justification-check-backend.md) | Justification check: sync endpoint and post-submit recording | backend | Additive | AI-01 |
| [x] | [AI-21](tasks/AI-21-ui-justification-hint.md) | UI: `ReasonQualityHint` on 5 forms | UI | Warning only | AI-20, AI-02 |
| [x] | [AI-30](tasks/AI-30-flag-kind-backend.md) | Flag kind: column, classification, suggestions, backfill, filter | backend | Additive | AI-01 |
| [x] | [AI-31](tasks/AI-31-stale-rules-use-flag-kind.md) | Stale rules skip permanent kinds | backend | **Yes, needs sign-off** | AI-30 |
| [x] | [AI-32](tasks/AI-32-ui-flag-kind.md) | UI: kind field, suggestion chips, filter, detail | UI | New UI | AI-30, AI-02 |
| [x] | [AI-40](tasks/AI-40-nl-search-backend.md) | NL search endpoint | backend | Additive | AI-01 (AI-30 optional) |
| [x] | [AI-41](tasks/AI-41-ui-nl-search-palette.md) | UI: "Ask FluxGate" in the command palette | UI | New UI | AI-40, AI-02 |

## Order and parallel work

```
AI-00 ─► AI-01 ─┬─► AI-02 (UI foundation) ───────────────┐
                ├─► AI-10 ─► AI-11 (sign-off)             │
                │     └──────────────► AI-12 ◄────────────┤
                ├─► AI-20 ─────────────► AI-21 ◄──────────┤
                ├─► AI-30 ─► AI-31 (sign-off)             │
                │     └──────────────► AI-32 ◄────────────┤
                └─► AI-40 ─────────────► AI-41 ◄──────────┘
```

After AI-01 merges, five agents can work at once: AI-02, AI-10, AI-20, AI-30, and AI-40.
