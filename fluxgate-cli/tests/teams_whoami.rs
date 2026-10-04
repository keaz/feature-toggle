mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn mount_teams(h: &Harness) {
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": TEAM_A, "name": "Payments" }, { "id": TEAM_B, "name": "Checkout" } ])))
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn teams_list_marks_the_active_team() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["teams", "list"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", "checkout")]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let items = json_out(&r)["items"].as_array().unwrap().clone();
    assert_eq!(items[0]["active"], false);
    assert_eq!(items[1]["active"], true);
}

#[tokio::test]
async fn teams_use_saves_the_team_on_the_profile() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["teams", "use", "Checkout", "--output", "text"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), format!("Profile 'default' now uses team Checkout ({TEAM_B})"));
    assert!(h.read("config").contains("team=Checkout"));
}

#[tokio::test]
async fn teams_use_of_an_unknown_team_lists_the_choices() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h
        .run(&["teams", "use", "billing", "--output", "text"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())])
        .await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("available: Payments, Checkout"));
}

#[tokio::test]
async fn whoami_for_a_user_shows_the_active_team_and_all_teams() {
    let h = Harness::new().await;
    mount_teams(&h).await;
    let (url, token) = (h.url(), user_token("alice"));
    let r = h.run(&["whoami"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str()), ("FLUXGATE_TEAM", "payments")]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["kind"], "user");
    assert_eq!(value["username"], "alice");
    assert_eq!(value["team"], format!("Payments ({TEAM_A})"));
    assert_eq!(value["teams"], json!(["Payments", "Checkout"]));
    assert_eq!(value["credential"], "static token (env)");
    assert_eq!(value["tokenExpiresAt"], "2100-01-01T00:00:00+00:00");
}

#[tokio::test]
async fn whoami_for_a_system_client_needs_no_teams_request() {
    let h = Harness::new().await;
    let (url, token) = (h.url(), system_token(TEAM_A));
    let r = h.run(&["whoami"], &[("FLUXGATE_URL", url.as_str()), ("FLUXGATE_TOKEN", token.as_str())]).await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let value = json_out(&r);
    assert_eq!(value["kind"], "system_client");
    assert_eq!(value["team"], TEAM_A);
}
