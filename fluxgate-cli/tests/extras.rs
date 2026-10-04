//! Shell completions, live streams, edge evaluation and config import.

mod common;

use common::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use wiremock::matchers::{body_json, body_partial_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn completions_are_printed_verbatim() {
    let h = Harness::new().await;
    for shell in ["bash", "zsh", "fish"] {
        let r = h.run(&["completions", shell], &[]).await;
        assert_eq!(r.code, 0, "{shell}: {}", r.stderr);
        assert!(r.stdout.contains("fluxgate"), "{shell}");
        assert!(!r.stdout.starts_with('{'), "{shell}: not wrapped in JSON");
    }
}

#[tokio::test]
// The handshake callback's error type is fixed by tungstenite.
#[allow(clippy::result_large_err)]
async fn watch_prints_stream_messages_as_json_lines() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new((String::new(), String::new())));
    let record = seen.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let callback = |request: &Request, response: Response| {
            let auth = request
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default();
            *record.lock().unwrap() = (request.uri().to_string(), auth);
            Ok(response)
        };
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, callback)
            .await
            .unwrap();
        for n in 1..=3 {
            let _ = socket
                .send(Message::Text(json!({ "n": n }).to_string().into()))
                .await;
        }
        while socket.next().await.is_some() {}
    });
    let h = Harness::new().await;
    let url = format!("http://127.0.0.1:{port}/api/v1");
    let token = user_token("alice");
    let r = h
        .run(
            &[
                "watch",
                "evaluation-summary",
                "--count",
                "2",
                "--query",
                "period=hour",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout, "{\"n\":1}\n{\"n\":2}\n");
    let (uri, auth) = seen.lock().unwrap().clone();
    assert!(uri.starts_with("/api/v1/ws?"), "{uri}");
    assert!(
        uri.contains("stream=evaluation-summary")
            && uri.contains(&format!("teamId={TEAM_A}"))
            && uri.contains("period=hour"),
        "{uri}"
    );
    assert_eq!(auth, format!("Bearer {token}"));
    server.abort();
}

#[tokio::test]
async fn edge_evaluate_uses_ofrep_with_the_sdk_key() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/ofrep/v1/evaluate/flags/checkout"))
        .and(header("authorization", "Bearer cid.secret"))
        .and(body_json(
            json!({ "context": { "targetingKey": "u1", "plan": "pro" } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key": "checkout", "value": false, "reason": "TARGETING_MATCH", "variant": "off" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let edge = h.server.uri();
    let r = h
        .run(
            &[
                "edge",
                "evaluate",
                "checkout",
                "--targeting-key",
                "u1",
                "--context",
                r#"{"plan":"pro"}"#,
                "--exit-code",
            ],
            &[
                ("FLUXGATE_EDGE_URL", edge.as_str()),
                ("FLUXGATE_EDGE_KEY", "cid.secret"),
            ],
        )
        .await;
    assert_eq!(r.code, 10, "{}", r.stderr);
    assert_eq!(json_out(&r)["value"], false);
}

#[tokio::test]
async fn edge_settings_come_from_the_profile_and_credentials() {
    let h = Harness::new().await;
    h.write(
        "config",
        &format!("[default]\nedge_url = {}\n", h.server.uri()),
    );
    h.write("credentials", "[default]\nedge_key = cid.from-file\n");
    Mock::given(method("POST"))
        .and(path("/ofrep/v1/evaluate/flags"))
        .and(header("authorization", "Bearer cid.from-file"))
        .and(body_json(json!({ "context": { "targetingKey": "u1" } })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "flags": [{ "key": "a", "value": true }] })),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let r = h
        .run(&["edge", "evaluate-all", "--targeting-key", "u1"], &[])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn edge_without_an_address_is_a_usage_error() {
    let h = Harness::new().await;
    let r = h
        .run(
            &[
                "edge",
                "evaluate",
                "checkout",
                "--targeting-key",
                "u1",
                "--output",
                "text",
            ],
            &[],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("FLUXGATE_EDGE_URL"), "{}", r.stderr);
}

fn export() -> serde_json::Value {
    json!({
        "team": { "id": TEAM_B, "name": "Source" },
        "environments": [
            { "id": "e1", "name": "staging", "environmentType": "STAGING", "active": true },
            { "id": "e2", "name": "prod", "environmentType": "PRODUCTION", "active": true }
        ],
        "features": [
            { "id": "f1", "key": "existing", "featureType": "SIMPLE", "enabled": true },
            { "id": "f2", "key": "new-flag", "featureType": "CONTEXTUAL", "enabled": false, "description": "d",
              "owner": "o", "tags": ["a"], "lifecycleStage": "ACTIVE", "flagKind": "release",
              "stages": [{ "id": "s1", "environmentId": "e1" }], "variants": [{ "key": "on" }] }
        ]
    })
}

async fn mount_target(h: &Harness) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "Staging", "active": true }], "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(wiremock::matchers::query_param("includeArchived", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": FEATURE_ID, "key": "existing" }], "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn config_import_creates_only_what_is_missing() {
    let h = Harness::new().await;
    h.write("export.json", &export().to_string());
    mount_target(&h).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .and(body_json(
            json!({ "name": "prod", "active": true, "environmentType": "PRODUCTION" }),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "new-env" })))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(body_partial_json(json!({
            "key": "new-flag", "featureType": "CONTEXTUAL", "enabled": false, "description": "d", "owner": "o",
            "tags": ["a"], "lifecycleStage": "ACTIVE", "flagKind": "release",
            "dependencies": [], "relationships": [], "stages": [] })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "new-feature" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let file = h.path("export.json").display().to_string();
    let r = h
        .run(
            &["config", "import", "--file", file.as_str()],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let report = json_out(&r);
    assert_eq!(report["created"]["environments"], json!(["prod"]));
    assert_eq!(report["created"]["features"], json!(["new-flag"]));
    assert_eq!(report["skipped"]["environments"], json!(["staging"]));
    assert_eq!(report["skipped"]["features"], json!(["existing"]));
    assert!(report["notes"][0].as_str().unwrap().contains("stages"));
}

#[tokio::test]
async fn config_import_dry_run_changes_nothing() {
    let h = Harness::new().await;
    h.write("export.json", &export().to_string());
    mount_target(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let file = h.path("export.json").display().to_string();
    let r = h
        .run(
            &["config", "import", "--file", file.as_str(), "--dry-run"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let report = json_out(&r);
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["created"]["features"], json!(["new-flag"]));
    let posts = h
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(posts, 0);
}

#[tokio::test]
async fn metrics_time_windows_default_to_the_last_day() {
    use wiremock::matchers::query_param;
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/metrics/feature-growth"))
        .and(query_param("interval", "day"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "points": [] })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["metrics", "growth", "--since", "7d"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let request = &h.server.received_requests().await.unwrap()[0];
    let params: std::collections::HashMap<String, String> =
        request.url.query_pairs().into_owned().collect();
    let from = chrono::DateTime::parse_from_rfc3339(&params["fromTime"]).unwrap();
    let to = chrono::DateTime::parse_from_rfc3339(&params["toTime"]).unwrap();
    assert_eq!((to - from).num_days(), 7);
}

#[tokio::test]
async fn metrics_reports_that_need_a_flag_say_so() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["metrics", "experiments", "--output", "text"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--flag"), "{}", r.stderr);
}

#[tokio::test]
async fn config_import_reports_failures_and_keeps_going() {
    let h = Harness::new().await;
    h.write("export.json", &export().to_string());
    mount_target(&h).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({ "error": "invalid_input", "message": "environment type not allowed" }),
        ))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "new-feature" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let file = h.path("export.json").display().to_string();
    let r = h
        .run(
            &["config", "import", "--file", file.as_str()],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 1, "{}", r.stderr);
    let report = json_out(&r);
    assert_eq!(report["created"]["features"], json!(["new-flag"]));
    assert_eq!(report["created"]["environments"], json!([]));
    assert_eq!(report["failed"]["environments"][0]["name"], "prod");
    assert!(
        report["failed"]["environments"][0]["error"]
            .as_str()
            .unwrap()
            .contains("environment type not allowed")
    );
}

#[tokio::test]
async fn config_import_stops_on_an_expired_session() {
    let h = Harness::new().await;
    h.write("export.json", &export().to_string());
    mount_target(&h).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "error": "unauthorized", "message": "token expired" })),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let file = h.path("export.json").display().to_string();
    let r = h
        .run(
            &[
                "config",
                "import",
                "--file",
                file.as_str(),
                "--output",
                "text",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert!(r.stderr.contains("token expired"), "{}", r.stderr);
}
