//! HTTP client for the FluxGate REST API.

use std::time::Duration;

use reqwest::{Method, Url};
use serde_json::{Value, json};

use crate::error::CliError;

/// Page size used when fetching every page (the server maximum).
pub const PAGE_SIZE: i64 = 200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest part of a non-JSON error body kept in the message.
const MAX_ERROR_TEXT: usize = 200;

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base", &self.base.as_str())
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl ApiClient {
    pub fn new(base_url: &str, token: Option<String>, timeout: Duration) -> Result<Self, CliError> {
        let base = Url::parse(base_url.trim().trim_end_matches('/'))
            .map_err(|err| CliError::Usage(format!("invalid url '{base_url}': {err}")))?;
        if base.cannot_be_a_base() {
            return Err(CliError::Usage(format!("invalid url '{base_url}'")));
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|err| CliError::Other(err.to_string()))?;
        Ok(Self {
            http,
            base,
            token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        })
    }

    pub fn with_token(&self, token: Option<String>) -> Self {
        Self { token, ..self.clone() }
    }

    /// Base URL plus `segments`, each percent-encoded as one path segment.
    pub fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("checked in ApiClient::new")
            .pop_if_empty()
            .extend(segments);
        url
    }

    pub async fn get(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Value, CliError> {
        self.send(Method::GET, segments, query, None).await
    }

    pub async fn post(&self, segments: &[&str], body: &Value) -> Result<Value, CliError> {
        self.send(Method::POST, segments, &[], Some(body)).await
    }

    /// Follows `meta.total` of a paginated list and returns all items.
    pub async fn get_all_pages(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<Vec<Value>, CliError> {
        let mut items = Vec::new();
        let mut offset: i64 = 0;
        loop {
            let mut page_query = query.to_vec();
            page_query.push(("limit", PAGE_SIZE.to_string()));
            page_query.push(("offset", offset.to_string()));
            let page = self.get(segments, &page_query).await?;
            let batch = page
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| CliError::Other("unexpected list response: no items".into()))?;
            let total = page.pointer("/meta/total").and_then(Value::as_i64).unwrap_or(0);
            let count = batch.len() as i64;
            items.extend(batch);
            offset += count;
            if count == 0 || offset >= total {
                return Ok(items);
            }
        }
    }

    async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, CliError> {
        let mut request = self.http.request(method, self.url(segments));
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(network_error)?;
        decode(response).await
    }
}

fn network_error(err: reqwest::Error) -> CliError {
    if err.is_timeout() {
        CliError::Network(format!("request timed out: {err}"))
    } else if err.is_connect() {
        CliError::Network(format!("cannot connect: {err}"))
    } else {
        CliError::Network(err.to_string())
    }
}

async fn decode(response: reqwest::Response) -> Result<Value, CliError> {
    let status = response.status();
    let text = response.text().await.map_err(network_error)?;
    let body: Value = if text.trim().is_empty() {
        json!({})
    } else {
        // Proxies answer with HTML; keep the start of it as the message.
        serde_json::from_str(&text).unwrap_or_else(|_| {
            json!({ "message": text.chars().take(MAX_ERROR_TEXT).collect::<String>() })
        })
    };
    if status.is_success() {
        return Ok(body);
    }
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("request failed"))
        .to_string();
    Err(CliError::Api { status: status.as_u16(), code, message, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{EXIT_NOT_FOUND, EXIT_SERVER, EXIT_USAGE};
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer, token: Option<&str>) -> ApiClient {
        ApiClient::new(&format!("{}/api/v1", server.uri()), token.map(str::to_string), Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn url_segments_are_encoded_and_trailing_slash_ignored() {
        let api = ApiClient::new("http://h/api/v1/", None, Duration::from_secs(1)).unwrap();
        assert_eq!(
            api.url(&["teams", "a b/c", "features"]).as_str(),
            "http://h/api/v1/teams/a%20b%2Fc/features"
        );
    }

    #[test]
    fn invalid_url_is_a_usage_error() {
        let err = ApiClient::new("not a url", None, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
    }

    #[test]
    fn debug_hides_the_token() {
        let api = ApiClient::new("http://h/api/v1", Some("secret-token".into()), Duration::from_secs(1)).unwrap();
        assert!(!format!("{api:?}").contains("secret-token"));
    }

    #[tokio::test]
    async fn sends_bearer_token_and_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/teams/t1/approval-requests"))
            .and(header("authorization", "Bearer tok"))
            .and(query_param("statuses", "pending"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
            .expect(1)
            .mount(&server)
            .await;
        let value = client(&server, Some("tok"))
            .get(&["teams", "t1", "approval-requests"], &[("statuses", "pending".into())])
            .await
            .unwrap();
        assert_eq!(value, json!({ "items": [] }));
    }

    #[tokio::test]
    async fn error_body_becomes_an_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": "not_found", "message": "feature not found", "code": "feature_not_found", "details": null
            })))
            .mount(&server)
            .await;
        let err = client(&server, None).get(&["features", "x"], &[]).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_NOT_FOUND);
        assert_eq!(err.to_string(), "feature not found (code feature_not_found, HTTP 404)");
    }

    #[tokio::test]
    async fn non_json_proxy_error_still_maps_the_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
            .mount(&server)
            .await;
        let err = client(&server, None).get(&["health"], &[]).await.unwrap_err();
        assert_eq!(err.exit_code(), EXIT_SERVER);
        assert!(err.to_string().contains("Bad Gateway"));
        assert!(err.to_string().contains("HTTP 502"));
    }

    #[tokio::test]
    async fn empty_success_body_is_an_empty_object() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let value = client(&server, None).post(&["auth", "logout"], &json!({})).await.unwrap();
        assert_eq!(value, json!({}));
    }

    #[tokio::test]
    async fn timeout_is_a_network_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let api = ApiClient::new(&server.uri(), None, Duration::from_millis(200)).unwrap();
        let err = api.get(&["health"], &[]).await.unwrap_err();
        assert!(matches!(err, CliError::Network(_)));
        assert_eq!(err.exit_code(), EXIT_SERVER);
    }

    #[tokio::test]
    async fn get_all_pages_follows_meta_total() {
        let server = MockServer::start().await;
        let page = |ids: &[&str], offset: i64| {
            json!({ "items": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
                    "meta": { "offset": offset, "limit": 200, "total": 3 } })
        };
        Mock::given(method("GET"))
            .and(query_param("offset", "0"))
            .and(query_param("limit", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&["a", "b"], 0)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("offset", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(&["c"], 2)))
            .expect(1)
            .mount(&server)
            .await;
        let items = client(&server, None).get_all_pages(&["teams", "t", "features"], &[]).await.unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[2]["id"], "c");
    }
}
