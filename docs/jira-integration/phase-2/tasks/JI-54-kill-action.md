# JI-54: `kill` status rule action (backend + UI)

| Field | Value |
|---|---|
| Type | Feature (requested 2026-10-04 during the KAN setup) |
| Status | Open, not planned in detail |
| Repo | backend (`feature-toggle/`), then UI (`../feature-toggle-ui/`) |
| Depends on | JI-15 |
| Behavior change | A Jira status rule can turn a feature off (kill switch) in an environment, for example when an issue is reopened after a production release. |

## Goal

Today a status rule can only `request`, `approve`, `deploy` or `rollback` (phase 1 design §3.8). "Kill switch from Jira" is listed as out of scope. In the KAN setup, "Reopened" should ideally kill the feature in Perf-Test-Prod at once; it is configured as `rollback` for now.

## Open questions (decide before planning)

- Trust: allow `kill` only in Jira-approved environments, like `approve`?
- Reason: the kill switch needs a reason (justification check, `emergency_disable`). Use `Jira status '<status>' on <issue>` (generated, not checked, see J27) or require a field from the issue?
- Emergency override settings and freeze windows: does a Jira kill bypass a freeze, as a person's emergency disable does?
- Undo: should a later status (for example "In QA" again) re-enable, or is re-enable always a person's action?

## Where it goes

- `logic/external_change.rs`: new `ExternalAction::Kill` and `plan` row; call the emergency disable path (`emergency_disable_feature_in_tx`) as the integration's shadow user.
- `rule_action` CHECK constraint (new migration) and the rule DTO enum; contract update.
- UI rules editor: "Kill switch" option, with a warning like the Jira-approved one.
- Setup guide: action table.

## Handoff log

(empty)
