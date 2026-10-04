mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn health_needs_no_credentials() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "ok" })))
        .expect(1)
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h.run(&["health"], &[("FLUXGATE_URL", url.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(json_out(&r)["status"], "ok");
}

#[tokio::test]
async fn flags_list_all_follows_every_page() {
    let h = Harness::new().await;
    let features_path = format!("/api/v1/teams/{TEAM_A}/features");
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(0..200), "meta": { "offset": 0, "limit": 200, "total": 250 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(200..250), "meta": { "offset": 200, "limit": 200, "total": 250 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "list", "--all"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["items"].as_array().unwrap().len(), 250);
    assert_eq!(value["meta"]["total"], 250);
}

#[tokio::test]
async fn flags_list_sends_limit_offset_and_accepts_legacy_team_flag() {
    let h = Harness::new().await;
    let token = user_token("alice");
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(query_param("limit", "10"))
        .and(query_param("offset", "20"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(0..1), "meta": { "offset": 20, "limit": 10, "total": 21 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h
        .run(
            &[
                "flags",
                "list",
                "--team-id",
                TEAM_A,
                "--limit",
                "10",
                "--offset",
                "20",
                "--output",
                "table",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("flag-0"));
    assert!(r.stdout.contains("1 of 21 shown"));
}

#[tokio::test]
async fn flags_get_by_key_uses_the_encoded_by_key_route() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/teams/{TEAM_A}/features/by-key/checkout%20v2"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": FEATURE_ID, "key": "checkout v2" })),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "get", "checkout v2"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(json_out(&r)["key"], "checkout v2");
}

#[tokio::test]
async fn flags_get_by_id_needs_no_team() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/features/{FEATURE_ID}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "id": FEATURE_ID, "key": "checkout" })),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "get", FEATURE_ID],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn team_name_from_a_profile_is_resolved() {
    let h = Harness::new().await;
    h.write("config", "[default]\nteam = payments\n");
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{ "id": TEAM_A, "name": "Payments" }])),
        )
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "items": [], "meta": { "offset": 0, "limit": 50, "total": 0 } }),
        ))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["flags", "list"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn approvals_list_sends_statuses_and_renders_a_table() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/approval-requests")))
        .and(query_param("statuses", "pending,approved"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": "ap1", "featureId": FEATURE_ID, "changeType": "STAGE_CHANGE", "status": "PENDING",
                        "requestedBy": "alice", "createdAt": "2026-10-04T10:00:00Z" }],
            "meta": { "offset": 0, "limit": 50, "total": 1 } })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &[
                "approvals",
                "list",
                "--status",
                "pending,approved",
                "--output",
                "table",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("STATUS") && r.stdout.contains("PENDING"));
}

#[tokio::test]
async fn forbidden_exits_4_with_the_server_message() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": "forbidden", "message": "policy denied", "code": "policy_denied", "details": null })))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let env = [
        ("FLUXGATE_URL", url.as_str()),
        ("FLUXGATE_TOKEN", token.as_str()),
        ("FLUXGATE_TEAM", TEAM_A),
    ];
    let r = h.run(&["flags", "list", "--output", "text"], &env).await;
    assert_eq!(r.code, 4);
    assert!(
        r.stderr
            .contains("error: policy denied (code policy_denied, HTTP 403)"),
        "{}",
        r.stderr
    );
    let r = h.run(&["flags", "list"], &env).await;
    let body: serde_json::Value = serde_json::from_str(&r.stderr).unwrap();
    assert_eq!(body["code"], "policy_denied");
}

#[tokio::test]
async fn missing_credentials_exit_3() {
    let h = Harness::new().await;
    let url = h.url();
    let r = h
        .run(
            &["flags", "list", "--output", "text"],
            &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TEAM", TEAM_A)],
        )
        .await;
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("no credentials for profile default"));
}

#[tokio::test]
async fn unknown_subcommand_is_a_usage_error() {
    let h = Harness::new().await;
    let r = h.run(&["flags", "nope"], &[]).await;
    assert_eq!(r.code, 2);
}
