//! Username and password login, including the temporary-password change.

use std::time::Duration;

use serde_json::json;

use super::session::LoginResponse;
use crate::api::ApiClient;
use crate::error::CliError;
use crate::prompt::Prompter;

pub async fn password_login(
    prompter: &mut dyn Prompter,
    base_url: &str,
    username: &str,
    timeout: Duration,
) -> Result<LoginResponse, CliError> {
    let api = ApiClient::new(base_url, None, timeout)?;
    let password = prompter.secret("Password")?;
    let response = login(&api, username, &password).await?;
    if !response.is_temporary {
        return Ok(response);
    }

    let new_password = prompter.secret("New password (the current one is temporary)")?;
    let confirm = prompter.secret("Confirm new password")?;
    if new_password != confirm {
        return Err(CliError::Usage("the new passwords do not match".into()));
    }
    let temporary = api.with_token(Some(response.token.clone()));
    temporary
        .post(
            &["auth", "reset-password"],
            &json!({ "currentPassword": password, "newPassword": new_password }),
        )
        .await?;
    // The reset answers 204 without a session: end the temporary one and log
    // in again with the new password.
    let _ = temporary
        .post(
            &["auth", "logout"],
            &json!({ "refreshToken": response.refresh_token }),
        )
        .await;
    login(&api, username, &new_password).await
}

async fn login(api: &ApiClient, username: &str, password: &str) -> Result<LoginResponse, CliError> {
    let value = api
        .post(
            &["auth", "login"],
            &json!({ "username": username, "password": password }),
        )
        .await
        .map_err(|err| match err {
            CliError::Api {
                status: 401,
                message,
                ..
            } => CliError::Auth(format!("login failed: {message}")),
            other => other,
        })?;
    serde_json::from_value(value)
        .map_err(|err| CliError::Other(format!("unexpected login response: {err}")))
}
