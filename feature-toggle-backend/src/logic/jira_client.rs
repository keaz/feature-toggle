//! Outbound HTTP client for Jira (write-back). It sends the stored credential and
//! nothing else: the credential never appears in `Debug` output, errors or logs.

use std::fmt;
use std::time::Duration;

use base64::Engine;

use crate::Error;
use crate::config::JiraConfig;
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

    pub fn edition(&self) -> JiraEdition {
        self.edition
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
        self.execute(self.http.get(url)).await
    }

    /// `POST rest/api/{v}/issue/{issue}/comment`.
    pub async fn add_comment(
        &self,
        issue: &str,
        body: &serde_json::Value,
    ) -> Result<JiraResponse, JiraTransportError> {
        let url = self.issue_url(issue, "comment");
        self.execute(self.http.post(url).json(body))
            .await
            .map(|(response, _)| response)
    }

    /// `POST rest/api/{v}/issue/{issue}/remotelink`. The same `globalId` updates the
    /// existing link.
    pub async fn put_remote_link(
        &self,
        issue: &str,
        body: &serde_json::Value,
    ) -> Result<JiraResponse, JiraTransportError> {
        let url = self.issue_url(issue, "remotelink");
        self.execute(self.http.post(url).json(body))
            .await
            .map(|(response, _)| response)
    }

    /// `DELETE rest/api/{v}/issue/{issue}/remotelink?globalId=...`.
    pub async fn delete_remote_link(
        &self,
        issue: &str,
        global_id: &str,
    ) -> Result<JiraResponse, JiraTransportError> {
        let url = self.issue_url(issue, "remotelink");
        self.execute(self.http.delete(url).query(&[("globalId", global_id)]))
            .await
            .map(|(response, _)| response)
    }

    fn issue_url(&self, issue: &str, resource: &str) -> String {
        format!(
            "{}/rest/api/{}/issue/{}/{resource}",
            self.base_url,
            self.edition.api_version(),
            encode_path_segment(issue)
        )
    }

    /// The credential and every form of it that can appear in a reply: the token, and
    /// the Basic header value (`base64(email:token)`) on Cloud.
    pub fn secrets(&self) -> Vec<String> {
        match &self.auth {
            JiraAuth::Basic { email, token } => vec![
                token.clone(),
                base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}")),
            ],
            JiraAuth::Bearer { token } => vec![token.clone()],
        }
    }

    /// Sends `request` with the credential. The body excerpt is scrubbed of the
    /// credential before it is cut, so a secret split by the cut cannot survive.
    async fn execute(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<(JiraResponse, String), JiraTransportError> {
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
        let body_excerpt = scrub_secrets(&body, &self.secrets())
            .chars()
            .take(BODY_EXCERPT_CHARS)
            .collect();
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

/// Replaces every non-empty string of `secrets` in `text` with `***`.
pub fn scrub_secrets(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_string(), |text, secret| text.replace(secret, "***"))
}

/// Percent-encodes everything but unreserved characters, for one path segment.
fn encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decrypts the stored credential and builds the client. `None` when write-back is
/// not configured: no base URL, auth kind or credential, or no account email on
/// Cloud. It does not look at `writeback_enabled`; the sender checks that.
/// A plain `http` base URL is refused unless `allow_insecure_http`: the credential must
/// not travel in cleartext, whatever route changed the URL.
pub fn client_for(
    row: &JiraIntegrationRow,
    config: &JiraConfig,
) -> Result<Option<JiraClient>, Error> {
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
    let insecure = reqwest::Url::parse(base_url)
        .map(|url| url.scheme() != "https")
        .unwrap_or(true);
    if insecure && !(config.allow_insecure_http && base_url.starts_with("http://")) {
        return Err(Error::InvalidInput(
            "Jira base URL must use https".to_string(),
        ));
    }
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
    use crate::config::JiraConfig;
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

    fn cloud_client(server: &MockServer, token: &str) -> JiraClient {
        JiraClient::new(
            &server.uri(),
            JiraEdition::Cloud,
            JiraAuth::Basic {
                email: "me@example.com".into(),
                token: token.to_string(),
            },
        )
        .unwrap()
    }

    fn basic_header(token: &str) -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("me@example.com:{token}"))
        )
    }

    #[tokio::test]
    async fn add_comment_posts_the_body_with_auth() {
        let server = MockServer::start().await;
        let token = token();
        let body = serde_json::json!({"body": "hello"});
        Mock::given(method("POST"))
            .and(path("/rest/api/3/issue/PROJ-1/comment"))
            .and(header("Authorization", basic_header(&token).as_str()))
            .and(wiremock::matchers::body_json(&body))
            .respond_with(ResponseTemplate::new(201).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;
        let response = cloud_client(&server, &token)
            .add_comment("PROJ-1", &body)
            .await
            .unwrap();
        assert_eq!(response.status, 201);
    }

    #[tokio::test]
    async fn put_remote_link_posts_to_remotelink_on_data_center() {
        let server = MockServer::start().await;
        let token = token();
        let body = serde_json::json!({"globalId": "fluxgate:feature:x"});
        Mock::given(method("POST"))
            .and(path("/rest/api/2/issue/PROJ-2/remotelink"))
            .and(header("Authorization", format!("Bearer {token}").as_str()))
            .and(wiremock::matchers::body_json(&body))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = JiraClient::new(
            &server.uri(),
            JiraEdition::DataCenter,
            JiraAuth::Bearer { token },
        )
        .unwrap();
        assert_eq!(
            client
                .put_remote_link("PROJ-2", &body)
                .await
                .unwrap()
                .status,
            200
        );
    }

    #[tokio::test]
    async fn delete_remote_link_sends_the_url_encoded_global_id() {
        let server = MockServer::start().await;
        let token = token();
        Mock::given(method("DELETE"))
            .and(path("/rest/api/3/issue/PROJ-3/remotelink"))
            .and(wiremock::matchers::query_param(
                "globalId",
                "fluxgate:feature:a b&c",
            ))
            .and(header("Authorization", basic_header(&token).as_str()))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let response = cloud_client(&server, &token)
            .delete_remote_link("PROJ-3", "fluxgate:feature:a b&c")
            .await
            .unwrap();
        assert_eq!(response.status, 204);
        let requests = server.received_requests().await.unwrap();
        let query = requests[0].url.query().unwrap();
        assert!(!query.contains(' ') && !query.contains("&c"), "{query}");
    }

    #[tokio::test]
    async fn issue_key_is_one_encoded_path_segment() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        cloud_client(&server, &token())
            .add_comment("A/../B?x", &serde_json::json!({}))
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0].url.path(),
            "/rest/api/3/issue/A%2F..%2FB%3Fx/comment"
        );
    }

    #[tokio::test]
    async fn echoed_credential_is_scrubbed_before_the_excerpt_is_cut() {
        let server = MockServer::start().await;
        let token = token();
        // The token sits across the 300 character cut.
        let echo = format!("{}{}", "x".repeat(290), token);
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(echo))
            .mount(&server)
            .await;
        let response = cloud_client(&server, &token)
            .add_comment("PROJ-1", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            !response.body_excerpt.contains(&token[..8]),
            "{}",
            response.body_excerpt
        );
        assert!(response.body_excerpt.ends_with("***"));
    }

    fn row(base_url: &str) -> JiraIntegrationRow {
        JiraIntegrationRow {
            id: uuid::Uuid::new_v4(),
            team_id: uuid::Uuid::new_v4(),
            name: "Jira".to_string(),
            jira_base_url: Some(base_url.to_string()),
            secret_hash: "hash".to_string(),
            environment_field: "labels".to_string(),
            environment_aliases: sqlx::types::Json(Default::default()),
            jira_approved_environment_ids: Vec::new(),
            feature_key_field: None,
            actor_user_id: uuid::Uuid::new_v4(),
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            writeback_enabled: true,
            writeback_comments: true,
            writeback_remote_link: true,
            jira_auth_kind: Some("dc_pat".to_string()),
            jira_account_email: None,
            jira_credential_enc: Some("sealed".to_string()),
            writeback_paused_reason: None,
            native_webhook_secret_enc: None,
        }
    }

    #[test]
    fn client_for_refuses_http_without_allow_insecure_http() {
        let strict = JiraConfig::default();
        let err = client_for(&row("http://jira.example.com"), &strict).unwrap_err();
        assert!(
            err.to_string().contains("Jira base URL must use https"),
            "{err}"
        );
        // Not configured stays None, also for http.
        let mut unconfigured = row("http://jira.example.com");
        unconfigured.jira_credential_enc = None;
        assert!(client_for(&unconfigured, &strict).unwrap().is_none());
    }
}
