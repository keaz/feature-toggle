//! Validation and secret handling for Jira integrations and their status rules
//! (design §3.7). The writes, with their activity rows, are in `jira_integration_tx`.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use regex::Regex;
use uuid::Uuid;

use crate::Error;
use crate::database::jira_integration::NewJiraStatusRule;

pub const ACTION_REQUEST: &str = "request";
pub const ACTION_APPROVE: &str = "approve";
pub const ACTION_DEPLOY: &str = "deploy";
pub const ACTION_ROLLBACK: &str = "rollback";
pub const RULE_ACTIONS: [&str; 4] = [
    ACTION_REQUEST,
    ACTION_APPROVE,
    ACTION_DEPLOY,
    ACTION_ROLLBACK,
];

pub const MAX_NAME_LENGTH: usize = 100;
pub const MAX_FIELD_LENGTH: usize = 100;
pub const MAX_ALIAS_LENGTH: usize = 100;
pub const MAX_STATUS_LENGTH: usize = 100;

static CUSTOM_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^customfield_[0-9]+$").expect("valid custom field regex"));
static SYSTEM_FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z_]+$").expect("valid system field regex"));

/// A new inbound secret: 32 random bytes, base64url without padding.
pub fn generate_secret() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 (hex) of the secret, the only form stored.
pub fn hash_secret(secret: &str) -> String {
    crate::middleware::jwt_guard::hash_token(secret)
}

/// Trimmed name, 1 to [`MAX_NAME_LENGTH`] characters.
pub fn validate_integration_name(name: &str) -> Result<String, Error> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
        return Err(Error::InvalidInput(format!(
            "name must be 1 to {MAX_NAME_LENGTH} characters"
        )));
    }
    Ok(name.to_string())
}

/// A Jira field name: `labels`, a custom field id (`customfield_<digits>`) or a
/// system field name made of `[a-z_]`. Trimmed.
pub fn validate_field_name(value: &str, label: &str) -> Result<String, Error> {
    let value = value.trim();
    if value.len() <= MAX_FIELD_LENGTH
        && (CUSTOM_FIELD.is_match(value)
            || (SYSTEM_FIELD.is_match(value) && !value.starts_with("customfield")))
    {
        Ok(value.to_string())
    } else {
        Err(Error::InvalidInput(format!(
            "{label} must be 'labels', a custom field id such as 'customfield_10042' \
             or a system field name made of a-z and _"
        )))
    }
}

/// Jira value -> environment id. Keys are trimmed, non-empty and unique ignoring
/// case; values are ids of environments of the team (`team_environment_ids`).
pub fn validate_environment_aliases(
    aliases: &BTreeMap<String, String>,
    team_environment_ids: &HashSet<Uuid>,
) -> Result<BTreeMap<String, Uuid>, Error> {
    let mut seen = HashSet::new();
    let mut result = BTreeMap::new();
    for (alias, environment_id) in aliases {
        let alias = alias.trim();
        if alias.is_empty() || alias.chars().count() > MAX_ALIAS_LENGTH {
            return Err(Error::InvalidInput(format!(
                "environmentAliases keys must be 1 to {MAX_ALIAS_LENGTH} characters"
            )));
        }
        if !seen.insert(alias.to_lowercase()) {
            return Err(Error::InvalidInput(format!(
                "environmentAliases has '{alias}' more than once (keys are compared ignoring case)"
            )));
        }
        let environment_id =
            team_environment_id(environment_id, team_environment_ids, "environmentAliases")?;
        result.insert(alias.to_string(), environment_id);
    }
    Ok(result)
}

/// Environment ids that all belong to the team, without duplicates, in input order.
pub fn validate_environment_ids(
    ids: &[String],
    team_environment_ids: &HashSet<Uuid>,
    label: &str,
) -> Result<Vec<Uuid>, Error> {
    let mut result: Vec<Uuid> = Vec::with_capacity(ids.len());
    for id in ids {
        let id = team_environment_id(id, team_environment_ids, label)?;
        if !result.contains(&id) {
            result.push(id);
        }
    }
    Ok(result)
}

fn team_environment_id(
    value: &str,
    team_environment_ids: &HashSet<Uuid>,
    label: &str,
) -> Result<Uuid, Error> {
    Uuid::parse_str(value.trim())
        .ok()
        .filter(|id| team_environment_ids.contains(id))
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "{label}: '{value}' is not an environment of this team"
            ))
        })
}

/// One status rule as sent by a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraStatusRuleInput {
    pub jira_status: String,
    pub action: String,
    /// `None` matches every environment the issue names.
    pub environment_ids: Option<Vec<String>>,
    pub enabled: bool,
}

/// Validates the whole rule list. Status trimmed, 1 to [`MAX_STATUS_LENGTH`]
/// characters; action one of [`RULE_ACTIONS`]; environment filter, when given,
/// non-empty and of the team; no two rules with the same status (ignoring case)
/// and action. Order is kept: it is the rule `position`.
pub fn validate_rules(
    rules: &[JiraStatusRuleInput],
    team_environment_ids: &HashSet<Uuid>,
) -> Result<Vec<NewJiraStatusRule>, Error> {
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(rules.len());
    for rule in rules {
        let jira_status = rule.jira_status.trim();
        if jira_status.is_empty()
            || jira_status.chars().count() > MAX_STATUS_LENGTH
            || jira_status.chars().any(char::is_control)
        {
            return Err(Error::InvalidInput(format!(
                "jiraStatus must be 1 to {MAX_STATUS_LENGTH} characters without control characters"
            )));
        }
        if !RULE_ACTIONS.contains(&rule.action.as_str()) {
            return Err(Error::InvalidInput(format!(
                "action must be one of {}",
                RULE_ACTIONS.join(", ")
            )));
        }
        let environment_ids = match &rule.environment_ids {
            None => None,
            Some(ids) if ids.is_empty() => {
                return Err(Error::InvalidInput(
                    "environmentIds must not be empty; omit it to match every environment"
                        .to_string(),
                ));
            }
            Some(ids) => Some(validate_environment_ids(
                ids,
                team_environment_ids,
                "environmentIds",
            )?),
        };
        if !seen.insert((jira_status.to_lowercase(), rule.action.clone())) {
            return Err(Error::InvalidInput(format!(
                "more than one '{}' rule for status '{jira_status}'",
                rule.action
            )));
        }
        result.push(NewJiraStatusRule {
            jira_status: jira_status.to_string(),
            action: rule.action.clone(),
            environment_ids,
            enabled: rule.enabled,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid<T: std::fmt::Debug>(result: Result<T, Error>) -> bool {
        matches!(result, Err(Error::InvalidInput(_)))
    }

    fn rule(status: &str, action: &str, envs: Option<Vec<String>>) -> JiraStatusRuleInput {
        JiraStatusRuleInput {
            jira_status: status.to_string(),
            action: action.to_string(),
            environment_ids: envs,
            enabled: true,
        }
    }

    #[test]
    fn generate_secret_is_32_random_bytes_base64url() {
        let a = generate_secret();
        let b = generate_secret();
        assert_ne!(a, b);
        assert!(!a.contains('='), "no padding: {a}");
        assert_eq!(URL_SAFE_NO_PAD.decode(&a).expect("base64url").len(), 32);
    }

    #[test]
    fn hash_secret_is_sha256_hex() {
        assert_eq!(
            hash_secret("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn integration_name_is_trimmed_and_bounded() {
        assert_eq!(
            validate_integration_name("  Jira PROJ ").unwrap(),
            "Jira PROJ"
        );
        assert!(invalid(validate_integration_name("   ")));
        assert!(invalid(validate_integration_name(
            &"n".repeat(MAX_NAME_LENGTH + 1)
        )));
        assert!(validate_integration_name(&"n".repeat(MAX_NAME_LENGTH)).is_ok());
    }

    #[test]
    fn field_name_accepts_labels_custom_fields_and_system_fields() {
        for (input, expected) in [
            ("labels", "labels"),
            (" customfield_10042 ", "customfield_10042"),
            ("components", "components"),
            ("fix_versions", "fix_versions"),
        ] {
            assert_eq!(
                validate_field_name(input, "environmentField")
                    .unwrap_or_else(|err| panic!("{input:?}: {err}")),
                expected
            );
        }
    }

    #[test]
    fn field_name_rejects_other_values() {
        for input in [
            "",
            "  ",
            "customfield_",
            "customfield_12a",
            "Labels",
            "fix-versions",
            "custom field",
            "labels;drop",
            "fields.labels",
        ] {
            assert!(
                invalid(validate_field_name(input, "environmentField")),
                "{input:?} should be rejected"
            );
        }
        assert!(invalid(validate_field_name(
            &"a".repeat(MAX_FIELD_LENGTH + 1),
            "environmentField"
        )));
    }

    #[test]
    fn aliases_map_trimmed_keys_to_team_environments() {
        let qa = Uuid::new_v4();
        let prod = Uuid::new_v4();
        let team: HashSet<Uuid> = [qa, prod].into();
        let aliases = BTreeMap::from([
            (" QA env ".to_string(), qa.to_string()),
            ("Production".to_string(), prod.to_string()),
        ]);
        assert_eq!(
            validate_environment_aliases(&aliases, &team).unwrap(),
            BTreeMap::from([("QA env".to_string(), qa), ("Production".to_string(), prod)])
        );
        assert!(
            validate_environment_aliases(&BTreeMap::new(), &team)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn aliases_reject_blank_or_duplicate_keys_and_foreign_or_bad_environments() {
        let qa = Uuid::new_v4();
        let team: HashSet<Uuid> = [qa].into();
        let cases = [
            BTreeMap::from([("  ".to_string(), qa.to_string())]),
            BTreeMap::from([("QA".to_string(), Uuid::new_v4().to_string())]),
            BTreeMap::from([("QA".to_string(), "qa".to_string())]),
            BTreeMap::from([
                ("QA".to_string(), qa.to_string()),
                ("qa ".to_string(), qa.to_string()),
            ]),
            BTreeMap::from([("a".repeat(MAX_ALIAS_LENGTH + 1), qa.to_string())]),
        ];
        for aliases in cases {
            assert!(
                invalid(validate_environment_aliases(&aliases, &team)),
                "{aliases:?} should be rejected"
            );
        }
    }

    #[test]
    fn environment_ids_must_belong_to_the_team_and_are_deduplicated() {
        let qa = Uuid::new_v4();
        let prod = Uuid::new_v4();
        let team: HashSet<Uuid> = [qa, prod].into();
        assert_eq!(
            validate_environment_ids(
                &[prod.to_string(), qa.to_string(), prod.to_string()],
                &team,
                "jiraApprovedEnvironmentIds"
            )
            .unwrap(),
            vec![prod, qa]
        );
        assert!(invalid(validate_environment_ids(
            &[Uuid::new_v4().to_string()],
            &team,
            "jiraApprovedEnvironmentIds"
        )));
        assert!(invalid(validate_environment_ids(
            &["not-a-uuid".to_string()],
            &team,
            "jiraApprovedEnvironmentIds"
        )));
    }

    #[test]
    fn rules_are_trimmed_and_keep_their_order() {
        let qa = Uuid::new_v4();
        let team: HashSet<Uuid> = [qa].into();
        let rules = validate_rules(
            &[
                rule(" Ready for Release ", "approve", Some(vec![qa.to_string()])),
                rule("Done", "deploy", None),
                rule("Done", "approve", None),
                JiraStatusRuleInput {
                    enabled: false,
                    ..rule("Reopened", "rollback", None)
                },
            ],
            &team,
        )
        .unwrap();
        assert_eq!(
            rules,
            vec![
                NewJiraStatusRule {
                    jira_status: "Ready for Release".to_string(),
                    action: "approve".to_string(),
                    environment_ids: Some(vec![qa]),
                    enabled: true,
                },
                NewJiraStatusRule {
                    jira_status: "Done".to_string(),
                    action: "deploy".to_string(),
                    environment_ids: None,
                    enabled: true,
                },
                NewJiraStatusRule {
                    jira_status: "Done".to_string(),
                    action: "approve".to_string(),
                    environment_ids: None,
                    enabled: true,
                },
                NewJiraStatusRule {
                    jira_status: "Reopened".to_string(),
                    action: "rollback".to_string(),
                    environment_ids: None,
                    enabled: false,
                },
            ]
        );
        assert!(validate_rules(&[], &team).unwrap().is_empty());
    }

    #[test]
    fn rules_reject_bad_status_action_environments_and_duplicates() {
        let qa = Uuid::new_v4();
        let team: HashSet<Uuid> = [qa].into();
        let long_status = "s".repeat(MAX_STATUS_LENGTH + 1);
        let cases: Vec<Vec<JiraStatusRuleInput>> = vec![
            vec![rule("  ", "deploy", None)],
            vec![rule(&long_status, "deploy", None)],
            vec![rule("Done\u{7}", "deploy", None)],
            vec![rule("Done", "release", None)],
            vec![rule("Done", "Deploy", None)],
            vec![rule("Done", "deploy", Some(vec![]))],
            vec![rule(
                "Done",
                "deploy",
                Some(vec![Uuid::new_v4().to_string()]),
            )],
            vec![rule("Done", "deploy", None), rule(" done ", "deploy", None)],
        ];
        for rules in cases {
            assert!(
                invalid(validate_rules(&rules, &team)),
                "{rules:?} should be rejected"
            );
        }
    }
}
