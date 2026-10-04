//! SSO login through the browser: the backend sends the one-time code to a
//! local callback address, and the code is exchanged with a PKCE verifier.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use super::session::LoginResponse;
use crate::api::ApiClient;
use crate::error::CliError;
use crate::prompt::Prompter;

/// How long to wait for the browser to come back.
pub const LOGIN_WAIT: Duration = Duration::from_secs(300);
/// Longest request line read from the browser.
const MAX_REQUEST_LINE: u64 = 8192;

/// A PKCE verifier: 32 random bytes, base64url without padding (43 characters).
pub fn new_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The S256 challenge of `verifier`.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code(String),
    /// The backend's error code, reduced to printable characters.
    Error(String),
    /// Anything else the browser asks for, such as `/favicon.ico`.
    Other,
}

/// Reads the request target of the browser's call to the local callback.
pub fn parse_callback(target: &str) -> Callback {
    let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Other;
    };
    if url.path() != "/callback" {
        return Callback::Other;
    }
    let mut code = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" if !value.is_empty() => code = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    match (code, error) {
        (_, Some(error)) => Callback::Error(
            error
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .take(64)
                .collect(),
        ),
        (Some(code), None) => Callback::Code(code),
        (None, None) => Callback::Other,
    }
}

/// Logs in through provider `slug` and returns the session.
pub async fn sso_login(
    prompter: &mut dyn Prompter,
    base_url: &str,
    slug: &str,
    timeout: Duration,
    wait: Duration,
) -> Result<LoginResponse, CliError> {
    // A mistyped slug would otherwise end on a browser error page while the
    // terminal waits; older backends without the list are not checked.
    let api = ApiClient::new(base_url, None, timeout)?;
    if let Ok(serde_json::Value::Array(providers)) =
        api.get(&["auth", "sso", "providers"], &[]).await
    {
        let slugs: Vec<&str> = providers
            .iter()
            .filter_map(|provider| provider.get("slug").and_then(serde_json::Value::as_str))
            .collect();
        if !slugs.contains(&slug) {
            return Err(CliError::Usage(format!(
                "unknown SSO provider '{slug}'; available: {}",
                if slugs.is_empty() {
                    "none".to_string()
                } else {
                    slugs.join(", ")
                }
            )));
        }
    }
    // Only this machine can reach the callback.
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let callback = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    let verifier = new_verifier();
    let mut authorize = api.url(&["auth", "sso", slug, "authorize"]);
    authorize
        .query_pairs_mut()
        .append_pair("cli_redirect", &callback)
        .append_pair("cli_challenge", &challenge_for(&verifier));

    prompter.notify(&format!(
        "Opening the browser to log in with '{slug}'. If it does not open, visit:\n{authorize}"
    ));
    prompter.open_browser(authorize.as_str());

    let code = tokio::time::timeout(wait, wait_for_code(&listener))
        .await
        .map_err(|_| CliError::Auth("SSO login timed out waiting for the browser".into()))??;
    let value = api
        .post(
            &["auth", "sso", "exchange"],
            &json!({ "code": code, "codeVerifier": verifier }),
        )
        .await
        .map_err(|err| match err {
            CliError::Api {
                status: 401, code, ..
            } => CliError::Auth(format!("SSO login failed: {code}")),
            other => other,
        })?;
    serde_json::from_value(value)
        .map_err(|err| CliError::Other(format!("unexpected exchange response: {err}")))
}

/// Serves the local callback until the browser brings a code or an error.
pub async fn wait_for_code(listener: &TcpListener) -> Result<String, CliError> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut line = String::new();
        BufReader::new(&mut stream)
            .take(MAX_REQUEST_LINE)
            .read_line(&mut line)
            .await?;
        let target = line.split_whitespace().nth(1).unwrap_or_default();
        match parse_callback(target) {
            Callback::Code(code) => {
                respond(
                    &mut stream,
                    "200 OK",
                    "Login complete. You can close this tab and return to the terminal.",
                )
                .await;
                return Ok(code);
            }
            Callback::Error(error) => {
                respond(
                    &mut stream,
                    "200 OK",
                    &format!("Login failed ({error}). Return to the terminal for details."),
                )
                .await;
                return Err(CliError::Auth(format!("SSO login failed: {error}")));
            }
            Callback::Other => respond(&mut stream, "404 Not Found", "Not found").await,
        }
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>FluxGate</title></head><body><p>{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_AUTH;
    use crate::prompt::ScriptedPrompter;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn verifier_is_43_url_safe_characters_and_challenge_is_its_s256() {
        let verifier = new_verifier();
        assert_eq!(verifier.len(), 43);
        assert!(
            verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(new_verifier(), verifier);
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn parses_callback_targets() {
        assert_eq!(
            parse_callback("/callback?code=abc"),
            Callback::Code("abc".into())
        );
        assert_eq!(
            parse_callback("/callback?error=sso_email_missing"),
            Callback::Error("sso_email_missing".into())
        );
        assert_eq!(
            parse_callback("/callback?error=%3Cscript%3E"),
            Callback::Error("script".into())
        );
        assert_eq!(parse_callback("/callback"), Callback::Other);
        assert_eq!(parse_callback("/favicon.ico"), Callback::Other);
    }

    #[tokio::test]
    async fn other_requests_get_404_until_the_callback_arrives() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut first = tokio::net::TcpStream::connect(address).await.unwrap();
            first
                .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut response = String::new();
            first.read_to_string(&mut response).await.unwrap();
            let mut second = tokio::net::TcpStream::connect(address).await.unwrap();
            second
                .write_all(b"GET /callback?code=abc HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut done = String::new();
            second.read_to_string(&mut done).await.unwrap();
            (response, done)
        });
        assert_eq!(wait_for_code(&listener).await.unwrap(), "abc");
        let (first, second) = client.await.unwrap();
        assert!(first.starts_with("HTTP/1.1 404"), "{first}");
        assert!(
            second.starts_with("HTTP/1.1 200") && second.contains("close this tab"),
            "{second}"
        );
    }

    #[tokio::test]
    async fn waiting_gives_up_after_the_timeout() {
        let mut prompter = ScriptedPrompter::new(&[]);
        let err = sso_login(
            &mut prompter,
            "http://127.0.0.1:9/api/v1",
            "okta",
            Duration::from_secs(1),
            Duration::from_millis(100),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(err.exit_code(), EXIT_AUTH);
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(
            prompter
                .notes
                .iter()
                .any(|note| note.contains("/api/v1/auth/sso/okta/authorize?"))
        );
    }
}
