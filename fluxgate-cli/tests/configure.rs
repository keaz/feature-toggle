mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn set_then_get_a_profile_value() {
    let h = Harness::new().await;
    let r = h.run(&["configure", "set", "team", "payments"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = h
        .run(&["configure", "get", "team", "--output", "text"], &[])
        .await;
    assert_eq!(r.stdout.trim(), "payments");
    let r = h
        .run(
            &[
                "--profile",
                "prod",
                "configure",
                "set",
                "url",
                "https://fg.example.com/api/v1",
            ],
            &[],
        )
        .await;
    assert_eq!(r.code, 0);
    assert!(h.read("config").contains("[profile prod]"));
}

#[tokio::test]
async fn set_rejects_bad_keys_and_values() {
    let h = Harness::new().await;
    let r = h
        .run(
            &["configure", "set", "colour", "blue", "--output", "text"],
            &[],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("unknown key 'colour'"), "{}", r.stderr);
    assert_eq!(
        h.run(&["configure", "set", "output", "yaml"], &[])
            .await
            .code,
        2
    );
    assert_eq!(
        h.run(&["configure", "set", "timeout", "soon"], &[])
            .await
            .code,
        2
    );
}

#[tokio::test]
async fn get_of_an_unset_key_exits_1() {
    let h = Harness::new().await;
    let r = h
        .run(
            &["configure", "get", "environment", "--output", "text"],
            &[],
        )
        .await;
    assert_eq!(r.code, 1);
    assert!(
        r.stderr
            .contains("environment is not set for profile 'default'")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn set_token_goes_to_the_private_credentials_file_and_get_masks_it() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new().await;
    let r = h
        .run(
            &[
                "--profile",
                "ci",
                "configure",
                "set",
                "token",
                "abcdef123456",
            ],
            &[],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.read("credentials").contains("[ci]"));
    let mode = std::fs::metadata(h.path("credentials"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let r = h
        .run(
            &[
                "--profile",
                "ci",
                "configure",
                "get",
                "token",
                "--output",
                "text",
            ],
            &[],
        )
        .await;
    assert_eq!(r.stdout.trim(), "****3456");
}

#[tokio::test]
async fn list_shows_values_and_their_sources() {
    let h = Harness::new().await;
    h.write("config", "[default]\nurl = https://fg.example.com/api/v1\n");
    let r = h
        .run(
            &["configure", "list"],
            &[
                ("FLUXGATE_TEAM", "payments"),
                ("FLUXGATE_TOKEN", "abcdef123456"),
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let items = json_out(&r)["items"].as_array().unwrap().clone();
    let find = |name: &str| items.iter().find(|i| i["name"] == name).unwrap().clone();
    assert_eq!(find("url")["source"], "profile");
    assert_eq!(find("team")["value"], "payments");
    assert_eq!(find("team")["source"], "env");
    assert_eq!(find("environment")["source"], "unset");
    assert_eq!(find("token")["value"], "****3456 (static)");
    assert!(!r.stdout.contains("abcdef123456"));
}

#[tokio::test]
async fn list_profiles_marks_the_active_one() {
    let h = Harness::new().await;
    h.write(
        "config",
        "[default]\nteam = a\n\n[profile prod]\nteam = b\n",
    );
    h.write("credentials", "[ci]\ntoken = t\n");
    let r = h
        .run(
            &["configure", "list-profiles"],
            &[("FLUXGATE_PROFILE", "prod")],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let items = json_out(&r)["items"].as_array().unwrap().clone();
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["default", "ci", "prod"]);
    assert_eq!(items[2]["active"], true);
    assert_eq!(items[0]["active"], false);
}

#[tokio::test]
async fn interactive_password_setup_logs_in_and_picks_team_and_environment() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(json!({ "username": "alice", "password": "pw" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .and(header("authorization", "Bearer a1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": TEAM_A, "name": "Payments" }, { "id": TEAM_B, "name": "Checkout" } ])))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "active": true }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h
        .run_with(
            &["configure", "--output", "text"],
            &[],
            &[
                url.as_str(),
                "Log in with username and password",
                "alice",
                "pw",
                "Payments",
                "staging",
                "table",
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let config = h.read("config");
    for expected in [
        "session=default",
        "team=Payments",
        "environment=staging",
        "output=table",
        "[session default]",
    ] {
        assert!(config.contains(expected), "missing {expected} in {config}");
    }
    assert!(h.exists("sessions/default.json"));
}

#[tokio::test]
async fn interactive_token_setup_stores_the_token_and_the_token_team() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "active": true }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
    let (url, token) = (h.url(), system_token(TEAM_A));
    let r = h
        .run_with(
            &["--profile", "ci", "configure"],
            &[],
            &[
                url.as_str(),
                "Static token (system client)",
                token.as_str(),
                "staging",
                "json",
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.read("credentials").contains("[ci]"));
    let config = h.read("config");
    assert!(
        config.contains("[profile ci]") && config.contains(&format!("team={TEAM_A}")),
        "{config}"
    );
}

#[tokio::test]
async fn interactive_setup_checks_the_new_profile_not_env_credentials() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .and(header("authorization", "Bearer a1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{ "id": TEAM_A, "name": "Payments" }])),
        )
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/environments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "id": ENV_STAGING, "name": "staging", "active": true }],
            "meta": { "offset": 0, "limit": 200, "total": 1 } })))
        .mount(&h.server)
        .await;
    let (url, env_token) = (h.url(), system_token(TEAM_B));
    // A CI token in the environment must not decide the profile's team.
    let r = h
        .run_with(
            &["configure"],
            &[("FLUXGATE_TOKEN", env_token.as_str())],
            &[
                url.as_str(),
                "Log in with username and password",
                "alice",
                "pw",
                "Payments",
                "staging",
                "table",
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let config = h.read("config");
    assert!(config.contains("team=Payments"), "{config}");
    assert!(!config.contains(TEAM_B), "{config}");
}

#[tokio::test]
async fn set_url_rejects_an_invalid_url() {
    let h = Harness::new().await;
    let r = h
        .run(
            &["configure", "set", "url", "not a url", "--output", "text"],
            &[],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("invalid url"), "{}", r.stderr);
}

#[tokio::test]
async fn failed_login_during_setup_leaves_the_config_unchanged() {
    let h = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(
                json!({ "error": "unauthorized", "message": "invalid credentials" }),
            ),
        )
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h
        .run_with(
            &["configure", "--output", "text"],
            &[],
            &[
                url.as_str(),
                "Log in with username and password",
                "alice",
                "bad",
            ],
        )
        .await;
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert!(!h.exists("config"), "{}", h.read("config"));
}

#[tokio::test]
async fn changing_a_shared_session_url_warns_about_other_profiles() {
    let h = Harness::new().await;
    h.write("config", "[default]\nsession = corp\n\n[profile other]\nsession = corp\n\n[session corp]\nurl = https://old.example.com/api/v1\n");
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")))
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&h.server)
        .await;
    let url = h.url();
    let r = h
        .run_with(
            &["configure", "--output", "text"],
            &[],
            &[
                url.as_str(),
                "Log in with username and password",
                "alice",
                "pw",
                "table",
            ],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stderr.contains("also used by profile 'other'"),
        "{}",
        r.stderr
    );
}

#[tokio::test]
async fn edge_settings_are_stored_like_url_and_token() {
    let h = Harness::new().await;
    assert_eq!(
        h.run(
            &["configure", "set", "edge_url", "https://edge.example.com"],
            &[]
        )
        .await
        .code,
        0
    );
    assert_eq!(
        h.run(&["configure", "set", "edge_key", "client.secret-1234"], &[])
            .await
            .code,
        0
    );
    assert!(
        h.read("config")
            .contains("edge_url=https://edge.example.com")
    );
    assert!(
        h.read("credentials")
            .contains("edge_key=client.secret-1234")
    );
    let r = h
        .run(&["configure", "get", "edge_key", "--output", "text"], &[])
        .await;
    assert_eq!(r.stdout.trim(), "****1234");
}
