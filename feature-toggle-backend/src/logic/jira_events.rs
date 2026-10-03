//! Parsing of inbound Jira events (design §3.9, JI-15). Pure: no I/O.
//!
//! Two body shapes are accepted:
//! - a Jira webhook (`webhookEvent`, `issue`, `user`, `changelog`), on Cloud and
//!   Data Center. It is a status change only when `changelog.items` has a
//!   `status` item; the target status is that item's `toString`.
//! - Automation "Issue data (Jira format)": the issue itself (`key`,
//!   `fields`), or wrapped as `{"issue": {...}, "user": {...}}`. The target
//!   status is `fields.status.name`, and every delivery counts as a status
//!   change (the Automation rule decides when to send).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::logic::external_link::normalize_jira_key;

/// The Jira user who changed the issue. Cloud sends `accountId`; Data Center
/// sends `key` and `name`, used in that order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JiraActor {
    pub account_id: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEvent {
    /// Upper case, validated like a linked issue key.
    pub issue_key: String,
    /// Status the issue moved to; for a webhook without a status change, its
    /// current status if the issue has one.
    pub target_status: Option<String>,
    pub status_changed: bool,
    pub jira_actor: JiraActor,
    pub fields: Map<String, Value>,
    /// Changelog id, else `fields.updated`. Part of the delivery hash.
    pub delivery_marker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The body is neither a webhook nor an issue.
    NoIssue,
    /// The issue key is missing or not a Jira key.
    InvalidIssueKey,
    /// An Automation body without `fields.status.name`.
    NoStatus,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ParseError::NoIssue => "body is not a Jira webhook or issue",
            ParseError::InvalidIssueKey => "issue key is missing or invalid",
            ParseError::NoStatus => "issue has no fields.status.name",
        })
    }
}

pub fn parse_event(json: &Value) -> Result<ParsedEvent, ParseError> {
    let body = json.as_object().ok_or(ParseError::NoIssue)?;

    let (issue, user, changelog) = if body.contains_key("webhookEvent") {
        (
            body.get("issue").and_then(Value::as_object),
            body.get("user"),
            body.get("changelog"),
        )
    } else if let Some(issue) = body.get("issue").and_then(Value::as_object) {
        (Some(issue), body.get("user"), None)
    } else if body.contains_key("key") && body.contains_key("fields") {
        (Some(body), None, None)
    } else {
        (None, None, None)
    };
    let issue = issue.ok_or(ParseError::NoIssue)?;

    let issue_key = issue
        .get("key")
        .and_then(Value::as_str)
        .and_then(|key| normalize_jira_key(key).ok())
        .ok_or(ParseError::InvalidIssueKey)?;
    let fields = issue
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let current_status = fields
        .get("status")
        .and_then(|status| status.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map(str::to_string);

    let (status_changed, target_status) = if body.contains_key("webhookEvent") {
        let status_item = changelog
            .and_then(|changelog| changelog.get("items"))
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("field")
                        .and_then(Value::as_str)
                        .is_some_and(|field| field.eq_ignore_ascii_case("status"))
                })
            });
        match status_item {
            Some(item) => {
                let target = item
                    .get("toString")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|status| !status.is_empty())
                    .map(str::to_string)
                    .or(current_status);
                (target.is_some(), target)
            }
            None => (false, current_status),
        }
    } else {
        (true, Some(current_status.ok_or(ParseError::NoStatus)?))
    };

    let delivery_marker = changelog
        .and_then(|changelog| changelog.get("id"))
        .and_then(scalar_string)
        .or_else(|| fields.get("updated").and_then(scalar_string));

    Ok(ParsedEvent {
        issue_key,
        target_status,
        status_changed,
        jira_actor: user.map(actor).unwrap_or_default(),
        fields,
        delivery_marker,
    })
}

fn actor(user: &Value) -> JiraActor {
    let text = |name: &str| {
        user.get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    JiraActor {
        account_id: text("accountId")
            .or_else(|| text("key"))
            .or_else(|| text("name")),
        display_name: text("displayName"),
    }
}

/// A string or number as text (changelog ids are numbers on Data Center).
fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Values of a Jira field: a string, an option (`{"value"}`) or named object
/// (`{"name"}`), or an array of these (`labels` is an array of strings).
/// Trimmed; empty values are dropped.
pub fn field_values(fields: &Map<String, Value>, field_name: &str) -> Vec<String> {
    fn one(value: &Value) -> Option<String> {
        let text = match value {
            Value::String(text) => text.as_str(),
            Value::Object(object) => object
                .get("value")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)?,
            _ => return None,
        };
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    match fields.get(field_name) {
        Some(Value::Array(items)) => items.iter().filter_map(one).collect(),
        Some(value) => one(value).into_iter().collect(),
        None => Vec::new(),
    }
}

/// SHA-256 (hex) identifying one delivery: issue key, status, the environment
/// values (sorted) and the delivery marker.
pub fn delivery_hash(
    issue_key: &str,
    status: &str,
    environment_values: &[String],
    delivery_marker: Option<&str>,
) -> String {
    let mut environments = environment_values.to_vec();
    environments.sort();
    // JSON keeps the parts unambiguous whatever characters they hold.
    let canonical = serde_json::json!([issue_key, status, environments, delivery_marker]);
    format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "cloud_status" => {
                include_str!("../../tests/fixtures/jira/cloud_webhook_status_change.json")
            }
            "cloud_other" => {
                include_str!("../../tests/fixtures/jira/cloud_webhook_other_change.json")
            }
            "dc_status" => {
                include_str!("../../tests/fixtures/jira/datacenter_webhook_status_change.json")
            }
            "automation" => include_str!("../../tests/fixtures/jira/automation_issue.json"),
            "automation_wrapped" => {
                include_str!("../../tests/fixtures/jira/automation_wrapped.json")
            }
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(text).expect("fixture is JSON")
    }

    #[test]
    fn cloud_webhook_status_change() {
        let event = parse_event(&fixture("cloud_status")).unwrap();
        assert_eq!(event.issue_key, "PROJ-123");
        assert!(event.status_changed);
        assert_eq!(event.target_status.as_deref(), Some("Ready for Release"));
        assert_eq!(
            event.jira_actor,
            JiraActor {
                account_id: Some("5b10ac8d82e05b22cc7d4ef5".to_string()),
                display_name: Some("Jane Doe".to_string()),
            }
        );
        assert_eq!(event.delivery_marker.as_deref(), Some("10500"));
        assert!(event.fields.contains_key("customfield_10042"));
    }

    #[test]
    fn cloud_webhook_without_a_status_item_is_not_a_status_change() {
        let event = parse_event(&fixture("cloud_other")).unwrap();
        assert_eq!(event.issue_key, "PROJ-123");
        assert!(!event.status_changed);
        assert_eq!(event.target_status.as_deref(), Some("In Review"));
    }

    #[test]
    fn data_center_webhook_uses_the_status_item_and_user_key() {
        let event = parse_event(&fixture("dc_status")).unwrap();
        assert_eq!(event.issue_key, "OPS-7");
        assert!(event.status_changed);
        assert_eq!(event.target_status.as_deref(), Some("Done"));
        assert_eq!(
            event.jira_actor.account_id.as_deref(),
            Some("JIRAUSER10100")
        );
        assert_eq!(event.jira_actor.display_name.as_deref(), Some("John Doe"));
        // A numeric changelog id.
        assert_eq!(event.delivery_marker.as_deref(), Some("30012"));
    }

    #[test]
    fn automation_issue_is_a_status_change_to_its_status() {
        let event = parse_event(&fixture("automation")).unwrap();
        assert_eq!(event.issue_key, "PROJ-123");
        assert!(event.status_changed);
        assert_eq!(event.target_status.as_deref(), Some("Done"));
        assert_eq!(event.jira_actor, JiraActor::default());
        assert_eq!(
            event.delivery_marker.as_deref(),
            Some("2026-10-03T13:15:00.000+0000")
        );
    }

    #[test]
    fn wrapped_automation_issue_takes_the_user() {
        let event = parse_event(&fixture("automation_wrapped")).unwrap();
        assert_eq!(event.issue_key, "PROJ-123");
        assert!(event.status_changed);
        assert_eq!(event.target_status.as_deref(), Some("Ready for Release"));
        assert_eq!(event.jira_actor.display_name.as_deref(), Some("Jane Doe"));
    }

    #[test]
    fn bodies_without_an_issue_or_key_are_rejected() {
        assert_eq!(parse_event(&json!({})), Err(ParseError::NoIssue));
        assert_eq!(parse_event(&json!([1, 2])), Err(ParseError::NoIssue));
        assert_eq!(
            parse_event(&json!({"webhookEvent": "jira:issue_updated"})),
            Err(ParseError::NoIssue)
        );
        assert_eq!(
            parse_event(&json!({"key": "not a key", "fields": {"status": {"name": "Done"}}})),
            Err(ParseError::InvalidIssueKey)
        );
        assert_eq!(
            parse_event(&json!({"issue": {"fields": {}}})),
            Err(ParseError::InvalidIssueKey)
        );
        assert_eq!(
            parse_event(&json!({"key": "PROJ-1", "fields": {}})),
            Err(ParseError::NoStatus)
        );
    }

    #[test]
    fn field_values_reads_strings_options_names_and_arrays() {
        let fields = json!({
            "single": "  QA ",
            "option": {"value": "Production", "id": "1"},
            "named": {"name": "Staging"},
            "multi": [{"value": "QA"}, {"value": "Production"}, {"name": "Dev"}, "Lab", " "],
            "labels": ["qa", "release"],
            "empty": null,
            "number": 7,
        });
        let fields = fields.as_object().unwrap();
        assert_eq!(field_values(fields, "single"), vec!["QA"]);
        assert_eq!(field_values(fields, "option"), vec!["Production"]);
        assert_eq!(field_values(fields, "named"), vec!["Staging"]);
        assert_eq!(
            field_values(fields, "multi"),
            vec!["QA", "Production", "Dev", "Lab"]
        );
        assert_eq!(field_values(fields, "labels"), vec!["qa", "release"]);
        assert!(field_values(fields, "empty").is_empty());
        assert!(field_values(fields, "number").is_empty());
        assert!(field_values(fields, "missing").is_empty());
    }

    #[test]
    fn delivery_hash_ignores_environment_order_only() {
        let qa_prod = ["QA".to_string(), "Prod".to_string()];
        let prod_qa = ["Prod".to_string(), "QA".to_string()];
        let base = delivery_hash("PROJ-1", "Done", &qa_prod, Some("1"));
        assert_eq!(base.len(), 64);
        assert_eq!(base, delivery_hash("PROJ-1", "Done", &prod_qa, Some("1")));
        assert_ne!(base, delivery_hash("PROJ-1", "Done", &qa_prod, Some("2")));
        assert_ne!(base, delivery_hash("PROJ-1", "Ready", &qa_prod, Some("1")));
        assert_ne!(base, delivery_hash("PROJ-2", "Done", &qa_prod, Some("1")));
        assert_ne!(
            base,
            delivery_hash("PROJ-1", "Done", &qa_prod[..1], Some("1"))
        );
        assert_ne!(base, delivery_hash("PROJ-1", "Done", &qa_prod, None));
    }
}
