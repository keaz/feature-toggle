//! Bounded HTTP helpers for talking to OIDC identity providers.
//!
//! Every response body read from an IdP (discovery document, JWKS, token response,
//! userinfo) goes through [`read_json_capped`], which stops reading after
//! [`MAX_RESPONSE_BYTES`], so a misbehaving or hostile IdP cannot exhaust memory.
//! Error values never contain URLs, request bodies or response bodies, so they are
//! safe to log and to show to administrators.

use serde::de::DeserializeOwned;
use std::time::Duration;

/// Largest IdP response body that is read (1 MiB).
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Timeout for each request to an IdP.
pub const IDP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Why an IdP request failed. Messages are short and contain no URLs or bodies.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("timed out")]
    Timeout,
    #[error("connection failed")]
    Connect,
    #[error("request failed")]
    Request,
    #[error("HTTP {0}")]
    Status(u16),
    #[error("response body exceeds 1 MiB")]
    TooLarge,
    #[error("response is not valid JSON")]
    InvalidJson,
}

impl From<reqwest::Error> for FetchError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            FetchError::Timeout
        } else if err.is_connect() {
            FetchError::Connect
        } else {
            FetchError::Request
        }
    }
}

/// HTTP client for IdP calls: request timeout, no redirects followed (an IdP
/// endpoint that redirects is treated as an error rather than followed blindly).
pub fn idp_http_client() -> Result<reqwest::Client, FetchError> {
    reqwest::Client::builder()
        .timeout(IDP_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| FetchError::Request)
}

/// Reads at most [`MAX_RESPONSE_BYTES`] of the body. Fails with
/// [`FetchError::TooLarge`] as soon as the declared length or the bytes read so far
/// exceed the limit.
pub async fn read_body_capped(mut response: reqwest::Response) -> Result<Vec<u8>, FetchError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        return Err(FetchError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(FetchError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Checks the status, then reads a capped body and parses it as JSON.
pub async fn read_json_capped<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, FetchError> {
    let status = response.status();
    if !status.is_success() {
        return Err(FetchError::Status(status.as_u16()));
    }
    let body = read_body_capped(response).await?;
    serde_json::from_slice(&body).map_err(|_| FetchError::InvalidJson)
}

/// GETs `url` and parses the capped JSON body.
pub async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, FetchError> {
    let response = client.get(url).send().await?;
    read_json_capped(response).await
}
