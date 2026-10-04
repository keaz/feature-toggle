//! Every command reaches the right endpoint with the right method, query and
//! body. Each case runs against a fresh mock that accepts exactly that call.

mod common;

use common::*;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockBuilder, ResponseTemplate};

const STAGE: &str = "55555555-5555-5555-5555-555555555555";
const OTHER: &str = "66666666-6666-6666-6666-666666666666";

struct Case {
    args: Vec<String>,
    method: &'static str,
    path: String,
    query: Vec<(&'static str, String)>,
    body: Option<Value>,
}

fn case(args: &[&str], method: &'static str, path: &str) -> Case {
    Case {
        args: args.iter().map(|a| a.to_string()).collect(),
        method,
        path: format!("/api/v1{path}"),
        query: vec![],
        body: None,
    }
}

impl Case {
    fn query(mut self, key: &'static str, value: &str) -> Self {
        self.query.push((key, value.to_string()));
        self
    }
    fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
}

fn cases() -> Vec<Case> {
    let f = FEATURE_ID;
    let t = TEAM_A;
    vec![
        // generic
        case(&["api", "GET", "/teams/{team}/features", "--query", "limit=5"], "GET", &format!("/teams/{t}/features")).query("limit", "5"),
        case(&["api", "POST", &format!("/features/{f}/emergency-enable"), "--data", r#"{"reason":"ok"}"#], "POST", &format!("/features/{f}/emergency-enable")).body(json!({"reason": "ok"})),
        // flags
        case(&["flags", "list", "--tag", "beta", "--owner", "bob", "--flag-kind", "release", "--external-key", "PROJ-1", "--lifecycle-stage", "ACTIVE", "--stale", "--include-archived", "--name", "chk"], "GET", &format!("/teams/{t}/features"))
            .query("tag", "beta").query("owner", "bob").query("flagKind", "release").query("externalKey", "PROJ-1")
            .query("lifecycleStage", "ACTIVE").query("stale", "true").query("includeArchived", "true").query("name", "chk"),
        case(&["flags", "create", "--data", r#"{"key":"new-flag"}"#], "POST", &format!("/teams/{t}/features")).body(json!({"key": "new-flag"})),
        case(&["flags", "update", f, "--data", r#"{"description":"d"}"#], "PATCH", &format!("/features/{f}")).body(json!({"description": "d"})),
        case(&["flags", "archive", f, OTHER, "--yes"], "POST", &format!("/teams/{t}/features/bulk-actions"))
            .body(json!({"featureIds": [f, OTHER], "action": "archive", "archiveConfirmation": true})),
        case(&["flags", "bulk", "update-tags", f, "--tag", "a", "--tag", "b"], "POST", &format!("/teams/{t}/features/bulk-actions"))
            .body(json!({"featureIds": [f], "action": "update_tags", "tags": ["a", "b"]})),
        case(&["flags", "versions", f], "GET", &format!("/features/{f}/versions")),
        case(&["flags", "diff", f, OTHER], "GET", &format!("/features/{f}/versions/{OTHER}/diff")),
        case(&["flags", "rollback", f, OTHER, "--yes"], "POST", &format!("/features/{f}/versions/{OTHER}/rollback")).body(json!({"archiveConfirmation": true})),
        case(&["flags", "impact", f], "GET", &format!("/features/{f}/dependency-impact")),
        case(&["flags", "impact-preview", f, "--data", r#"{"enabled":false}"#], "POST", &format!("/features/{f}/impact-preview")).body(json!({"enabled": false})),
        case(&["flags", "kill", f, "--reason", "incident 42", "--rollback-in", "30"], "POST", &format!("/features/{f}/emergency-disable"))
            .body(json!({"reason": "incident 42", "rollbackInMinutes": 30})),
        case(&["flags", "unkill", f, "--reason", "fixed"], "POST", &format!("/features/{f}/emergency-enable")).body(json!({"reason": "fixed"})),
        case(&["flags", "kill-switches"], "GET", "/features/active-kill-switches").query("teamId", t),
        case(&["flags", "schedules", f], "GET", &format!("/features/{f}/scheduled-changes")),
        case(&["flags", "schedule", f, "--action", "disable-feature", "--at", "2026-10-05T10:00:00Z", "--reason", "night"], "POST", &format!("/features/{f}/scheduled-changes"))
            .body(json!({"action": "DISABLE_FEATURE", "scheduledAt": "2026-10-05T10:00:00Z", "reason": "night"})),
        case(&["flags", "unschedule", OTHER], "PATCH", &format!("/scheduled-changes/{OTHER}/cancel")).body(json!({})),
        case(&["flags", "reschedule", OTHER, "--at", "2026-10-06T10:00:00Z"], "PATCH", &format!("/scheduled-changes/{OTHER}/reschedule")).body(json!({"scheduledAt": "2026-10-06T10:00:00Z"})),
        case(&["flags", "links", f], "GET", &format!("/features/{f}/external-links")),
        case(&["flags", "link", f, "PROJ-9", "--issue-url", "https://jira.example.com/browse/PROJ-9"], "POST", &format!("/features/{f}/external-links"))
            .body(json!({"system": "jira", "externalKey": "PROJ-9", "url": "https://jira.example.com/browse/PROJ-9"})),
        case(&["flags", "unlink", f, OTHER], "DELETE", &format!("/features/{f}/external-links/{OTHER}")),
        case(&["flags", "search", "checkout flags owned by payments", "--limit", "5"], "POST", &format!("/teams/{t}/features/nl-search")).body(json!({"query": "checkout flags owned by payments", "limit": 5})),
        // approvals
        case(&["approvals", "approve", OTHER, "--comment", "lgtm"], "POST", &format!("/approval-requests/{OTHER}/approve")).body(json!({"comment": "lgtm"})),
        case(&["approvals", "reject", OTHER], "POST", &format!("/approval-requests/{OTHER}/reject")).body(json!({})),
        case(&["approvals", "cancel", OTHER], "POST", &format!("/approval-requests/{OTHER}/cancel")).body(json!({})),
        case(&["approvals", "preview", "--data", r#"{"changeType":"stage_change"}"#], "POST", &format!("/teams/{t}/approval-policy-preview")).body(json!({"changeType": "stage_change"})),
        // change safety
        case(&["freeze", "active"], "GET", &format!("/teams/{t}/freeze-windows/active")),
        case(&["canary", "gates", STAGE], "GET", &format!("/stages/{STAGE}/canary-gates")),
        case(&["canary", "set", STAGE, "--data", r#"{"gates":[]}"#], "PUT", &format!("/stages/{STAGE}/canary-gates")).body(json!({"gates": []})),
        case(&["canary", "analyze", OTHER], "POST", &format!("/canary-gates/{OTHER}/analyze")).body(json!({})),
        case(&["criteria", "get", STAGE], "GET", &format!("/stages/{STAGE}/criteria")),
        case(&["criteria", "set", STAGE, "--data", "[]"], "PUT", &format!("/stages/{STAGE}/criteria")).body(json!([])),
        case(&["criteria", "variants", OTHER, "--data", "[]"], "PUT", &format!("/criteria/{OTHER}/variant-allocations")).body(json!([])),
        // jira
        case(&["jira", "rules", OTHER], "GET", &format!("/jira-integrations/{OTHER}/rules")),
        case(&["jira", "set-rules", OTHER, "--data", "[]"], "PUT", &format!("/jira-integrations/{OTHER}/rules")).body(json!([])),
        case(&["jira", "rotate-secret", OTHER], "POST", &format!("/jira-integrations/{OTHER}/rotate-secret")).body(json!({})),
        case(&["jira", "webhook-secret", OTHER], "POST", &format!("/jira-integrations/{OTHER}/native-webhook-secret")).body(json!({})),
        case(&["jira", "webhook-secret", OTHER, "--delete"], "DELETE", &format!("/jira-integrations/{OTHER}/native-webhook-secret")),
        case(&["jira", "writeback", OTHER, "--data", r#"{"enabled":true}"#], "PUT", &format!("/jira-integrations/{OTHER}/writeback")).body(json!({"enabled": true})),
        case(&["jira", "writeback-test", OTHER], "POST", &format!("/jira-integrations/{OTHER}/writeback/test")).body(json!({})),
        case(&["jira", "writeback-resume", OTHER], "POST", &format!("/jira-integrations/{OTHER}/writeback/resume")).body(json!({})),
        case(&["jira", "events", OTHER], "GET", &format!("/jira-integrations/{OTHER}/events")),
        case(&["jira", "jobs", OTHER], "GET", &format!("/jira-integrations/{OTHER}/outbound-jobs")),
        case(&["jira", "retry", OTHER, STAGE], "POST", &format!("/jira-integrations/{OTHER}/outbound-jobs/{STAGE}/retry")).body(json!({})),
        // ai
        case(&["ai", "status"], "GET", "/ai/status"),
        case(&["ai", "settings"], "GET", &format!("/teams/{t}/ai-settings")),
        case(&["ai", "set-settings", "--data", r#"{"enabled":true}"#], "PUT", &format!("/teams/{t}/ai-settings")).body(json!({"enabled": true})),
        case(&["ai", "justify", "--data", r#"{"reason":"r"}"#], "POST", &format!("/teams/{t}/ai/justification-check")).body(json!({"reason": "r"})),
        case(&["ai", "suggest", "--data", r#"{"description":"d"}"#], "POST", &format!("/teams/{t}/ai/feature-suggestions")).body(json!({"description": "d"})),
        case(&["ai", "backfill-kinds"], "POST", &format!("/teams/{t}/ai/flag-kind/backfill")).body(json!({})),
        // observability
        case(&["metrics", "summary", "--query", "period=day"], "GET", "/metrics/evaluations/summary").query("teamId", t).query("period", "day"),
        case(&["metrics", "experiments", "--query", "featureKey=checkout"], "GET", "/metrics/experiment-results").query("teamId", t),
        case(&["audit"], "GET", &format!("/teams/{t}/audit-analytics")),
        case(&["activity", "--query", "limit=20"], "GET", "/activity/recent").query("limit", "20"),
        // admin resources
        case(&["admin", "environments", "list"], "GET", &format!("/teams/{t}/environments")),
        case(&["admin", "environments", "get", OTHER], "GET", &format!("/environments/{OTHER}")),
        case(&["admin", "environments", "create", "--data", r#"{"name":"qa"}"#], "POST", &format!("/teams/{t}/environments")).body(json!({"name": "qa"})),
        case(&["admin", "environments", "update", OTHER, "--data", r#"{"active":false}"#], "PATCH", &format!("/environments/{OTHER}")).body(json!({"active": false})),
        case(&["admin", "environments", "delete", OTHER], "DELETE", &format!("/environments/{OTHER}")),
        case(&["admin", "contexts", "list"], "GET", &format!("/teams/{t}/contexts")),
        case(&["admin", "clients", "create", "--data", "{}"], "POST", &format!("/teams/{t}/clients")).body(json!({})),
        case(&["admin", "system-clients", "list"], "GET", &format!("/teams/{t}/system-clients")),
        case(&["admin", "pipelines", "update", OTHER, "--data", "{}"], "PATCH", &format!("/pipelines/{OTHER}")).body(json!({})),
        case(&["admin", "rollout-templates", "list"], "GET", &format!("/teams/{t}/rollout-templates")),
        case(&["admin", "metric-definitions", "list"], "GET", &format!("/teams/{t}/metrics")),
        case(&["admin", "teams", "create", "--data", r#"{"name":"new"}"#], "POST", "/teams").body(json!({"name": "new"})),
        case(&["admin", "users", "list"], "GET", "/users"),
        case(&["admin", "roles", "delete", OTHER], "DELETE", &format!("/roles/{OTHER}")),
        case(&["admin", "sso-providers", "get", OTHER], "GET", &format!("/sso/providers/{OTHER}")),
        case(&["admin", "jira-integrations", "list"], "GET", &format!("/teams/{t}/jira-integrations")),
        case(&["admin", "approval-policies", "delete", OTHER], "DELETE", &format!("/approval-policies/{OTHER}")),
        case(&["admin", "freeze-windows", "create", "--data", "{}"], "POST", &format!("/teams/{t}/freeze-windows")).body(json!({})),
        case(&["admin", "rule-groups", "create", "--data", "{}"], "POST", "/rule-groups").body(json!({})),
        // system clients, secrets, sso, users, notifications
        case(&["system-clients", "tokens", OTHER], "GET", &format!("/system-clients/{OTHER}/tokens")),
        case(&["system-clients", "create-token", OTHER], "POST", &format!("/system-clients/{OTHER}/tokens")).body(json!({})),
        case(&["system-clients", "revoke-token", STAGE], "POST", &format!("/system-client-tokens/{STAGE}/revoke")).body(json!({})),
        case(&["system-clients", "rotate-token", OTHER], "POST", &format!("/system-clients/{OTHER}/regenerate-token")).body(json!({})),
        case(&["jwt-secrets", "list"], "GET", "/auth/jwt-secrets"),
        case(&["jwt-secrets", "rotate"], "POST", "/auth/jwt-secrets").body(json!({})),
        case(&["jwt-secrets", "deactivate-all"], "POST", "/auth/jwt-secrets/deactivate-all").body(json!({})),
        case(&["sso", "mappings", OTHER], "GET", &format!("/sso/providers/{OTHER}/mappings")),
        case(&["sso", "set-mappings", OTHER, "--data", "[]"], "PUT", &format!("/sso/providers/{OTHER}/mappings")).body(json!([])),
        case(&["sso", "test", OTHER], "POST", &format!("/sso/providers/{OTHER}/test")).body(json!({})),
        case(&["sso", "settings"], "GET", "/sso/settings"),
        case(&["sso", "set-settings", "--data", r#"{"enforceSso":false}"#], "PUT", "/sso/settings").body(json!({"enforceSso": false})),
        case(&["users", "roles", OTHER], "GET", &format!("/users/{OTHER}/roles")),
        case(&["users", "set-roles", OTHER, "--data", r#"{"roleIds":[]}"#], "POST", &format!("/users/{OTHER}/roles")).body(json!({"roleIds": []})),
        case(&["users", "set-teams", OTHER, "--data", r#"{"teamIds":[]}"#], "POST", &format!("/users/{OTHER}/teams")).body(json!({"teamIds": []})),
        case(&["users", "temporary-password", OTHER, "--data", r#"{"temporaryPassword":"x"}"#], "POST", &format!("/auth/users/{OTHER}/temporary-password")).body(json!({"temporaryPassword": "x"})),
        case(&["notifications", "show"], "GET", "/notifications/settings"),
        case(&["notifications", "channel", "slack", "--data", r#"{"enabled":true}"#], "PUT", "/notifications/channels/slack").body(json!({"enabled": true})),
        case(&["notifications", "preference", "approval_requested", "--data", r#"{"enabled":true}"#], "PUT", "/notifications/preferences/approval_requested").body(json!({"enabled": true})),
    ]
}

#[tokio::test]
async fn every_command_calls_its_endpoint() {
    let mut failures = Vec::new();
    for case in cases() {
        let h = Harness::new().await;
        let mut mock: MockBuilder = Mock::given(method(case.method)).and(path(case.path.as_str()));
        for (key, value) in &case.query {
            mock = mock.and(query_param(*key, value.as_str()));
        }
        if let Some(body) = &case.body {
            mock = mock.and(body_json(body.clone()));
        }
        mock.respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .named(case.args.join(" "))
            .mount(&h.server)
            .await;
        let (url, token) = (h.url(), user_token("alice"));
        let args: Vec<&str> = case.args.iter().map(String::as_str).collect();
        let r = h
            .run(
                &args,
                &[
                    ("FLUXGATE_URL", url.as_str()),
                    ("FLUXGATE_TOKEN", token.as_str()),
                    ("FLUXGATE_TEAM", TEAM_A),
                ],
            )
            .await;
        if r.code != 0 {
            failures.push(format!(
                "{} -> exit {}: {}",
                case.args.join(" "),
                r.code,
                r.stderr.trim()
            ));
        }
        let received = h.server.received_requests().await.unwrap_or_default();
        if received.len() != 1 {
            failures.push(format!(
                "{} -> {} requests: {:?}",
                case.args.join(" "),
                received.len(),
                received
                    .iter()
                    .map(|r| format!("{} {}", r.method, r.url))
                    .collect::<Vec<_>>()
            ));
        }
        // Verify on drop would panic without the summary; check here instead.
        h.server.reset().await;
    }
    assert!(
        failures.is_empty(),
        "{} failing commands:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn data_can_come_from_a_file() {
    let h = Harness::new().await;
    h.write("feature.json", r#"{"key":"from-file"}"#);
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(body_json(json!({"key": "from-file"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": FEATURE_ID})))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let file = format!("@{}", h.path("feature.json").display());
    let r = h
        .run(
            &["flags", "create", "--data", file.as_str()],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn invalid_data_is_a_usage_error_before_any_request() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "create", "--data", "{not json", "--output", "text"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--data"), "{}", r.stderr);
    assert!(h.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn archive_without_confirmation_is_refused() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "archive", FEATURE_ID, "--output", "text"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--yes"), "{}", r.stderr);
}

#[tokio::test]
async fn flag_keys_are_resolved_to_ids() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/teams/{TEAM_A}/features/by-key/checkout"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": FEATURE_ID, "key": "checkout"})),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/features/{FEATURE_ID}/emergency-disable"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": FEATURE_ID})))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "kill", "checkout", "--reason", "incident"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn generic_lists_render_as_tables() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/contexts")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "c1", "name": "beta-users", "description": "Beta", "nested": {"x": 1}}],
            "meta": {"offset": 0, "limit": 50, "total": 1}})))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["admin", "contexts", "list", "--output", "table"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stdout.contains("NAME") && r.stdout.contains("beta-users"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("nested"), "{}", r.stdout);
}

#[tokio::test]
async fn unknown_admin_resource_is_a_usage_error() {
    let h = Harness::new().await;
    let r = h.run(&["admin", "widgets", "list"], &[]).await;
    assert_eq!(r.code, 2);
    assert!(
        r.stderr.contains("unrecognized subcommand 'widgets'"),
        "{}",
        r.stderr
    );
    let help = h.run(&["admin", "--help"], &[]).await;
    assert_eq!(help.code, 0);
    assert!(
        help.stdout.contains("environments") && help.stdout.contains("freeze-windows"),
        "{}",
        help.stdout
    );
}
