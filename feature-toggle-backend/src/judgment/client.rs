//! HTTP client for TypeSafe System One. Callers depend on `JudgmentClient`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::{info, warn};
use mockall::automock;
use serde_json::Value;
use tokio::sync::Semaphore;

use super::types::{Question, SystemOneRequest, SystemOneResponse};
use crate::config::TypesafeConfig;

/// Retries after the first attempt.
pub const MAX_RETRIES: u32 = 2;
const BASE_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const BODY_SNIPPET_BYTES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JudgmentError {
    #[error("TypeSafe judgments are not configured")]
    Unavailable,
    #[error("TypeSafe request timed out")]
    Timeout,
    /// No request slot (`max_in_flight` permit) came free in time, so nothing
    /// was sent. Distinct from `Timeout`, which means a request was sent.
    #[error("TypeSafe request not sent: all request slots busy")]
    Busy,
    #[error("TypeSafe rate limit or overload")]
    RateLimited,
    #[error("TypeSafe connection error: {0}")]
    Transport(String),
    #[error("TypeSafe returned HTTP {0}: {1}")]
    Http(u16, String),
    #[error("Could not decode TypeSafe response: {0}")]
    Decode(String),
}

impl JudgmentError {
    /// A short label safe for log lines: never includes response bodies.
    pub fn log_label(&self) -> String {
        match self {
            JudgmentError::Unavailable => "unavailable".to_string(),
            JudgmentError::Timeout => "timeout".to_string(),
            JudgmentError::Busy => "busy".to_string(),
            JudgmentError::RateLimited => "rate_limited".to_string(),
            JudgmentError::Transport(_) => "transport".to_string(),
            JudgmentError::Http(status, _) => format!("http_{status}"),
            JudgmentError::Decode(_) => "decode".to_string(),
        }
    }

    /// Whether a request was sent (or tried) to the API. False only when the
    /// call never started: no client configured, or no request slot free.
    pub fn reached_api(&self) -> bool {
        !matches!(self, JudgmentError::Unavailable | JudgmentError::Busy)
    }
}

#[automock]
#[async_trait]
pub trait JudgmentClient: Send + Sync {
    async fn evaluate(
        &self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, JudgmentError>;

    /// The configured model id sent with every request.
    fn model(&self) -> String;
}

pub fn should_retry_status(status: u16) -> bool {
    matches!(status, 429 | 529)
}

/// Delay before retry number `attempt + 1`.
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exponential = BASE_BACKOFF.saturating_mul(1u32 << attempt.min(16));
    retry_after.unwrap_or(exponential).min(MAX_BACKOFF)
}

/// `retry-after` in whole seconds. HTTP-date values are ignored.
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    value?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

pub fn body_snippet(body: &str) -> String {
    if body.len() <= BODY_SNIPPET_BYTES {
        return body.to_string();
    }
    let mut end = BODY_SNIPPET_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    body[..end].to_string()
}

enum AttemptError {
    Retryable(JudgmentError, Option<Duration>),
    Fatal(JudgmentError),
}

fn classify_transport_error(error: reqwest::Error) -> AttemptError {
    if error.is_timeout() {
        AttemptError::Retryable(JudgmentError::Timeout, None)
    } else if error.is_connect() {
        AttemptError::Retryable(
            JudgmentError::Transport(error.without_url().to_string()),
            None,
        )
    } else {
        AttemptError::Fatal(JudgmentError::Transport(error.without_url().to_string()))
    }
}

/// A timeout while reading the body is retried like any other timeout; any
/// other body error means the response cannot be decoded.
fn classify_body_error(error: reqwest::Error) -> AttemptError {
    if error.is_timeout() {
        AttemptError::Retryable(JudgmentError::Timeout, None)
    } else {
        AttemptError::Fatal(JudgmentError::Decode(error.without_url().to_string()))
    }
}

pub struct HttpJudgmentClient {
    http: reqwest::Client,
    api_key: String,
    pub(crate) endpoint: String,
    model: String,
    /// Global cap on concurrent API calls (`typesafe.max_in_flight`).
    permits: Arc<Semaphore>,
    /// Longest wait for a permit: the per-attempt timeout.
    permit_wait: Duration,
}

impl HttpJudgmentClient {
    pub fn new(config: &TypesafeConfig, api_key: String) -> Result<Self, JudgmentError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1)))
            .build()
            .map_err(|e| JudgmentError::Transport(e.without_url().to_string()))?;
        Ok(Self {
            http,
            api_key,
            endpoint: format!("{}/v1/systemone", config.base_url.trim_end_matches('/')),
            model: config.model.clone(),
            permits: Arc::new(Semaphore::new(config.max_in_flight.max(1))),
            permit_wait: Duration::from_millis(config.timeout_ms.max(1)),
        })
    }

    async fn send_once(
        &self,
        request: &SystemOneRequest,
    ) -> Result<SystemOneResponse, AttemptError> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(classify_transport_error)?;

        let status = response.status().as_u16();
        if response.status().is_success() {
            return response
                .json::<SystemOneResponse>()
                .await
                .map_err(classify_body_error);
        }

        let retry_after = parse_retry_after(
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
        );
        let body = response.text().await.unwrap_or_default();
        if should_retry_status(status) {
            Err(AttemptError::Retryable(
                JudgmentError::RateLimited,
                retry_after,
            ))
        } else {
            Err(AttemptError::Fatal(JudgmentError::Http(
                status,
                body_snippet(&body),
            )))
        }
    }
}

#[async_trait]
impl JudgmentClient for HttpJudgmentClient {
    async fn evaluate(
        &self,
        state: Value,
        questions: BTreeMap<String, Question>,
    ) -> Result<SystemOneResponse, JudgmentError> {
        let question_count = questions.len();
        let request = SystemOneRequest {
            state,
            model: self.model.clone(),
            questions,
        };
        // Bounded: a sync endpoint must not hang behind other calls. A full
        // queue fails with `Busy` (nothing sent); async callers retry via the
        // sweep without spending an attempt.
        let _permit = match tokio::time::timeout(self.permit_wait, self.permits.acquire()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => return Err(JudgmentError::Unavailable),
            Err(_) => {
                warn!(
                    "TypeSafe call not sent: no permit within {} ms (max_in_flight reached)",
                    self.permit_wait.as_millis()
                );
                return Err(JudgmentError::Busy);
            }
        };
        let started = Instant::now();
        let mut attempt = 0;
        loop {
            match self.send_once(&request).await {
                Ok(response) => {
                    info!(
                        "TypeSafe call ok: questions={} model={} input_tokens={} latency_ms={}",
                        question_count,
                        response.model,
                        response.usage.input_tokens,
                        started.elapsed().as_millis()
                    );
                    return Ok(response);
                }
                Err(AttemptError::Retryable(error, retry_after)) if attempt < MAX_RETRIES => {
                    let delay = backoff(attempt, retry_after);
                    warn!(
                        "TypeSafe call failed ({}); retry {} in {} ms",
                        error.log_label(),
                        attempt + 1,
                        delay.as_millis()
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(AttemptError::Retryable(error, _)) | Err(AttemptError::Fatal(error)) => {
                    warn!(
                        "TypeSafe call failed: questions={} model={} latency_ms={} error={}",
                        question_count,
                        self.model,
                        started.elapsed().as_millis(),
                        error.log_label()
                    );
                    return Err(error);
                }
            }
        }
    }

    fn model(&self) -> String {
        self.model.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_only_rate_limit_and_overload() {
        assert!(should_retry_status(429));
        assert!(should_retry_status(529));
        for status in [200, 400, 401, 403, 404, 422, 500, 502, 503] {
            assert!(!should_retry_status(status), "status {status}");
        }
    }

    #[test]
    fn backoff_doubles_from_250ms_and_caps_at_5s() {
        assert_eq!(backoff(0, None), Duration::from_millis(250));
        assert_eq!(backoff(1, None), Duration::from_millis(500));
        assert_eq!(backoff(2, None), Duration::from_millis(1000));
        assert_eq!(backoff(10, None), Duration::from_secs(5));
        assert_eq!(backoff(40, None), Duration::from_secs(5));
    }

    #[test]
    fn backoff_honors_retry_after_with_cap() {
        assert_eq!(
            backoff(0, Some(Duration::from_secs(2))),
            Duration::from_secs(2)
        );
        assert_eq!(
            backoff(0, Some(Duration::from_secs(60))),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn parses_retry_after_seconds_only() {
        assert_eq!(parse_retry_after(Some("3")), Some(Duration::from_secs(3)));
        assert_eq!(parse_retry_after(Some(" 1 ")), Some(Duration::from_secs(1)));
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(None), None);
    }

    #[test]
    fn body_snippet_cuts_at_500_bytes_on_a_char_boundary() {
        assert_eq!(body_snippet("short"), "short");
        let long = "a".repeat(499) + "é" + &"b".repeat(100);
        let snippet = body_snippet(&long);
        assert_eq!(snippet.len(), 499);
        assert!(snippet.chars().all(|c| c == 'a'));
    }

    #[test]
    fn log_label_never_contains_body() {
        let error = JudgmentError::Http(422, "state.reason: my secret text".to_string());
        assert_eq!(error.log_label(), "http_422");
        assert!(error.to_string().contains("my secret text"));
        assert_eq!(JudgmentError::Timeout.log_label(), "timeout");
        assert_eq!(JudgmentError::Busy.log_label(), "busy");
        assert_eq!(
            JudgmentError::Transport("x".into()).log_label(),
            "transport"
        );
    }

    /// Serves headers, then never finishes the body. Returns the base URL and
    /// a counter of accepted connections.
    async fn stalling_body_server() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = connections.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{",
                        )
                        .await;
                    tokio::time::sleep(Duration::from_secs(10)).await;
                });
            }
        });
        (format!("http://{address}"), connections)
    }

    #[tokio::test]
    async fn body_read_timeout_is_retried_as_timeout() {
        let (base_url, connections) = stalling_body_server().await;
        let cfg = TypesafeConfig {
            base_url,
            timeout_ms: 200,
            ..TypesafeConfig::default()
        };
        let client = HttpJudgmentClient::new(&cfg, "key".into()).unwrap();

        let error = client
            .evaluate(Value::from("state"), BTreeMap::new())
            .await
            .unwrap_err();

        assert_eq!(error, JudgmentError::Timeout);
        assert_eq!(
            connections.load(std::sync::atomic::Ordering::SeqCst),
            1 + MAX_RETRIES as usize
        );
    }

    /// A caller never waits behind a full set of permits for longer than the
    /// configured timeout: it fails with `Busy` (not `Timeout`) and sends nothing,
    /// so the async pipeline can tell that no request reached the API.
    #[tokio::test]
    async fn waiting_for_a_permit_is_bounded_by_the_timeout() {
        let (base_url, connections) = stalling_body_server().await;
        let cfg = TypesafeConfig {
            base_url,
            timeout_ms: 100,
            max_in_flight: 2,
            ..TypesafeConfig::default()
        };
        let client = HttpJudgmentClient::new(&cfg, "key".into()).unwrap();
        let _held = client.permits.clone().acquire_many_owned(2).await.unwrap();

        let started = Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            client.evaluate(Value::from("state"), BTreeMap::new()),
        )
        .await
        .expect("evaluate must not wait for a permit past its timeout");

        let error = outcome.unwrap_err();
        assert_eq!(error, JudgmentError::Busy);
        assert!(!error.reached_api());
        assert!(started.elapsed() < Duration::from_millis(1000));
        assert_eq!(connections.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn only_busy_and_unavailable_mean_nothing_was_sent() {
        assert!(!JudgmentError::Busy.reached_api());
        assert!(!JudgmentError::Unavailable.reached_api());
        assert!(JudgmentError::Timeout.reached_api());
        assert!(JudgmentError::RateLimited.reached_api());
        assert!(JudgmentError::Transport("x".into()).reached_api());
        assert!(JudgmentError::Http(500, String::new()).reached_api());
        assert!(JudgmentError::Decode("x".into()).reached_api());
    }

    #[test]
    fn new_client_targets_systemone_endpoint() {
        let cfg = TypesafeConfig {
            base_url: "https://example.test".into(),
            ..TypesafeConfig::default()
        };
        let client = HttpJudgmentClient::new(&cfg, "key".into()).unwrap();
        assert_eq!(client.endpoint, "https://example.test/v1/systemone");
        assert_eq!(client.model(), "jev-1.13.0");
    }
}
