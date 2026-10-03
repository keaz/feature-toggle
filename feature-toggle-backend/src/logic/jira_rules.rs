//! The Jira rule engine (design §3.7, §3.9, JI-15): for one status change,
//! match the integration's rules, resolve the features and environments the
//! issue names, and run each (rule, feature, environment) through JI-14's
//! `ExternalChangeLogic`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::Error;
use crate::database::entity::{Environment, JiraIntegrationRow, JiraStatusRuleRow};
use crate::database::environment::EnvironmentRepository;
use crate::database::external_link::ExternalLinkRepository;
use crate::database::feature::FeatureRepository;
use crate::logic::external_change::{
    ExternalAction, ExternalActor, ExternalChangeContext, ExternalChangeLogic, ExternalOutcome,
};
use crate::logic::external_link::SYSTEM_JIRA;
use crate::logic::jira_events::{ParsedEvent, field_values};
use crate::logic::jira_integration::{
    ACTION_APPROVE, ACTION_DEPLOY, ACTION_REQUEST, ACTION_ROLLBACK,
};

/// An environment an issue names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvTarget {
    pub id: Uuid,
    pub name: String,
}

/// The issue's environment values resolved to environments of the team, and
/// the values that matched none. A value matches an alias key (ignoring case
/// and surrounding spaces) whose environment is active in the team, else the
/// one active environment with that name (ignoring case). A name shared by
/// several active environments matches none. Targets are unique, in value
/// order.
pub fn resolve_environments(
    values: &[String],
    aliases: &BTreeMap<String, Uuid>,
    team_environments: &[Environment],
) -> (Vec<EnvTarget>, Vec<String>) {
    let active: Vec<&Environment> = team_environments.iter().filter(|env| env.active).collect();
    let mut targets: Vec<EnvTarget> = Vec::new();
    let mut unknown = Vec::new();
    for value in values {
        let wanted = value.trim();
        let alias = aliases
            .iter()
            .find(|(key, _)| key.trim().to_lowercase() == wanted.to_lowercase())
            .map(|(_, id)| *id);
        let found = match alias {
            Some(id) => active.iter().find(|env| env.id == id).copied(),
            None => {
                let mut named = active
                    .iter()
                    .filter(|env| env.name.trim().to_lowercase() == wanted.to_lowercase());
                match (named.next(), named.next()) {
                    (Some(env), None) => Some(*env),
                    _ => None,
                }
            }
        };
        match found {
            Some(env) if !targets.iter().any(|target| target.id == env.id) => {
                targets.push(EnvTarget {
                    id: env.id,
                    name: env.name.clone(),
                })
            }
            Some(_) => {}
            None => unknown.push(value.clone()),
        }
    }
    (targets, unknown)
}

/// Enabled rules whose status equals `status` ignoring case and surrounding
/// spaces, in `position` order.
pub fn matching_rules<'a>(
    rules: &'a [JiraStatusRuleRow],
    status: &str,
) -> Vec<&'a JiraStatusRuleRow> {
    let wanted = status.trim().to_lowercase();
    let mut matched: Vec<&JiraStatusRuleRow> = rules
        .iter()
        .filter(|rule| rule.enabled && rule.jira_status.trim().to_lowercase() == wanted)
        .collect();
    matched.sort_by_key(|rule| rule.position);
    matched
}

fn external_action(action: &str) -> Option<ExternalAction> {
    match action {
        ACTION_REQUEST => Some(ExternalAction::Request),
        ACTION_APPROVE => Some(ExternalAction::Approve),
        ACTION_DEPLOY => Some(ExternalAction::Deploy),
        ACTION_ROLLBACK => Some(ExternalAction::Rollback),
        _ => None,
    }
}

/// What one (rule, feature, environment) did. Stored in the event log and
/// returned to Jira.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleResult {
    pub feature_id: String,
    pub feature_key: String,
    pub environment_id: String,
    pub environment: String,
    pub rule_id: String,
    pub rule_status: String,
    /// `request`, `approve`, `deploy` or `rollback`.
    pub action: String,
    /// `applied`, `no_op`, `refused` or `error`.
    pub outcome: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub reason: Option<String>,
    pub approval_request_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventResults {
    pub results: Vec<RuleResult>,
    /// Environment values on the issue that matched no environment.
    pub unknown_environments: Vec<String>,
    /// Feature keys from `feature_key_field` that matched no feature.
    pub unknown_features: Vec<String>,
}

pub const OUTCOME_APPLIED: &str = "applied";
pub const OUTCOME_NO_OP: &str = "no_op";
pub const OUTCOME_REFUSED: &str = "refused";
pub const OUTCOME_ERROR: &str = "error";

/// What the engine needs, borrowed from the handler's app data.
pub struct RuleEngine<'a> {
    pub external_change: &'a dyn ExternalChangeLogic,
    pub links: &'a dyn ExternalLinkRepository,
    pub features: &'a dyn FeatureRepository,
    pub environments: &'a dyn EnvironmentRepository,
}

impl RuleEngine<'_> {
    /// Runs the matching rules for a status change of `event`. One target's
    /// error is recorded as an `error` result and does not stop the others;
    /// `Err` only when the features or environments cannot be read.
    pub async fn run(
        &self,
        integration: &JiraIntegrationRow,
        rules: &[JiraStatusRuleRow],
        event: &ParsedEvent,
        status: &str,
    ) -> Result<EventResults, Error> {
        let team_id = integration.team_id;
        let team_environments = self
            .environments
            .get_environments(team_id, None, Some(true))
            .await?;
        let (targets, unknown_environments) = resolve_environments(
            &field_values(&event.fields, &integration.environment_field),
            &integration.environment_aliases,
            &team_environments,
        );
        let (features, unknown_features) = self.features_of(integration, event).await?;
        let mut results = EventResults {
            results: Vec::new(),
            unknown_environments,
            unknown_features,
        };

        let ctx = ExternalChangeContext {
            actor_user_id: integration.actor_user_id,
            external_ref: event.issue_key.clone(),
            external_status: status.to_string(),
            reason: format!("Jira status '{status}'"),
            external_actor: ExternalActor {
                system: SYSTEM_JIRA.to_string(),
                account_id: event.jira_actor.account_id.clone(),
                display_name: event.jira_actor.display_name.clone(),
            },
            trusted_approval: false,
        };
        for rule in matching_rules(rules, status) {
            // Rules are validated on save; skip one that is not runnable.
            let Some(action) = external_action(&rule.action) else {
                continue;
            };
            for (feature_id, feature_key) in &features {
                for target in &targets {
                    if rule
                        .environment_ids
                        .as_ref()
                        .is_some_and(|ids| !ids.contains(&target.id))
                    {
                        continue;
                    }
                    let ctx = ExternalChangeContext {
                        trusted_approval: integration
                            .jira_approved_environment_ids
                            .contains(&target.id),
                        ..ctx.clone()
                    };
                    let mut result = RuleResult {
                        feature_id: feature_id.to_string(),
                        feature_key: feature_key.clone(),
                        environment_id: target.id.to_string(),
                        environment: target.name.clone(),
                        rule_id: rule.id.to_string(),
                        rule_status: rule.jira_status.clone(),
                        action: rule.action.clone(),
                        outcome: OUTCOME_ERROR.to_string(),
                        from: None,
                        to: None,
                        reason: None,
                        approval_request_id: None,
                    };
                    match self
                        .external_change
                        .apply_external_action(*feature_id, target.id, action, &ctx)
                        .await
                    {
                        Ok(ExternalOutcome::Applied {
                            from,
                            to,
                            approval_request_id,
                        }) => {
                            result.outcome = OUTCOME_APPLIED.to_string();
                            result.from = Some(from);
                            result.to = Some(to);
                            result.approval_request_id =
                                approval_request_id.map(|id| id.to_string());
                        }
                        Ok(ExternalOutcome::NoOp { status }) => {
                            result.outcome = OUTCOME_NO_OP.to_string();
                            result.from = Some(status.clone());
                            result.to = Some(status);
                        }
                        Ok(ExternalOutcome::Refused { reason }) => {
                            result.outcome = OUTCOME_REFUSED.to_string();
                            result.reason = Some(reason);
                        }
                        Err(err) => {
                            log::warn!(
                                "Jira integration {}: {} on feature {} in {} failed: {err}",
                                integration.id,
                                rule.action,
                                feature_key,
                                target.name
                            );
                            result.reason = Some(err.to_string());
                        }
                    }
                    results.results.push(result);
                }
            }
        }
        Ok(results)
    }

    /// Features linked to the issue in the integration's team, then features
    /// named in `feature_key_field` (keys already linked are skipped), as
    /// (id, key). Also the field's keys that match no feature.
    async fn features_of(
        &self,
        integration: &JiraIntegrationRow,
        event: &ParsedEvent,
    ) -> Result<(Vec<(Uuid, String)>, Vec<String>), Error> {
        let team_id = integration.team_id;
        let mut features = Vec::new();
        for id in self
            .links
            .feature_ids_for_key(team_id, SYSTEM_JIRA, &event.issue_key)
            .await?
        {
            if let Some(scope) = self.links.feature_scope(id).await?
                && !features.iter().any(|(known, _)| *known == id)
            {
                features.push((id, scope.key));
            }
        }

        let mut unknown = Vec::new();
        if let Some(field) = &integration.feature_key_field {
            for key in field_values(&event.fields, field) {
                if features.iter().any(|(_, known)| *known == key) {
                    continue;
                }
                match self
                    .features
                    .get_feature_by_key(team_id, key.clone())
                    .await?
                {
                    Some(feature) if !features.iter().any(|(id, _)| *id == feature.id) => {
                        features.push((feature.id, feature.key))
                    }
                    Some(_) => {}
                    None => unknown.push(key),
                }
            }
        }
        Ok((features, unknown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::environment::MockEnvironmentRepository;
    use crate::database::external_link::{FeatureScope, MockExternalLinkRepository};
    use crate::database::feature::MockFeatureRepository;
    use crate::logic::external_change::MockExternalChangeLogic;
    use crate::logic::jira_events::JiraActor;
    use chrono::Utc;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn env(name: &str, active: bool) -> Environment {
        Environment {
            id: Uuid::new_v4(),
            name: name.to_string(),
            active,
            team_id: Uuid::nil(),
            environment_type: "Development".to_string(),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn rule(status: &str, action: &str, position: i32) -> JiraStatusRuleRow {
        JiraStatusRuleRow {
            id: Uuid::new_v4(),
            integration_id: Uuid::nil(),
            jira_status: status.to_string(),
            action: action.to_string(),
            environment_ids: None,
            enabled: true,
            position,
        }
    }

    #[test]
    fn resolve_environments_uses_aliases_then_names() {
        let qa = env("QA", true);
        let prod = env("Production", true);
        let old = env("Legacy", false);
        let teams = vec![qa.clone(), prod.clone(), old.clone()];
        let aliases = BTreeMap::from([
            ("Prod".to_string(), prod.id),
            ("Old".to_string(), old.id),
            ("Elsewhere".to_string(), Uuid::new_v4()),
        ]);

        let (targets, unknown) = resolve_environments(
            &strings(&[
                "qa",
                " PROD ",
                "Production",
                "Old",
                "Elsewhere",
                "Legacy",
                "Lab",
            ]),
            &aliases,
            &teams,
        );

        assert_eq!(
            targets,
            vec![
                EnvTarget {
                    id: qa.id,
                    name: "QA".to_string()
                },
                EnvTarget {
                    id: prod.id,
                    name: "Production".to_string()
                },
            ]
        );
        // Inactive environments, foreign alias targets and unknown names.
        assert_eq!(unknown, strings(&["Old", "Elsewhere", "Legacy", "Lab"]));
    }

    #[test]
    fn resolve_environments_refuses_an_ambiguous_name() {
        let teams = vec![env("QA", true), env("qa", true)];
        let (targets, unknown) = resolve_environments(&strings(&["QA"]), &BTreeMap::new(), &teams);
        assert!(targets.is_empty());
        assert_eq!(unknown, strings(&["QA"]));
    }

    #[test]
    fn matching_rules_ignores_case_spaces_and_disabled_rules_in_position_order() {
        let deploy = rule("Done", ACTION_DEPLOY, 2);
        let approve = rule(" done ", ACTION_APPROVE, 1);
        let mut disabled = rule("DONE", ACTION_ROLLBACK, 0);
        disabled.enabled = false;
        let other = rule("Ready", ACTION_REQUEST, 3);
        let rules = vec![deploy.clone(), approve.clone(), disabled, other];

        let matched = matching_rules(&rules, "  DONE");

        assert_eq!(matched, vec![&approve, &deploy]);
        assert!(matching_rules(&rules, "In Review").is_empty());
    }

    struct EngineFixture {
        integration: JiraIntegrationRow,
        qa: Environment,
        prod: Environment,
        features: Vec<(Uuid, String)>,
    }

    fn engine_fixture() -> EngineFixture {
        let qa = env("QA", true);
        let prod = env("Production", true);
        let integration = JiraIntegrationRow {
            id: Uuid::new_v4(),
            team_id: Uuid::new_v4(),
            name: "Jira".to_string(),
            jira_base_url: None,
            secret_hash: "hash".to_string(),
            environment_field: "customfield_10042".to_string(),
            environment_aliases: sqlx::types::Json(BTreeMap::new()),
            jira_approved_environment_ids: vec![qa.id],
            feature_key_field: None,
            actor_user_id: Uuid::new_v4(),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        EngineFixture {
            integration,
            qa,
            prod,
            features: vec![
                (Uuid::new_v4(), "checkout".to_string()),
                (Uuid::new_v4(), "search".to_string()),
            ],
        }
    }

    fn event(environments: serde_json::Value) -> ParsedEvent {
        ParsedEvent {
            issue_key: "PROJ-123".to_string(),
            target_status: Some("Done".to_string()),
            status_changed: true,
            jira_actor: JiraActor {
                account_id: Some("acc-1".to_string()),
                display_name: Some("Jane Doe".to_string()),
            },
            fields: json!({ "customfield_10042": environments })
                .as_object()
                .unwrap()
                .clone(),
            delivery_marker: Some("1".to_string()),
        }
    }

    type Calls = Arc<Mutex<Vec<(Uuid, Uuid, ExternalAction, bool, String)>>>;

    fn mocks(
        fx: &EngineFixture,
        fail_feature: Option<Uuid>,
    ) -> (
        MockExternalChangeLogic,
        MockExternalLinkRepository,
        MockFeatureRepository,
        MockEnvironmentRepository,
        Calls,
    ) {
        let calls: Calls = Arc::default();
        let mut external = MockExternalChangeLogic::new();
        let recorded = calls.clone();
        external.expect_apply_external_action().returning(
            move |feature, environment, action, ctx| {
                recorded.lock().unwrap().push((
                    feature,
                    environment,
                    action,
                    ctx.trusted_approval,
                    ctx.reason.clone(),
                ));
                if Some(feature) == fail_feature {
                    return Err(Error::DatabaseError(sqlx::Error::PoolTimedOut));
                }
                Ok(ExternalOutcome::Applied {
                    from: "NOT_DEPLOYED".to_string(),
                    to: "DEPLOYMENT_APPROVED".to_string(),
                    approval_request_id: None,
                })
            },
        );

        let mut links = MockExternalLinkRepository::new();
        let team_id = fx.integration.team_id;
        let ids: Vec<Uuid> = fx.features.iter().map(|(id, _)| *id).collect();
        links
            .expect_feature_ids_for_key()
            .withf(move |team, system, key| {
                *team == team_id && system == SYSTEM_JIRA && key == "PROJ-123"
            })
            .returning(move |_, _, _| Ok(ids.clone()));
        let keys = fx.features.clone();
        links.expect_feature_scope().returning(move |id| {
            Ok(keys
                .iter()
                .find(|(feature, _)| *feature == id)
                .map(|(_, key)| FeatureScope {
                    team_id,
                    key: key.clone(),
                }))
        });

        let mut environments = MockEnvironmentRepository::new();
        let team_envs = vec![fx.qa.clone(), fx.prod.clone()];
        environments
            .expect_get_environments()
            .withf(move |team, name, active| {
                *team == team_id && name.is_none() && *active == Some(true)
            })
            .returning(move |_, _, _| Ok(team_envs.clone()));

        (
            external,
            links,
            MockFeatureRepository::new(),
            environments,
            calls,
        )
    }

    #[tokio::test]
    async fn runs_each_rule_feature_and_environment_once_with_trust_per_environment() {
        let fx = engine_fixture();
        let (external, links, features, environments, calls) = mocks(&fx, None);
        let engine = RuleEngine {
            external_change: &external,
            links: &links,
            features: &features,
            environments: &environments,
        };
        let mut deploy_qa_only = rule("Done", ACTION_DEPLOY, 1);
        deploy_qa_only.environment_ids = Some(vec![fx.qa.id]);
        let rules = vec![
            rule("Done", ACTION_APPROVE, 0),
            deploy_qa_only,
            rule("Ready", ACTION_REQUEST, 2),
        ];

        let results = engine
            .run(
                &fx.integration,
                &rules,
                &event(json!([{"value": "QA"}, {"value": "Production"}, {"value": "Lab"}])),
                "Done",
            )
            .await
            .unwrap();

        let calls = calls.lock().unwrap().clone();
        let (checkout, search) = (fx.features[0].0, fx.features[1].0);
        // approve: 2 features x 2 environments; deploy: 2 features x QA only.
        assert_eq!(
            calls
                .iter()
                .map(|(feature, env, action, trusted, _)| (*feature, *env, *action, *trusted))
                .collect::<Vec<_>>(),
            vec![
                (checkout, fx.qa.id, ExternalAction::Approve, true),
                (checkout, fx.prod.id, ExternalAction::Approve, false),
                (search, fx.qa.id, ExternalAction::Approve, true),
                (search, fx.prod.id, ExternalAction::Approve, false),
                (checkout, fx.qa.id, ExternalAction::Deploy, true),
                (search, fx.qa.id, ExternalAction::Deploy, true),
            ]
        );
        assert!(calls.iter().all(|call| call.4 == "Jira status 'Done'"));
        assert_eq!(results.results.len(), 6);
        assert_eq!(results.unknown_environments, strings(&["Lab"]));
        let first = &results.results[0];
        assert_eq!(first.feature_key, "checkout");
        assert_eq!(first.environment, "QA");
        assert_eq!(first.action, ACTION_APPROVE);
        assert_eq!(first.outcome, OUTCOME_APPLIED);
        assert_eq!(first.to.as_deref(), Some("DEPLOYMENT_APPROVED"));
        assert_eq!(first.rule_id, rules[0].id.to_string());
    }

    #[tokio::test]
    async fn one_failing_target_does_not_stop_the_others() {
        let fx = engine_fixture();
        let failing = fx.features[0].0;
        let (external, links, features, environments, calls) = mocks(&fx, Some(failing));
        let engine = RuleEngine {
            external_change: &external,
            links: &links,
            features: &features,
            environments: &environments,
        };
        let rules = vec![rule("Done", ACTION_APPROVE, 0)];

        let results = engine
            .run(
                &fx.integration,
                &rules,
                &event(json!({"value": "QA"})),
                "Done",
            )
            .await
            .unwrap();

        assert_eq!(calls.lock().unwrap().len(), 2);
        assert_eq!(results.results[0].outcome, OUTCOME_ERROR);
        assert!(results.results[0].reason.is_some());
        assert_eq!(results.results[1].outcome, OUTCOME_APPLIED);
    }

    #[tokio::test]
    async fn no_matching_rule_calls_nothing() {
        let fx = engine_fixture();
        let (external, links, features, environments, calls) = mocks(&fx, None);
        let engine = RuleEngine {
            external_change: &external,
            links: &links,
            features: &features,
            environments: &environments,
        };

        let results = engine
            .run(
                &fx.integration,
                &[rule("Ready", ACTION_REQUEST, 0)],
                &event(json!({"value": "QA"})),
                "Done",
            )
            .await
            .unwrap();

        assert!(calls.lock().unwrap().is_empty());
        assert!(results.results.is_empty());
    }

    #[tokio::test]
    async fn feature_keys_from_the_key_field_add_features_and_report_unknown_keys() {
        let mut fx = engine_fixture();
        fx.integration.feature_key_field = Some("customfield_10050".to_string());
        fx.features.truncate(1);
        let (external, links, _, environments, calls) = mocks(&fx, None);
        let extra = crate::judgment::flag_kind::test_support::entity_feature("search");
        let extra_id = extra.id;
        let mut features = MockFeatureRepository::new();
        let team_id = fx.integration.team_id;
        features
            .expect_get_feature_by_key()
            .returning(move |team, key| {
                assert_eq!(team, team_id);
                Ok((key == "search").then(|| extra.clone()))
            });
        let engine = RuleEngine {
            external_change: &external,
            links: &links,
            features: &features,
            environments: &environments,
        };
        let mut parsed = event(json!({"value": "QA"}));
        parsed.fields.insert(
            "customfield_10050".to_string(),
            json!(["search", "missing", fx.features[0].1]),
        );

        let results = engine
            .run(
                &fx.integration,
                &[rule("Done", ACTION_APPROVE, 0)],
                &parsed,
                "Done",
            )
            .await
            .unwrap();

        let called: Vec<Uuid> = calls.lock().unwrap().iter().map(|call| call.0).collect();
        assert_eq!(called, vec![fx.features[0].0, extra_id]);
        assert_eq!(results.results[1].feature_key, "search");
        assert_eq!(results.unknown_features, strings(&["missing"]));
    }
}
