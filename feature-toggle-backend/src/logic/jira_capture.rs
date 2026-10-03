//! Pure planning of Jira write-back jobs (JI-43): the comment text of an inbound
//! event or of an activity row, and the outbox jobs one activity row gives.

use std::collections::HashSet;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::database::activity_log::ActivityLogRow;
use crate::database::jira_outbound_job::{NewOutboundJob, OutboundKind};
use crate::logic::jira_rules::EventResults;

const STAGE_APPROVED: &str = "stage_approved";
const STAGE_REJECTED: &str = "stage_rejected";
const STAGE_DEPLOYED: &str = "stage_deployed";
const STAGE_ROLLBACKED: &str = "stage_rollbacked";
const APPROVAL_CANCELLED: &str = "approval_request_cancelled";
const KILL_SWITCH_ACTIVATED: &str = "kill_switch_activated";
const KILL_SWITCH_DEACTIVATED: &str = "kill_switch_deactivated";
const STAGE_CHANGE_REQUESTED: &str = "stage_change_requested";
const APPROVED_EXTERNALLY: &str = "approval_request_approved_externally";
const FEATURE_UPDATED: &str = "feature_updated";
const LINK_ADDED: &str = "external_link_added";
const LINK_REMOVED: &str = "external_link_removed";

/// Activity types Source 2 reads (`feature_updated` is filtered by [`is_capturable`]).
pub const CAPTURE_TYPES: &[&str] = &[
    STAGE_APPROVED,
    STAGE_REJECTED,
    STAGE_DEPLOYED,
    STAGE_ROLLBACKED,
    APPROVAL_CANCELLED,
    KILL_SWITCH_ACTIVATED,
    KILL_SWITCH_DEACTIVATED,
    STAGE_CHANGE_REQUESTED,
    APPROVED_EXTERNALLY,
    FEATURE_UPDATED,
    LINK_ADDED,
    LINK_REMOVED,
];

/// Comment of one inbound event (Source 1). `None` when there is nothing to report.
pub fn event_comment_lines(jira_status: &str, outcome: &EventResults) -> Option<Vec<String>> {
    if outcome.results.is_empty()
        && outcome.unknown_environments.is_empty()
        && outcome.unknown_features.is_empty()
    {
        return None;
    }
    let mut lines = vec![format!("FluxGate: Jira status '{jira_status}'")];
    for result in &outcome.results {
        let mut line = format!(
            "{} · {}: {} {}",
            result.feature_key, result.environment, result.action, result.outcome
        );
        if let Some(reason) = result.reason.as_deref().filter(|r| !r.is_empty()) {
            line.push_str(": ");
            line.push_str(reason);
        }
        lines.push(line);
    }
    if !outcome.unknown_environments.is_empty() {
        lines.push(format!(
            "Unknown environment values: {}",
            outcome.unknown_environments.join(", ")
        ));
    }
    if !outcome.unknown_features.is_empty() {
        lines.push(format!(
            "Unknown features: {}",
            outcome.unknown_features.join(", ")
        ));
    }
    Some(lines)
}

/// A write-back enabled integration of the feature's team.
#[derive(Debug, Clone)]
pub struct WritebackTarget {
    pub integration_id: Uuid,
    pub comments: bool,
    pub remote_link: bool,
}

/// What [`plan_jobs`] needs besides the activity row.
#[derive(Debug, Clone)]
pub struct CaptureContext {
    pub feature_key: String,
    /// Jira issues to write to. For link rows only the row's own key.
    pub issue_keys: Vec<String>,
    /// Write-back enabled integrations of the feature's team.
    pub integrations: Vec<WritebackTarget>,
    /// `actor_user_id` of every Jira integration (all teams): marks Jira-made rows.
    pub jira_actor_ids: HashSet<Uuid>,
    /// Resolved name of `row.actor_id`.
    pub actor_name: Option<String>,
}

fn meta_str<'a>(row: &'a ActivityLogRow, key: &str) -> Option<&'a str> {
    row.metadata
        .as_ref()
        .and_then(|m| m.get(key))
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
}

fn meta_value<'a>(row: &'a ActivityLogRow, key: &str) -> Option<&'a Value> {
    row.metadata.as_ref().and_then(|m| m.get(key))
}

/// `metadata.feature_id` as a UUID.
pub fn row_feature_id(row: &ActivityLogRow) -> Option<Uuid> {
    meta_str(row, "feature_id").and_then(|id| Uuid::parse_str(id).ok())
}

/// False for a `feature_updated` row that is not a version rollback.
pub fn is_capturable(row: &ActivityLogRow) -> bool {
    row.activity_type != FEATURE_UPDATED || meta_str(row, "target_version_id").is_some()
}

/// Issue keys of a link row: its own key, only for a `jira` link.
pub fn link_row_issue_keys(row: &ActivityLogRow) -> Vec<String> {
    if meta_str(row, "system") != Some("jira") {
        return Vec::new();
    }
    meta_str(row, "external_key")
        .map(|key| vec![key.to_string()])
        .unwrap_or_default()
}

pub fn is_link_row(row: &ActivityLogRow) -> bool {
    matches!(row.activity_type.as_str(), LINK_ADDED | LINK_REMOVED)
}

fn is_comment_kind(activity_type: &str) -> bool {
    matches!(
        activity_type,
        STAGE_APPROVED
            | STAGE_REJECTED
            | STAGE_DEPLOYED
            | STAGE_ROLLBACKED
            | APPROVAL_CANCELLED
            | KILL_SWITCH_ACTIVATED
            | KILL_SWITCH_DEACTIVATED
    )
}

fn is_link_only_kind(activity_type: &str) -> bool {
    matches!(
        activity_type,
        STAGE_CHANGE_REQUESTED | APPROVED_EXTERNALLY | FEATURE_UPDATED | LINK_ADDED
    )
}

/// Comment text of an activity row. Only environment, actor, reason and external
/// reference are used; no other metadata.
pub fn activity_comment_lines(row: &ActivityLogRow, ctx: &CaptureContext) -> Option<Vec<String>> {
    let kind = row.activity_type.as_str();
    if !is_comment_kind(kind) {
        return None;
    }
    let env = meta_str(row, "environment_name").unwrap_or("an environment");
    let actor = row
        .actor_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .or(ctx.actor_name.as_deref().filter(|n| !n.is_empty()))
        .unwrap_or("FluxGate");
    let action = match kind {
        STAGE_APPROVED => format!("Approved for {env} by {actor}"),
        STAGE_REJECTED => format!("Rejected for {env} by {actor}"),
        STAGE_DEPLOYED => format!("Deployed to {env} by {actor}"),
        STAGE_ROLLBACKED => format!("Rolled back in {env} by {actor}"),
        APPROVAL_CANCELLED => format!("Approval request for {env} cancelled by {actor}"),
        KILL_SWITCH_DEACTIVATED => format!("Kill switch deactivated by {actor}"),
        _ => {
            let minutes = meta_value(row, "rollback_in_minutes")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if minutes > 0 {
                format!("Kill switch scheduled by {actor} in {minutes} min")
            } else if meta_value(row, "scheduled_execution").and_then(Value::as_bool) == Some(true)
            {
                "Kill switch activated (scheduled)".to_string()
            } else {
                format!("Kill switch activated by {actor}")
            }
        }
    };
    let mut lines = vec![format!("FluxGate: {}", ctx.feature_key), action];
    if let Some(reason) = meta_str(row, "reason") {
        lines.push(format!("Reason: {reason}"));
    }
    if let Some(reference) = meta_str(row, "external_ref") {
        lines.push(format!("Ref: {reference}"));
    }
    Some(lines)
}

/// The outbox jobs of one activity row: for each issue and integration, the comment
/// first, then the remote link (they keep insert order in one transaction).
pub fn plan_jobs(row: &ActivityLogRow, ctx: &CaptureContext) -> Vec<NewOutboundJob> {
    let kind = row.activity_type.as_str();
    let Some(feature_id) = row_feature_id(row) else {
        return Vec::new();
    };
    let jira_made = row
        .actor_id
        .is_some_and(|actor| ctx.jira_actor_ids.contains(&actor));
    let comment_lines = if is_comment_kind(kind) && !jira_made {
        activity_comment_lines(row, ctx)
    } else {
        None
    };
    let mut jobs = Vec::new();
    for issue in &ctx.issue_keys {
        for target in &ctx.integrations {
            let integration = target.integration_id;
            if kind == LINK_REMOVED {
                if target.remote_link {
                    jobs.push(NewOutboundJob {
                        integration_id: integration,
                        issue_key: issue.clone(),
                        // None: the feature may be deleted by now (foreign key); the
                        // sender reads `payload.featureId`.
                        feature_id: None,
                        kind: OutboundKind::RemoteLinkDelete,
                        payload: json!({"featureId": feature_id.to_string()}),
                        dedupe_key: format!("unlink:{}:{integration}", row.id),
                    });
                }
                continue;
            }
            if let (true, Some(lines)) = (target.comments, comment_lines.as_ref()) {
                jobs.push(NewOutboundJob {
                    integration_id: integration,
                    issue_key: issue.clone(),
                    feature_id: Some(feature_id),
                    kind: OutboundKind::Comment,
                    payload: json!({"lines": lines}),
                    dedupe_key: format!("activity:{}:{integration}:{issue}", row.id),
                });
            }
            if target.remote_link && (is_comment_kind(kind) || is_link_only_kind(kind)) {
                jobs.push(NewOutboundJob {
                    integration_id: integration,
                    issue_key: issue.clone(),
                    feature_id: Some(feature_id),
                    kind: OutboundKind::RemoteLink,
                    payload: json!({}),
                    dedupe_key: format!(
                        "link:{integration}:{issue}:{feature_id}:activity:{}",
                        row.id
                    ),
                });
            }
        }
    }
    jobs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::jira_rules::RuleResult;
    use serde_json::json;

    fn result(
        feature: &str,
        env: &str,
        action: &str,
        outcome: &str,
        reason: Option<&str>,
    ) -> RuleResult {
        RuleResult {
            feature_id: Uuid::new_v4().to_string(),
            feature_key: feature.to_string(),
            environment_id: Uuid::new_v4().to_string(),
            environment: env.to_string(),
            rule_id: Uuid::new_v4().to_string(),
            rule_status: "Ready".to_string(),
            action: action.to_string(),
            outcome: outcome.to_string(),
            from: None,
            to: None,
            reason: reason.map(str::to_string),
            approval_request_id: None,
        }
    }

    #[test]
    fn event_comment_lists_each_result_and_unknowns() {
        let outcome = EventResults {
            results: vec![
                result("flag-x", "qa", "approve", "applied", None),
                result("flag-x", "prod", "deploy", "refused", Some("not approved")),
            ],
            unknown_environments: vec!["Staging-EU".to_string()],
            unknown_features: vec!["flag-y".to_string()],
        };
        assert_eq!(
            event_comment_lines("Ready for Release", &outcome).unwrap(),
            vec![
                "FluxGate: Jira status 'Ready for Release'",
                "flag-x · qa: approve applied",
                "flag-x · prod: deploy refused: not approved",
                "Unknown environment values: Staging-EU",
                "Unknown features: flag-y",
            ]
        );
    }

    #[test]
    fn event_comment_is_none_without_results() {
        assert!(event_comment_lines("Done", &EventResults::default()).is_none());
    }

    struct Fx {
        feature_id: Uuid,
        integrations: Vec<Uuid>,
    }

    fn fx(n: usize) -> Fx {
        Fx {
            feature_id: Uuid::new_v4(),
            integrations: (0..n).map(|_| Uuid::new_v4()).collect(),
        }
    }

    fn row(kind: &str, actor: Option<Uuid>, fx: &Fx, extra: serde_json::Value) -> ActivityLogRow {
        let mut metadata = json!({
            "feature_id": fx.feature_id.to_string(),
            "feature_key": "flag-x",
            "environment_name": "prod",
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            metadata[key] = value;
        }
        ActivityLogRow {
            id: Uuid::new_v4(),
            activity_type: kind.to_string(),
            entity_type: "stage".to_string(),
            entity_id: Uuid::new_v4().to_string(),
            actor_id: actor,
            actor_name: Some("Jane Doe".to_string()),
            description: "d".to_string(),
            metadata: Some(metadata),
            created_at: chrono::Utc::now(),
        }
    }

    fn ctx(fx: &Fx, issues: &[&str], comments: bool, remote_link: bool) -> CaptureContext {
        CaptureContext {
            feature_key: "flag-x".to_string(),
            issue_keys: issues.iter().map(|k| k.to_string()).collect(),
            integrations: fx
                .integrations
                .iter()
                .map(|id| WritebackTarget {
                    integration_id: *id,
                    comments,
                    remote_link,
                })
                .collect(),
            jira_actor_ids: HashSet::new(),
            actor_name: None,
        }
    }

    fn count(jobs: &[NewOutboundJob], kind: OutboundKind) -> usize {
        jobs.iter().filter(|job| job.kind == kind).count()
    }

    #[test]
    fn human_approval_gives_comment_and_link() {
        let fx = fx(1);
        let r = row(
            "stage_approved",
            Some(Uuid::new_v4()),
            &fx,
            json!({"reason": "ok", "external_ref": "REL-9"}),
        );
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-1"], true, true));
        assert_eq!(jobs.len(), 2);
        // comment first, then link (same-row insert order)
        assert_eq!(jobs[0].kind, OutboundKind::Comment);
        assert_eq!(jobs[1].kind, OutboundKind::RemoteLink);
        assert_eq!(
            jobs[0].payload["lines"],
            json!([
                "FluxGate: flag-x",
                "Approved for prod by Jane Doe",
                "Reason: ok",
                "Ref: REL-9"
            ])
        );
        assert_eq!(jobs[0].feature_id, Some(fx.feature_id));
        assert_eq!(jobs[0].issue_key, "PROJ-1");
        assert_eq!(
            jobs[0].dedupe_key,
            format!("activity:{}:{}:PROJ-1", r.id, fx.integrations[0])
        );
        assert_eq!(
            jobs[1].dedupe_key,
            format!(
                "link:{}:PROJ-1:{}:activity:{}",
                fx.integrations[0], fx.feature_id, r.id
            )
        );
    }

    #[test]
    fn jira_made_rows_refresh_the_link_without_a_comment() {
        let fx = fx(1);
        let jira_user = Uuid::new_v4();
        let r = row("stage_deployed", Some(jira_user), &fx, json!({}));
        let mut c = ctx(&fx, &["PROJ-1"], true, true);
        c.jira_actor_ids.insert(jira_user);
        let jobs = plan_jobs(&r, &c);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, OutboundKind::RemoteLink);
        assert!(activity_comment_lines(&r, &c).is_some());
    }

    #[test]
    fn capture_fans_out_once_per_issue_and_integration() {
        let fx = fx(2);
        let r = row("stage_deployed", None, &fx, json!({}));
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-1", "PROJ-2"], true, true));
        assert_eq!(count(&jobs, OutboundKind::Comment), 4);
        assert_eq!(count(&jobs, OutboundKind::RemoteLink), 4);
        let keys: HashSet<_> = jobs.iter().map(|j| j.dedupe_key.clone()).collect();
        assert_eq!(keys.len(), 8);
    }

    #[test]
    fn comments_off_gives_links_only_and_link_off_gives_comments_only() {
        let fx = fx(1);
        let r = row("stage_deployed", None, &fx, json!({}));
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-1"], false, true));
        assert_eq!(
            (
                count(&jobs, OutboundKind::Comment),
                count(&jobs, OutboundKind::RemoteLink)
            ),
            (0, 1)
        );
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-1"], true, false));
        assert_eq!(
            (
                count(&jobs, OutboundKind::Comment),
                count(&jobs, OutboundKind::RemoteLink)
            ),
            (1, 0)
        );
    }

    #[test]
    fn link_only_kinds_give_a_link_and_no_comment() {
        let fx = fx(1);
        for kind in [
            "stage_change_requested",
            "approval_request_approved_externally",
            "feature_updated",
        ] {
            let r = row(
                kind,
                None,
                &fx,
                json!({"target_version_id": Uuid::new_v4().to_string()}),
            );
            let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-1"], true, true));
            assert_eq!(jobs.len(), 1, "{kind}");
            assert_eq!(jobs[0].kind, OutboundKind::RemoteLink);
        }
    }

    #[test]
    fn link_added_targets_only_that_issue() {
        let fx = fx(1);
        let r = row(
            "external_link_added",
            None,
            &fx,
            json!({"external_key": "PROJ-7", "system": "jira"}),
        );
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-7"], true, true));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, OutboundKind::RemoteLink);
        assert_eq!(jobs[0].issue_key, "PROJ-7");
    }

    #[test]
    fn link_removed_gives_delete_with_feature_id_payload() {
        let fx = fx(1);
        let r = row(
            "external_link_removed",
            None,
            &fx,
            json!({"external_key": "PROJ-7", "system": "jira"}),
        );
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-7"], true, true));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].kind, OutboundKind::RemoteLinkDelete);
        assert_eq!(
            jobs[0].payload,
            json!({"featureId": fx.feature_id.to_string()})
        );
        assert_eq!(
            jobs[0].dedupe_key,
            format!("unlink:{}:{}", r.id, fx.integrations[0])
        );
        let jobs = plan_jobs(&r, &ctx(&fx, &["PROJ-7"], true, false));
        assert!(jobs.is_empty());
    }

    #[test]
    fn non_jira_link_rows_give_nothing() {
        let fx = fx(1);
        let r = row(
            "external_link_added",
            None,
            &fx,
            json!({"external_key": "GH-1", "system": "github"}),
        );
        // the scheduler leaves issue_keys empty for non-jira rows
        assert!(plan_jobs(&r, &ctx(&fx, &[], true, true)).is_empty());
        assert_eq!(link_row_issue_keys(&r), Vec::<String>::new());
        let r = row(
            "external_link_added",
            None,
            &fx,
            json!({"external_key": "PROJ-1", "system": "jira"}),
        );
        assert_eq!(link_row_issue_keys(&r), vec!["PROJ-1".to_string()]);
    }

    #[test]
    fn comment_never_contains_other_metadata() {
        let fx = fx(1);
        let r = row(
            "stage_approved",
            None,
            &fx,
            json!({"secret": "x", "old_state": {"a": 1}, "status": "approved"}),
        );
        let lines = activity_comment_lines(&r, &ctx(&fx, &["PROJ-1"], true, true)).unwrap();
        let text = lines.join("\n");
        assert!(!text.contains("secret") && !text.contains("old_state") && !text.contains("\"a\""));
        assert_eq!(
            lines,
            vec!["FluxGate: flag-x", "Approved for prod by Jane Doe"]
        );
    }

    #[test]
    fn actor_falls_back_to_resolved_name_then_fluxgate() {
        let fx = fx(1);
        let mut r = row("stage_rejected", Some(Uuid::new_v4()), &fx, json!({}));
        r.actor_name = None;
        let mut c = ctx(&fx, &["PROJ-1"], true, true);
        c.actor_name = Some("Resolved Name".to_string());
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap()[1],
            "Rejected for prod by Resolved Name"
        );
        c.actor_name = None;
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap()[1],
            "Rejected for prod by FluxGate"
        );
        r.metadata
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("environment_name");
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap()[1],
            "Rejected for an environment by FluxGate"
        );
    }

    #[test]
    fn scheduled_kill_switch_says_scheduled() {
        let fx = fx(1);
        let c = ctx(&fx, &["PROJ-1"], true, true);
        let r = row(
            "kill_switch_activated",
            None,
            &fx,
            json!({"scheduled_execution": true}),
        );
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap()[1],
            "Kill switch activated (scheduled)"
        );
        let r = row(
            "kill_switch_activated",
            None,
            &fx,
            json!({"rollback_in_minutes": 15}),
        );
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap()[1],
            "Kill switch scheduled by Jane Doe in 15 min"
        );
        let r = row(
            "kill_switch_activated",
            None,
            &fx,
            json!({"rollback_in_minutes": 0}),
        );
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap()[1],
            "Kill switch activated by Jane Doe"
        );
        let r = row(
            "kill_switch_deactivated",
            None,
            &fx,
            json!({"reason": "fixed"}),
        );
        assert_eq!(
            activity_comment_lines(&r, &c).unwrap(),
            vec![
                "FluxGate: flag-x",
                "Kill switch deactivated by Jane Doe",
                "Reason: fixed"
            ]
        );
    }

    #[test]
    fn feature_updated_needs_a_target_version() {
        let fx = fx(1);
        let with = row(
            "feature_updated",
            None,
            &fx,
            json!({"target_version_id": "v"}),
        );
        let without = row("feature_updated", None, &fx, json!({}));
        assert!(is_capturable(&with));
        assert!(!is_capturable(&without));
    }
}
