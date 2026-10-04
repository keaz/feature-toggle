mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, ResponseTemplate};

fn sso_config(h: &Harness, provider: Option<&str>) {
    let provider = provider
        .map(|p| format!("sso_provider = {p}\n"))
        .unwrap_or_default();
    h.write(
        "config",
        &format!(
            "[default]\nsession = corp\n\n[session corp]\nurl = {}\n{provider}",
            h.url()
        ),
    );
}

async fn mount_exchange(h: &Harness) {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/sso/exchange"))
        .and(body_partial_json(json!({ "code": "one-time" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")))
        .expect(1)
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn sso_login_exchanges_the_loopback_code_with_the_verifier() {
    let h = Harness::new().await;
    sso_config(&h, Some("okta"));
    mount_exchange(&h).await;
    let (mut prompter, challenge) = browser("code=one-time");
    let r = h
        .run_prompted(&["login", "--output", "text"], &[], &mut prompter)
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged in as alice (session 'corp')");
    assert!(
        prompter.opened[0].contains("/api/v1/auth/sso/okta/authorize?"),
        "{:?}",
        prompter.opened
    );
    assert!(h.read("sessions/corp.json").contains("\"a1\""));
    let requests = h.server.received_requests().await.unwrap();
    let exchange = requests
        .iter()
        .find(|r| r.url.path() == "/api/v1/auth/sso/exchange")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&exchange.body).unwrap();
    let verifier = body["codeVerifier"].as_str().unwrap();
    assert_eq!(verifier.len(), 43);
    assert_eq!(Some(challenge_of(verifier)), *challenge.lock().unwrap());
}

#[tokio::test]
async fn sso_login_reports_the_error_sent_to_the_loopback_address() {
    let h = Harness::new().await;
    sso_config(&h, Some("okta"));
    let (mut prompter, _) = browser("error=sso_user_not_provisioned");
    let r = h
        .run_prompted(&["login", "--output", "text"], &[], &mut prompter)
        .await;
    assert_eq!(r.code, 3);
    assert!(
        r.stderr
            .contains("SSO login failed: sso_user_not_provisioned"),
        "{}",
        r.stderr
    );
    assert!(!h.exists("sessions/corp.json"));
}

#[tokio::test]
async fn login_sso_with_a_slug_remembers_the_provider() {
    let h = Harness::new().await;
    sso_config(&h, None);
    mount_exchange(&h).await;
    let (mut prompter, _) = browser("code=one-time");
    let r = h
        .run_prompted(&["login", "--sso", "okta"], &[], &mut prompter)
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        h.read("config").contains("sso_provider=okta"),
        "{}",
        h.read("config")
    );
}

#[tokio::test]
async fn login_sso_without_a_provider_is_a_usage_error() {
    let h = Harness::new().await;
    sso_config(&h, None);
    let r = h.run(&["login", "--sso", "--output", "text"], &[]).await;
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--sso <slug>"), "{}", r.stderr);
}

#[tokio::test]
async fn password_flag_overrides_the_session_provider() {
    let h = Harness::new().await;
    sso_config(&h, Some("okta"));
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")))
        .expect(1)
        .mount(&h.server)
        .await;
    let r = h
        .run_with(
            &["login", "--password", "--username", "alice"],
            &[],
            &["pw"],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[tokio::test]
async fn interactive_sso_setup_picks_a_provider_and_logs_in() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/sso/providers"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "slug": "okta", "displayName": "Okta" }])),
        )
        .mount(&h.server)
        .await;
    mount_exchange(&h).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/teams"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&h.server)
        .await;
    let url = h.url();
    let (prompter, _) = browser("code=one-time");
    let mut prompter =
        prompter.with_answers(&[url.as_str(), "Single sign-on (SSO)", "Okta", "table"]);
    let r = h
        .run_prompted(&["configure", "--output", "text"], &[], &mut prompter)
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    let config = h.read("config");
    assert!(
        config.contains("sso_provider=okta") && config.contains("session=default"),
        "{config}"
    );
    assert!(h.exists("sessions/default.json"));
}

// ---------------------------------------------------------- device code

async fn mount_device(h: &Harness, token_answers: Vec<ResponseTemplate>) {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/device/authorize"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "deviceCode": "device-secret", "userCode": "BCDF-GHJK",
            "verificationUri": "http://ui.test/device",
            "verificationUriComplete": "http://ui.test/device?code=BCDF-GHJK",
            "expiresIn": 600, "interval": 0 })))
        .expect(1)
        .mount(&h.server)
        .await;
    let count = token_answers.len() as u64;
    for (index, answer) in token_answers.into_iter().enumerate() {
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/device/token"))
            .and(body_partial_json(json!({ "deviceCode": "device-secret" })))
            .respond_with(answer)
            .up_to_n_times(1)
            .with_priority((index + 1) as u8)
            .expect(1)
            .mount(&h.server)
            .await;
    }
    let _ = count;
}

fn pending() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(
        json!({ "error": "invalid_input", "message": "waiting", "code": "authorization_pending" }),
    )
}

#[tokio::test]
async fn device_login_waits_for_approval_and_saves_the_session() {
    let h = Harness::new().await;
    sso_config(&h, None);
    mount_device(
        &h,
        vec![
            pending(),
            ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")),
        ],
    )
    .await;
    let mut prompter = fluxgate_cli::prompt::ScriptedPrompter::new(&[]).on_open(|_| {});
    let r = h
        .run_prompted(
            &["login", "--use-device-code", "--output", "text"],
            &[],
            &mut prompter,
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "Logged in as alice (session 'corp')");
    assert!(
        prompter
            .notes
            .iter()
            .any(|n| n.contains("BCDF-GHJK") && n.contains("http://ui.test/device")),
        "{:?}",
        prompter.notes
    );
    assert_eq!(
        prompter.opened,
        vec!["http://ui.test/device?code=BCDF-GHJK"]
    );
    assert!(h.read("sessions/corp.json").contains("\"a1\""));
}

#[tokio::test]
async fn device_login_reports_a_denial() {
    let h = Harness::new().await;
    sso_config(&h, None);
    mount_device(
        &h,
        vec![ResponseTemplate::new(400).set_body_json(
            json!({ "error": "invalid_input", "message": "denied", "code": "access_denied" }),
        )],
    )
    .await;
    let r = h
        .run(&["login", "--use-device-code", "--output", "text"], &[])
        .await;
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("denied in the browser"), "{}", r.stderr);
}

#[tokio::test]
async fn no_browser_with_an_sso_session_uses_the_device_code() {
    let h = Harness::new().await;
    sso_config(&h, Some("okta"));
    mount_device(
        &h,
        vec![ResponseTemplate::new(200).set_body_json(login_body("a1", "r1"))],
    )
    .await;
    let mut prompter = fluxgate_cli::prompt::ScriptedPrompter::new(&[]).on_open(|_| {});
    let r = h
        .run_prompted(&["login", "--no-browser"], &[], &mut prompter)
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(prompter.opened.is_empty(), "{:?}", prompter.opened);
}

#[tokio::test]
async fn interactive_device_setup_logs_in_without_a_password() {
    let h = Harness::new().await;
    mount_device(
        &h,
        vec![ResponseTemplate::new(200).set_body_json(login_body("a1", "r1"))],
    )
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
            &[url.as_str(), "Approve in a browser (device code)", "table"],
        )
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(h.exists("sessions/default.json"));
}

#[tokio::test]
async fn sso_login_with_an_unknown_provider_fails_fast() {
    let h = Harness::new().await;
    sso_config(&h, Some("otka"));
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/sso/providers"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "slug": "okta", "displayName": "Okta" }])),
        )
        .mount(&h.server)
        .await;
    let mut prompter = fluxgate_cli::prompt::ScriptedPrompter::new(&[]).on_open(|_| {});
    let r = h
        .run_prompted(&["login", "--output", "text"], &[], &mut prompter)
        .await;
    assert_eq!(r.code, 2);
    assert!(
        r.stderr
            .contains("unknown SSO provider 'otka'; available: okta"),
        "{}",
        r.stderr
    );
    assert!(prompter.opened.is_empty());
}

#[tokio::test]
async fn device_login_keeps_polling_through_rate_limits() {
    let h = Harness::new().await;
    sso_config(&h, None);
    let limited = ResponseTemplate::new(429)
        .insert_header("retry-after", "0")
        .set_body_json(json!({ "error": "rate_limited", "message": "Rate limited" }));
    mount_device(
        &h,
        vec![
            limited,
            ResponseTemplate::new(200).set_body_json(login_body("a1", "r1")),
        ],
    )
    .await;
    let r = h
        .run(&["login", "--use-device-code", "--no-browser"], &[])
        .await;
    assert_eq!(r.code, 0, "{}", r.stderr);
}
