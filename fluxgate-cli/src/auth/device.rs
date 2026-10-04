//! Device-code login: the person approves a short code in the web UI, from any
//! machine, while the CLI polls. Works over SSH and in containers.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::session::LoginResponse;
use crate::api::ApiClient;
use crate::error::CliError;
use crate::prompt::Prompter;

/// Seconds added to the interval on each `slow_down`.
const SLOW_DOWN_SECS: u64 = 5;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

/// The poll interval after the server answered `code`, or why login stops.
pub fn next_step(code: &str, interval: u64) -> Result<u64, CliError> {
    match code {
        "authorization_pending" => Ok(interval),
        "slow_down" => Ok(interval + SLOW_DOWN_SECS),
        "access_denied" => Err(CliError::Auth("the login was denied in the browser".into())),
        "expired_token" => Err(CliError::Auth(
            "the device code expired before it was approved: run fluxgate login again".into(),
        )),
        other => Err(CliError::Other(format!(
            "unexpected device login answer: {other}"
        ))),
    }
}

/// Logs in by device code. `open_browser` false only prints the link.
pub async fn device_login(
    prompter: &mut dyn Prompter,
    base_url: &str,
    timeout: Duration,
    open_browser: bool,
) -> Result<LoginResponse, CliError> {
    let api = ApiClient::new(base_url, None, timeout)?;
    let started: DeviceAuthorization = serde_json::from_value(
        api.post(&["auth", "device", "authorize"], &json!({}))
            .await?,
    )
    .map_err(|err| CliError::Other(format!("unexpected device authorize response: {err}")))?;
    prompter.notify(&format!(
        "To log in, open {} and enter the code {}\n(or open {})",
        started.verification_uri, started.user_code, started.verification_uri_complete
    ));
    if open_browser {
        prompter.open_browser(&started.verification_uri_complete);
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(started.expires_in);
    let mut interval = started.interval;
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err(next_step("expired_token", interval).unwrap_err());
        }
        match api
            .post(
                &["auth", "device", "token"],
                &json!({ "deviceCode": started.device_code }),
            )
            .await
        {
            Ok(value) => {
                return serde_json::from_value(value).map_err(|err| {
                    CliError::Other(format!("unexpected device token response: {err}"))
                });
            }
            Err(CliError::Api {
                status: 400, code, ..
            }) => interval = next_step(&code, interval)?,
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_AUTH;

    #[test]
    fn poll_answers_decide_the_next_step() {
        assert_eq!(next_step("authorization_pending", 5).unwrap(), 5);
        assert_eq!(next_step("slow_down", 5).unwrap(), 10);
        let denied = next_step("access_denied", 5).unwrap_err();
        assert_eq!(denied.exit_code(), EXIT_AUTH);
        assert!(denied.to_string().contains("denied"));
        let expired = next_step("expired_token", 5).unwrap_err();
        assert!(expired.to_string().contains("expired"));
        assert_eq!(
            next_step("something_else", 5).unwrap_err().exit_code(),
            crate::error::EXIT_OTHER
        );
    }
}
