//! Outbound HTTP client for Jira (write-back). It sends the stored credential and
//! nothing else: the credential never appears in `Debug` output, errors or logs.

use std::fmt;
use std::time::Duration;

use crate::Error;
use crate::database::entity::JiraIntegrationRow;
use crate::logic::secret_box;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_EXCERPT_CHARS: usize = 300;

/// Jira edition, selected by `jira_auth_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JiraEdition {
    Cloud,
    DataCenter,
}

impl JiraEdition {
    /// `cloud_basic` is Cloud, `dc_pat` is Data Center; anything else is `None`.
    pub fn from_auth_kind(kind: &str) -> Option<Self> {
        match kind {
            "cloud_basic" => Some(Self::Cloud),
            "dc_pat" => Some(Self::DataCenter),
            _ => None,
        }
    }

    /// REST API version: Cloud v3 (ADF comments), Data Center v2 (plain text).
    pub fn api_version(self) -> &'static str {
        match self {
            Self::Cloud => "3",
            Self::DataCenter => "2",
        }
    }
}

/// The credential of a client. `Debug` hides the token.
pub enum JiraAuth {
    Basic { email: String, token: String },
    Bearer { token: String },
}

impl fmt::Debug for JiraAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Basic { email, .. } => f
                .debug_struct("Basic")
                .field("email", email)
                .field("token", &"***")
                .finish(),
            Self::Bearer { .. } => f.debug_struct("Bearer").field("token", &"***").finish(),
        }
    }
}

pub struct JiraClient {
    http: reqwest::Client,
    base_url: String,
    edition: JiraEdition,
    auth: JiraAuth,
}

impl fmt::Debug for JiraClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JiraClient")
            .field("base_url", &self.base_url)
            .field("edition", &self.edition)
            .field("auth", &self.auth)
            .finish()
    }
}

/// A Jira reply of any status. The body is cut to 300 characters.
#[derive(Debug, PartialEq, Eq)]
pub struct JiraResponse {
    pub status: u16,
    pub retry_after_secs: Option<u64>,
    pub body_excerpt: String,
}

/// Timeout or connect error. The message never contains the auth value.
#[derive(Debug)]
pub struct JiraTransportError(pub String);

impl fmt::Display for JiraTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JiraClient {
    /// Timeout 10 s. Redirects are not followed: the credential must not leave the
    /// configured host.
    pub fn new(base_url: &str, edition: JiraEdition, auth: JiraAuth) -> Result<Self, Error> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        if reqwest::Url::parse(&base_url).is_err() {
            return Err(Error::InvalidInput(
                "jira base URL must be a valid URL".to_string(),
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::InvalidInput("could not build the Jira client".to_string()))?;
        Ok(Self {
            http,
            base_url,
            edition,
            auth,
        })
    }

    /// `GET rest/api/{v}/myself`.
    pub async fn myself(&self) -> Result<JiraResponse, JiraTransportError> {
        self.get_myself().await.map(|(response, _)| response)
    }

    /// Like [`JiraClient::myself`], plus `displayName` read from the full body of a
    /// 2xx reply (the excerpt is too short to hold it).
    pub async fn myself_display_name(
        &self,
    ) -> Result<(JiraResponse, Option<String>), JiraTransportError> {
        let (response, body) = self.get_myself().await?;
        let name = (200..300)
            .contains(&response.status)
            .then(|| serde_json::from_str::<serde_json::Value>(&body).ok())
            .flatten()
            .and_then(|json| json["displayName"].as_str().map(str::to_string));
        Ok((response, name))
    }

    async fn get_myself(&self) -> Result<(JiraResponse, String), JiraTransportError> {
        let url = format!(
            "{}/rest/api/{}/myself",
            self.base_url,
            self.edition.api_version()
        );
        let request = self.http.get(url);
        let request = match &self.auth {
            JiraAuth::Basic { email, token } => request.basic_auth(email, Some(token)),
            JiraAuth::Bearer { token } => request.bearer_auth(token),
        };
        let response = request.send().await.map_err(|err| {
            // `reqwest::Error` text can carry the URL; keep only the failure class.
            JiraTransportError(
                if err.is_timeout() {
                    "timeout"
                } else {
                    "connect"
                }
                .to_string(),
            )
        })?;
        let status = response.status().as_u16();
        let retry_after_secs = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let body = response.text().await.unwrap_or_default();
        let body_excerpt = body.chars().take(BODY_EXCERPT_CHARS).collect();
        Ok((
            JiraResponse {
                status,
                retry_after_secs,
                body_excerpt,
            },
            body,
        ))
    }
}

/// Decrypts the stored credential and builds the client. `None` when write-back is
/// not configured: no base URL, auth kind or credential, or no account email on
/// Cloud. It does not look at `writeback_enabled`; the sender checks that.
pub fn client_for(row: &JiraIntegrationRow) -> Result<Option<JiraClient>, Error> {
    let (Some(base_url), Some(kind), Some(sealed)) = (
        row.jira_base_url.as_deref(),
        row.jira_auth_kind.as_deref(),
        row.jira_credential_enc.as_deref(),
    ) else {
        return Ok(None);
    };
    let Some(edition) = JiraEdition::from_auth_kind(kind) else {
        return Ok(None);
    };
    let email = match edition {
        JiraEdition::Cloud => match row.jira_account_email.as_deref() {
            Some(email) => Some(email.to_string()),
            None => return Ok(None),
        },
        JiraEdition::DataCenter => None,
    };
    let token = secret_box::decrypt_with_aad(sealed, row.id.as_bytes()).map_err(|_| {
        Error::InvalidInput("stored Jira credential cannot be decrypted".to_string())
    })?;
    let auth = match email {
        Some(email) => JiraAuth::Basic { email, token },
        None => JiraAuth::Bearer { token },
    };
    JiraClient::new(base_url, edition, auth).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn token() -> String {
        format!("tok-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn edition_from_auth_kind() {
        assert_eq!(
            JiraEdition::from_auth_kind("cloud_basic"),
            Some(JiraEdition::Cloud)
        );
        assert_eq!(JiraEdition::Cloud.api_version(), "3");
        assert_eq!(
            JiraEdition::from_auth_kind("dc_pat"),
            Some(JiraEdition::DataCenter)
        );
        assert_eq!(JiraEdition::DataCenter.api_version(), "2");
        assert_eq!(JiraEdition::from_auth_kind("other"), None);
    }

    #[test]
    fn debug_hides_the_token() {
        let token = token();
        let bearer = JiraAuth::Bearer {
            token: token.clone(),
        };
        let basic = JiraAuth::Basic {
            email: "a@example.com".into(),
            token: token.clone(),
        };
        assert!(!format!("{bearer:?}").contains(&token));
        assert!(!format!("{basic:?}").contains(&token));
        let client =
            JiraClient::new("https://jira.example.com", JiraEdition::DataCenter, bearer).unwrap();
        assert!(!format!("{client:?}").contains(&token));
    }

    #[tokio::test]
    async fn myself_sends_basic_auth_on_cloud() {
        let server = MockServer::start().await;
        let token = token();
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("me@example.com:{token}"))
        );
        Mock::given(method("GET"))
            .and(path("/rest/api/3/myself"))
            .and(header("Authorization", expected.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;
        let client = JiraClient::new(
            &server.uri(),
            JiraEdition::Cloud,
            JiraAuth::Basic {
                email: "me@example.com".into(),
                token,
            },
        )
        .unwrap();
        assert_eq!(client.myself().await.unwrap().status, 200);
    }

    #[tokio::test]
    async fn myself_sends_bearer_on_data_center() {
        let server = MockServer::start().await;
        let token = token();
        Mock::given(method("GET"))
            .and(path("/rest/api/2/myself"))
            .and(header("Authorization", format!("Bearer {token}").as_str()))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = JiraClient::new(
            &format!("{}/", server.uri()),
            JiraEdition::DataCenter,
            JiraAuth::Bearer { token },
        )
        .unwrap();
        assert_eq!(client.myself().await.unwrap().status, 200);
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let server = MockServer::start().await;
        let other = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/elsewhere", other.uri()).as_str()),
            )
            .mount(&server)
            .await;
        let client = JiraClient::new(
            &server.uri(),
            JiraEdition::DataCenter,
            JiraAuth::Bearer { token: token() },
        )
        .unwrap();
        assert_eq!(client.myself().await.unwrap().status, 302);
        assert!(other.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn body_excerpt_is_cut_to_300_chars() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(500)
                    .insert_header("Retry-After", "7")
                    .set_body_string("é".repeat(1000)),
            )
            .mount(&server)
            .await;
        let client = JiraClient::new(
            &server.uri(),
            JiraEdition::DataCenter,
            JiraAuth::Bearer { token: token() },
        )
        .unwrap();
        let response = client.myself().await.unwrap();
        assert_eq!(response.status, 500);
        assert_eq!(response.retry_after_secs, Some(7));
        assert_eq!(response.body_excerpt.chars().count(), 300);
    }
}
