mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

async fn mount_environments(h: &Harness) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "teamId": TEAM_A, "active": true, "environmentType": "STAGING" }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
}

async fn mount_evaluate(h: &Harness, value: bool) {
    Mock::given(method("POST"))
        .and(path("/api/v1/evaluate"))
        .and(body_json(json!({ "teamId": TEAM_A, "featureKey": "checkout", "environmentId": ENV_STAGING,
                               "targetingKey": "user-1", "context": { "plan": "pro" } })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "flagKey": "checkout", "value": value, "variant": null, "reason": "TARGETING_MATCH", "errorCode": null })))
        .expect(1)
        .mount(&h.server)
        .await;
}

const EVALUATE: [&str; 9] = [
    "evaluate",
    "--flag",
    "checkout",
    "--targeting-key",
    "user-1",
    "--context",
    "{\"plan\":\"pro\"}",
    "--env",
    ENV_STAGING,
];

#[tokio::test]
async fn evaluate_exit_code_is_10_when_the_flag_is_off() {
    let h = Harness::new().await;
    mount_evaluate(&h, false).await;
    let (url, token) = (h.url(), user_token("alice"));
    let mut args = EVALUATE.to_vec();
    args.push("--exit-code");
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
    assert_eq!(r.code, 10, "{}", r.stderr);
    assert_eq!(json_out(&r)["value"], false);
}

#[tokio::test]
async fn evaluate_exit_code_is_0_when_the_flag_is_on() {
    let h = Harness::new().await;
    mount_evaluate(&h, true).await;
    let (url, token) = (h.url(), user_token("alice"));
    let mut args = EVALUATE.to_vec();
    args.push("--exit-code");
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
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn evaluate_resolves_an_environment_name_with_legacy_flags() {
    let h = Harness::new().await;
    mount_environments(&h).await;
    mount_evaluate(&h, true).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &[
                "evaluate",
                "--feature-key",
                "checkout",
                "--environment-id",
                "staging",
                "--targeting-key",
                "user-1",
                "--context",
                "{\"plan\":\"pro\"}",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM_ID", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn evaluate_uses_the_system_token_team_and_refuses_another() {
    let h = Harness::new().await;
    mount_evaluate(&h, true).await;
    let (url, token) = (h.url(), system_token(TEAM_A));
    let r = h
        .run(
            &EVALUATE,
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = h
        .run(
            &EVALUATE,
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_B),
            ],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("does not match"), "{}", r.stderr);
}

#[tokio::test]
async fn evaluate_rejects_a_context_that_is_not_an_object() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &[
                "evaluate",
                "--flag",
                "checkout",
                "--targeting-key",
                "u",
                "--context",
                "[1]",
                "--env",
                ENV_STAGING,
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
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--context must be a JSON object"));
}

#[tokio::test]
async fn rollout_promote_by_flag_maps_an_environment_id_to_its_name() {
    let h = Harness::new().await;
    mount_environments(&h).await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/teams/{TEAM_A}/features/by-key/checkout/environments/staging/request-change"
        )))
        .and(body_json(
            json!({ "request": "DEPLOYED", "reason": "ship it", "externalRef": "PROJ-1" }),
        ))
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
            &[
                "rollout",
                "promote",
                "--flag",
                "checkout",
                "--env",
                ENV_STAGING,
                "--request",
                "deployed",
                "--reason",
                "ship it",
                "--external-ref",
                "PROJ-1",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(json_out(&r)["key"], "checkout");
}

#[tokio::test]
async fn rollout_promote_by_stage_keeps_the_old_form() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/stages/stage-123/request-change"))
        .and(body_json(
            json!({ "request": "DEPLOYMENT_REQUESTED", "freezeOverrideReason": "hotfix" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": FEATURE_ID })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &[
                "rollout",
                "promote",
                "stage-123",
                "--freeze-override-reason",
                "hotfix",
            ],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn rollout_promote_needs_a_stage_or_a_flag() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["rollout", "promote"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
            ],
        )
        .await;
    assert_eq!(r.code, 2);
}

#[tokio::test]
async fn config_export_includes_every_page() {
    let h = Harness::new().await;
    mount_environments(&h).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{ "id": TEAM_A, "name": "Payments" }])),
        )
        .mount(&h.server)
        .await;
    let features_path = format!("/api/v1/teams/{TEAM_A}/features");
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(0..200), "meta": { "offset": 0, "limit": 200, "total": 201 } })))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(features_path.as_str()))
        .and(query_param("offset", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": features(200..201), "meta": { "offset": 200, "limit": 200, "total": 201 } })))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(
            &["config", "export", "--output", "table"],
            &[
                ("FLUXGATE_URL", url.as_str()),
                ("FLUXGATE_TOKEN", token.as_str()),
                ("FLUXGATE_TEAM", TEAM_A),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["team"]["name"], "Payments");
    assert_eq!(value["environments"].as_array().unwrap().len(), 1);
    assert_eq!(value["features"].as_array().unwrap().len(), 201);
}
