mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

async fn mount_login(h: &Harness, password: &str, body: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(
            json!({ "username": "alice", "password": password }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn password_login_writes_the_session_and_links_the_profile() {
    let h = Harness::new().await;
    h.write("config", "[default]\nteam = payments\n");
    mount_login(&h, "pw", login_body("a1", "r1")).await;
    let url = h.url();
    let r = h
        .run_with(
            &[
                "login",
                "--password",
                "--username",
                "alice",
                "--output",
                "text",
            ],
            &[("FLUXGATE_URL", url.as_str())],
            &["pw"],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged in as alice (session 'default')");
    let session: serde_json::Value =
        serde_json::from_str(&h.read("sessions/default.json")).unwrap();
    assert_eq!(session["accessToken"], "a1");
    let config = h.read("config");
    assert!(config.contains("session=default"), "{config}");
    assert!(config.contains("[session default]"), "{config}");
    assert!(config.contains(&format!("url={url}")), "{config}");
}

#[tokio::test]
async fn later_commands_use_and_refresh_the_session() {
    let h = Harness::new().await;
    h.write(
        "config",
        &format!(
            "[default]\nsession = corp\nteam = {TEAM_A}\n\n[session corp]\nurl = {}\n",
            h.url()
        ),
    );
    h.write_session("corp", "a1", "r1", -5);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({ "refreshToken": "r1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a2", "r2")))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/teams/{TEAM_A}/features")))
        .and(header("authorization", "Bearer a2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "items": [], "meta": { "offset": 0, "limit": 50, "total": 0 } }),
        ))
        .expect(1)
        .mount(&h.server)
        .await;
    let r = h.run(&["flags", "list"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn temporary_password_is_changed_then_login_repeats() {
    let h = Harness::new().await;
    let mut temporary = login_body("temp", "temp-refresh");
    temporary["isTemporary"] = json!(true);
    mount_login(&h, "old", temporary).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/reset-password"))
        .and(header("authorization", "Bearer temp"))
        .and(body_json(
            json!({ "currentPassword": "old", "newPassword": "new-pass" }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&h.server)
        .await;
    mount_login(&h, "new-pass", login_body("a1", "r1")).await;
    let url = h.url();
    let r = h
        .run_with(
            &["login", "--password", "--username", "alice"],
            &[("FLUXGATE_URL", url.as_str())],
            &["old", "new-pass", "new-pass"],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.read("sessions/default.json").contains("\"a1\""));
}

#[tokio::test]
async fn mismatched_new_passwords_stop_the_login() {
    let h = Harness::new().await;
    let mut temporary = login_body("temp", "temp-refresh");
    temporary["isTemporary"] = json!(true);
    mount_login(&h, "old", temporary).await;
    let url = h.url();
    let r = h
        .run_with(
            &[
                "login",
                "--password",
                "--username",
                "alice",
                "--output",
                "text",
            ],
            &[("FLUXGATE_URL", url.as_str())],
            &["old", "a", "b"],
        )
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("do not match"));
    assert!(!h.exists("sessions/default.json"));
}

#[tokio::test]
async fn wrong_password_exits_3() {
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
            &["login", "--username", "alice", "--output", "text"],
            &[("FLUXGATE_URL", url.as_str())],
            &["bad"],
        )
        .await;
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("login failed: invalid credentials"));
}

#[tokio::test]
async fn sso_sessions_need_the_password_flag_in_this_version() {
    let h = Harness::new().await;
    h.write(
        "config",
        &format!(
            "[default]\nsession = corp\n\n[session corp]\nurl = {}\nsso_provider = okta\n",
            h.url()
        ),
    );
    let r = h.run(&["login", "--output", "text"], &[]).await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("fluxgate login --password"));
}

#[tokio::test]
async fn logout_revokes_the_refresh_token_and_deletes_the_cache() {
    let h = Harness::new().await;
    h.write(
        "config",
        &format!(
            "[default]\nsession = corp\n\n[session corp]\nurl = {}\n",
            h.url()
        ),
    );
    h.write_session("corp", "a1", "r1", 600);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(header("authorization", "Bearer a1"))
        .and(body_json(json!({ "refreshToken": "r1" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&h.server)
        .await;
    let r = h.run(&["logout", "--output", "text"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged out of 'corp'");
    assert!(!h.exists("sessions/corp.json"));
}

#[tokio::test]
async fn logout_deletes_the_cache_even_when_the_server_fails() {
    let h = Harness::new().await;
    h.write(
        "config",
        &format!(
            "[default]\nsession = corp\n\n[session corp]\nurl = {}\n",
            h.url()
        ),
    );
    h.write_session("corp", "a1", "r1", 600);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(json!({ "error": "internal", "message": "boom" })),
        )
        .mount(&h.server)
        .await;
    let r = h.run(&["logout"], &[]).await;
    assert_eq!(r.code, 0);
    assert!(
        r.stderr
            .contains("warning: server logout failed for session 'corp'")
    );
    assert!(!h.exists("sessions/corp.json"));
}

#[tokio::test]
async fn logout_all_ends_every_cached_session() {
    let h = Harness::new().await;
    let url = h.url();
    h.write(
        "config",
        &format!("[session one]\nurl = {url}\n\n[session two]\nurl = {url}\n"),
    );
    h.write_session("one", "a1", "r1", 600);
    h.write_session("two", "a2", "r2", 600);
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&h.server)
        .await;
    let r = h.run(&["logout", "--all", "--output", "text"], &[]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged out of 'one', 'two'");
}
