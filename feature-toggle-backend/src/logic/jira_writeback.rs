//! Pure rules of the Jira write-back sender (JI-42): retry schedule, classification
//! of Jira replies and the bodies sent to Jira.

use chrono::Duration;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::database::jira_outbound_job::OutboundKind;
use crate::logic::jira_client::{JiraEdition, JiraResponse, JiraTransportError, scrub_secrets};

/// A job is `dead` after this many failed attempts.
pub const MAX_ATTEMPTS: i32 = 6;
const MAX_TITLE_CHARS: usize = 255;

/// Wait after `attempts_done` failed attempts; `None` when out of attempts.
pub fn retry_delay(attempts_done: i32) -> Option<Duration> {
    match attempts_done {
        1 => Some(Duration::seconds(30)),
        2 => Some(Duration::minutes(2)),
        3 => Some(Duration::minutes(10)),
        4 => Some(Duration::hours(1)),
        5 => Some(Duration::hours(6)),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    Retry {
        delay: Duration,
    },
    Dead,
    /// 401 or 403: the job is `dead` and the integration pauses.
    PauseIntegration,
}

/// `attempts_done` counts this attempt.
pub fn classify(
    kind: OutboundKind,
    result: &Result<JiraResponse, JiraTransportError>,
    attempts_done: i32,
) -> Outcome {
    let retry = |retry_after: Option<u64>| match retry_delay(attempts_done) {
        None => Outcome::Dead,
        Some(delay) => Outcome::Retry {
            delay: retry_after
                .and_then(|secs| i64::try_from(secs).ok())
                .and_then(Duration::try_seconds)
                .map_or(delay, |after| delay.max(after)),
        },
    };
    let response = match result {
        Ok(response) => response,
        Err(_) => return retry(None),
    };
    match response.status {
        200..=299 => Outcome::Sent,
        404 if kind == OutboundKind::RemoteLinkDelete => Outcome::Sent,
        401 | 403 => Outcome::PauseIntegration,
        429 => retry(response.retry_after_secs),
        500..=599 => retry(None),
        _ => Outcome::Dead,
    }
}

/// Cloud takes an ADF document (one paragraph per line), Data Center plain text.
pub fn comment_body(edition: JiraEdition, lines: &[String]) -> Value {
    match edition {
        JiraEdition::Cloud => {
            let paragraphs: Vec<Value> = lines
                .iter()
                .map(|line| json!({"type": "paragraph", "content": [{"type": "text", "text": line}]}))
                .collect();
            json!({"body": {"type": "doc", "version": 1, "content": paragraphs}})
        }
        JiraEdition::DataCenter => json!({"body": lines.join("\n")}),
    }
}

pub fn remote_link_global_id(feature_id: Uuid) -> String {
    format!("fluxgate:feature:{feature_id}")
}

/// `FluxGate: <key> · <env> <STATUS> · ...`, stages as (environment name, status) in
/// pipeline order. At most 255 characters, cut with a trailing `…`.
pub fn remote_link_title(feature_key: &str, stages: &[(String, String)]) -> String {
    let mut title = format!("FluxGate: {feature_key}");
    for (environment, status) in stages {
        title.push_str(&format!(" · {environment} {status}"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        title = title.chars().take(MAX_TITLE_CHARS - 1).collect();
        title.push('…');
    }
    title
}

pub fn remote_link_body(feature_id: Uuid, title: &str, url: &str) -> Value {
    json!({
        "globalId": remote_link_global_id(feature_id),
        "application": {"type": "io.fluxgate", "name": "FluxGate"},
        "object": {"url": url, "title": title},
    })
}

/// `<status>: <body excerpt>` for `last_error`. The excerpt is raw Jira text, so every
/// string in `secrets` (the credential and its Basic base64 form) is replaced by `***`.
pub fn error_note(resp: &JiraResponse, secrets: &[String]) -> String {
    format!(
        "{}: {}",
        resp.status,
        scrub_secrets(&resp.body_excerpt, secrets)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::jira_outbound_job::OutboundKind;
    use crate::logic::jira_client::{JiraEdition, JiraResponse, JiraTransportError};
    use chrono::Duration;
    use serde_json::json;
    use uuid::Uuid;

    fn resp(
        status: u16,
        retry_after_secs: Option<u64>,
    ) -> Result<JiraResponse, JiraTransportError> {
        Ok(JiraResponse {
            status,
            retry_after_secs,
            body_excerpt: String::new(),
        })
    }

    #[test]
    fn retry_delays_follow_the_schedule() {
        let d: Vec<_> = (1..=6).map(retry_delay).collect();
        assert_eq!(
            d,
            vec![
                Some(Duration::seconds(30)),
                Some(Duration::minutes(2)),
                Some(Duration::minutes(10)),
                Some(Duration::hours(1)),
                Some(Duration::hours(6)),
                None
            ]
        );
        assert_eq!(retry_delay(0), None);
        assert_eq!(MAX_ATTEMPTS, 6);
    }

    #[test]
    fn classify_table() {
        use OutboundKind::*;
        let retry = |secs| Outcome::Retry {
            delay: Duration::seconds(secs),
        };
        assert_eq!(classify(Comment, &resp(200, None), 1), Outcome::Sent);
        assert_eq!(classify(Comment, &resp(204, None), 1), Outcome::Sent);
        assert_eq!(
            classify(RemoteLinkDelete, &resp(404, None), 1),
            Outcome::Sent
        );
        assert_eq!(classify(Comment, &resp(404, None), 1), Outcome::Dead);
        assert_eq!(classify(RemoteLink, &resp(404, None), 1), Outcome::Dead);
        assert_eq!(
            classify(Comment, &resp(401, None), 1),
            Outcome::PauseIntegration
        );
        assert_eq!(
            classify(RemoteLink, &resp(403, None), 3),
            Outcome::PauseIntegration
        );
        assert_eq!(classify(Comment, &resp(429, None), 1), retry(30));
        assert_eq!(classify(Comment, &resp(429, Some(3600)), 1), retry(3600));
        // Retry-After smaller than the schedule: the schedule wins.
        assert_eq!(classify(Comment, &resp(429, Some(1)), 2), retry(120));
        assert_eq!(classify(Comment, &resp(429, None), 6), Outcome::Dead);
        assert_eq!(classify(Comment, &resp(500, None), 1), retry(30));
        assert_eq!(
            classify(Comment, &resp(503, Some(5000)), 5),
            retry(6 * 3600)
        );
        assert_eq!(classify(Comment, &resp(502, None), 6), Outcome::Dead);
        let transport = Err(JiraTransportError("timeout".to_string()));
        assert_eq!(classify(Comment, &transport, 2), retry(120));
        assert_eq!(classify(Comment, &transport, 6), Outcome::Dead);
        assert_eq!(classify(Comment, &resp(302, None), 1), Outcome::Dead);
        assert_eq!(classify(Comment, &resp(400, None), 1), Outcome::Dead);
        assert_eq!(classify(Comment, &resp(409, None), 1), Outcome::Dead);
    }

    #[test]
    fn cloud_comment_is_adf_with_one_paragraph_per_line() {
        let body = comment_body(JiraEdition::Cloud, &["a".to_string(), "b".to_string()]);
        assert_eq!(
            body,
            json!({"body":{"type":"doc","version":1,"content":[
                {"type":"paragraph","content":[{"type":"text","text":"a"}]},
                {"type":"paragraph","content":[{"type":"text","text":"b"}]}]}})
        );
    }

    #[test]
    fn data_center_comment_is_plain_text() {
        let body = comment_body(JiraEdition::DataCenter, &["a".to_string(), "b".to_string()]);
        assert_eq!(body, json!({"body":"a\nb"}));
    }

    #[test]
    fn remote_link_title_lists_stages_in_order_and_fits_255() {
        let title = remote_link_title(
            "flag-x",
            &[
                ("qa".to_string(), "DEPLOYED".to_string()),
                ("prod".to_string(), "DEPLOYMENT_REQUESTED".to_string()),
            ],
        );
        assert_eq!(
            title,
            "FluxGate: flag-x · qa DEPLOYED · prod DEPLOYMENT_REQUESTED"
        );
        let many: Vec<_> = (0..40)
            .map(|i| (format!("environment-{i}"), "DEPLOYED".to_string()))
            .collect();
        let long = remote_link_title("flag-x", &many);
        assert_eq!(long.chars().count(), 255);
        assert!(long.ends_with('…'));
        assert!(long.starts_with("FluxGate: flag-x · environment-0 DEPLOYED"));
    }

    #[test]
    fn remote_link_body_uses_the_global_id() {
        let id = Uuid::new_v4();
        let body = remote_link_body(id, "T", "https://flux.example.com/features/x");
        assert_eq!(
            body,
            json!({"globalId": format!("fluxgate:feature:{id}"),
                   "application": {"type": "io.fluxgate", "name": "FluxGate"},
                   "object": {"url": "https://flux.example.com/features/x", "title": "T"}})
        );
        assert_eq!(remote_link_global_id(id), format!("fluxgate:feature:{id}"));
    }

    #[test]
    fn error_note_has_status_and_scrubs_the_credential() {
        let token = format!("tok-{}", Uuid::new_v4());
        let basic = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("me@example.com:{token}"),
        );
        let resp = JiraResponse {
            status: 400,
            retry_after_secs: None,
            body_excerpt: format!("bad. Authorization: Basic {basic} / token {token} end"),
        };
        let note = error_note(&resp, &[token.clone(), basic.clone()]);
        assert!(note.starts_with("400: bad."), "{note}");
        assert!(!note.contains(&token), "{note}");
        assert!(!note.contains(&basic), "{note}");
        assert!(note.contains("***"));
        // No secrets: the excerpt is kept as is.
        assert_eq!(
            error_note(
                &JiraResponse {
                    status: 500,
                    retry_after_secs: None,
                    body_excerpt: "boom".to_string()
                },
                &[]
            ),
            "500: boom"
        );
    }
}
