use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

// Evaluation request with dynamic context object
#[derive(Deserialize, Clone, Debug)]
pub struct FeatureEvaluationContext {
    #[serde(rename = "flagKey")]
    pub flag_key: String,
    pub context: ContextObject,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ContextObject {
    #[serde(rename = "bucketingKey")]
    pub bucketing_key: String,
    #[serde(rename = "environment_id")]
    pub environment_id: String,
    #[serde(flatten)]
    pub attributes: HashMap<String, JsonValue>,
}

// Evaluation response
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EvaluationResult {
    #[serde(rename = "flagKey")]
    pub flag_key: String,
    pub value: JsonValue,
    pub variant: Option<String>,
    pub reason: EvaluationReason,
    #[serde(rename = "errorCode")]
    pub error_code: Option<ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, JsonValue>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvaluationReason {
    Static,           // Feature is statically enabled/disabled (kill switch)
    Default,          // Default value returned (feature not found, stage not found, etc.)
    TargetingMatch,   // Criteria matched for this user
    Split,            // User is in rollout percentage
    Cached,           // Value returned from cache
    DependencyFailed, // Feature disabled due to dependency failure
    Disabled,         // Stage or feature is disabled
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    FlagNotFound,
    TypeMismatch,
    TargetingKeyMissing,
    EnvironmentNotFound,
    InvalidContext,
    EvaluationError,
}

// Feature data structures
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Feature {
    pub id: String,
    pub key: String,
    pub feature_type: String,
    pub active: bool,
    pub enabled: bool,
    pub dependencies: Vec<Feature>,
    pub stages: Vec<FeatureStage>,
    pub variants: Vec<FeatureVariant>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FeatureVariant {
    pub control: String,
    pub value: JsonValue,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FeatureStage {
    pub environment_id: String,
    pub enabled: bool,
    pub bucketing_key: Option<String>,
    pub criterias: Vec<StageCriterion>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StageContext {
    pub key: String,
    pub entries: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Operator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Contains,
    StartsWith,
    EndsWith,
    Regex,
    In,
    NotIn,
    SemverGreaterThan,
    SemverLessThan,
}

impl Default for Operator {
    fn default() -> Self {
        Operator::In
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StageCriterion {
    pub context_key: String,
    pub context: StageContext,
    pub rollout_percentage: i32,
    pub serve: Option<String>,
    #[serde(default)]
    pub operator: Operator,
}

fn get_context_attribute(ctx: &ContextObject, key: &str) -> Option<String> {
    ctx.attributes.get(key).and_then(|v| match v {
        JsonValue::String(s) => Some(s.clone()),
        JsonValue::Number(n) => Some(n.to_string()),
        JsonValue::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn hash_to_percentage(hash: &[u8]) -> f32 {
    // Use first 8 bytes to create a u64 number, then map to [0.0, 100.0)
    if hash.len() < 8 {
        return 0.0;
    }
    let mut eight = [0u8; 8];
    eight.copy_from_slice(&hash[0..8]);
    let val = u64::from_be_bytes(eight);
    let ratio = (val as f64) / (u64::MAX as f64);
    (ratio * 100.0) as f32
}

struct CriteriaEvaluationResult {
    matched: bool,
    variant: Option<String>,
    reason: EvaluationReason,
}

/// Evaluates a value against a criterion using the specified operator
fn matches_operator(operator: &Operator, provided: &str, allowed_values: &[String]) -> bool {
    match operator {
        Operator::In => allowed_values.iter().any(|v| v == provided),

        Operator::NotIn => !allowed_values.iter().any(|v| v == provided),

        Operator::Equals => {
            allowed_values.first().map_or(false, |v| v == provided)
        }

        Operator::NotEquals => {
            allowed_values.first().map_or(false, |v| v != provided)
        }

        Operator::GreaterThan => {
            if let (Ok(provided_num), Some(Ok(allowed_num))) = (
                provided.parse::<f64>(),
                allowed_values.first().map(|v| v.parse::<f64>()),
            ) {
                provided_num > allowed_num
            } else {
                false
            }
        }

        Operator::LessThan => {
            if let (Ok(provided_num), Some(Ok(allowed_num))) = (
                provided.parse::<f64>(),
                allowed_values.first().map(|v| v.parse::<f64>()),
            ) {
                provided_num < allowed_num
            } else {
                false
            }
        }

        Operator::GreaterThanOrEqual => {
            if let (Ok(provided_num), Some(Ok(allowed_num))) = (
                provided.parse::<f64>(),
                allowed_values.first().map(|v| v.parse::<f64>()),
            ) {
                provided_num >= allowed_num
            } else {
                false
            }
        }

        Operator::LessThanOrEqual => {
            if let (Ok(provided_num), Some(Ok(allowed_num))) = (
                provided.parse::<f64>(),
                allowed_values.first().map(|v| v.parse::<f64>()),
            ) {
                provided_num <= allowed_num
            } else {
                false
            }
        }

        Operator::Contains => {
            allowed_values.first().map_or(false, |v| provided.contains(v))
        }

        Operator::StartsWith => {
            allowed_values.first().map_or(false, |v| provided.starts_with(v))
        }

        Operator::EndsWith => {
            allowed_values.first().map_or(false, |v| provided.ends_with(v))
        }

        Operator::Regex => {
            allowed_values.first().and_then(|pattern| {
                Regex::new(pattern).ok().map(|re| re.is_match(provided))
            }).unwrap_or(false)
        }

        Operator::SemverGreaterThan => {
            if let (Ok(provided_ver), Some(allowed_ver)) = (
                Version::parse(provided),
                allowed_values.first().and_then(|v| Version::parse(v).ok()),
            ) {
                provided_ver > allowed_ver
            } else {
                false
            }
        }

        Operator::SemverLessThan => {
            if let (Ok(provided_ver), Some(allowed_ver)) = (
                Version::parse(provided),
                allowed_values.first().and_then(|v| Version::parse(v).ok()),
            ) {
                provided_ver < allowed_ver
            } else {
                false
            }
        }
    }
}

fn passes_stage_criteria(
    ec: &FeatureEvaluationContext,
    stage: &FeatureStage,
) -> CriteriaEvaluationResult {
    // If no criteria defined, treat as pass-through (stage gating only)
    if stage.criterias.is_empty() {
        return CriteriaEvaluationResult {
            matched: true,
            variant: None,
            reason: EvaluationReason::Static,
        };
    }

    // Use bucketing_key from stage or default to bucketingKey from context
    let sticky_key = stage.bucketing_key.as_deref().unwrap_or("bucketingKey");
    let sticky_val = if sticky_key == "bucketingKey" {
        ec.context.bucketing_key.clone()
    } else {
        match get_context_attribute(&ec.context, sticky_key) {
            Some(v) => v,
            None => {
                return CriteriaEvaluationResult {
                    matched: false,
                    variant: None,
                    reason: EvaluationReason::Default,
                };
            }
        }
    };

    if sticky_val.is_empty() {
        return CriteriaEvaluationResult {
            matched: false,
            variant: None,
            reason: EvaluationReason::Default,
        };
    }

    // Precompute user bucket percentage
    let mut hasher = Sha256::new();
    hasher.update(ec.flag_key.as_bytes());
    hasher.update(b":");
    hasher.update(sticky_val.as_bytes());
    let digest = hasher.finalize();
    let user_bucket = hash_to_percentage(&digest); // 0..100

    // Evaluate criteria in order (by priority, lowest first)
    // Note: Criteria should be pre-sorted by the caller (database query sorts by priority ASC)
    for crit in &stage.criterias {
        // Find provided value for the actual context key
        let ctx_key = &crit.context_key;
        if let Some(provided) = get_context_attribute(&ec.context, ctx_key) {
            // Check using operator-based matching
            if matches_operator(&crit.operator, &provided, &crit.context.entries) {
                let pct = crit.rollout_percentage.clamp(0, 100) as f32;
                if user_bucket < pct {
                    return CriteriaEvaluationResult {
                        matched: true,
                        variant: crit.serve.clone(),
                        reason: EvaluationReason::TargetingMatch,
                    };
                } else {
                    return CriteriaEvaluationResult {
                        matched: true,
                        variant: None,
                        reason: EvaluationReason::TargetingMatch,
                    };
                }
            }
        }
    }

    CriteriaEvaluationResult {
        matched: false,
        variant: None,
        reason: EvaluationReason::Default,
    }
}

fn get_variant_value(feature: &Feature, variant_control: Option<String>) -> JsonValue {
    if let Some(control) = variant_control {
        if let Some(variant) = feature.variants.iter().find(|v| v.control == control) {
            return variant.value.clone();
        }
    }

    // Default value if no variant found
    JsonValue::Bool(true)
}

pub fn evaluate(
    evaluation_context: FeatureEvaluationContext,
    feature: Feature,
) -> EvaluationResult {
    let flag_key = evaluation_context.flag_key.clone();

    // Kill switch check: if enabled is false, feature is disabled
    if !feature.enabled {
        return EvaluationResult {
            flag_key,
            value: JsonValue::Bool(false),
            variant: None,
            reason: EvaluationReason::Static,
            error_code: None,
            metadata: None,
        };
    }

    // All dependencies must evaluate to true
    for dependency in feature.dependencies.clone() {
        let dep_result = evaluate(evaluation_context.clone(), dependency);
        if !dep_result.value.as_bool().unwrap_or(false) {
            return EvaluationResult {
                flag_key,
                value: JsonValue::Bool(false),
                variant: None,
                reason: EvaluationReason::DependencyFailed,
                error_code: None,
                metadata: None,
            };
        }
    }

    // There must be a stage matching the environment_id and it must be enabled
    let stage = match feature
        .stages
        .iter()
        .find(|stage| stage.environment_id == evaluation_context.context.environment_id)
    {
        None => {
            return EvaluationResult {
                flag_key,
                value: JsonValue::Bool(false),
                variant: None,
                reason: EvaluationReason::Default,
                error_code: Some(ErrorCode::EnvironmentNotFound),
                metadata: None,
            };
        }
        Some(stage) => stage,
    };

    // Stage must be enabled
    if !stage.enabled {
        return EvaluationResult {
            flag_key,
            value: JsonValue::Bool(false),
            variant: None,
            reason: EvaluationReason::Disabled,
            error_code: None,
            metadata: None,
        };
    }

    // Evaluate stage criteria
    let criteria_result = passes_stage_criteria(&evaluation_context, stage);

    if !criteria_result.matched {
        return EvaluationResult {
            flag_key,
            value: JsonValue::Bool(false),
            variant: None,
            reason: criteria_result.reason,
            error_code: None,
            metadata: None,
        };
    }

    // Feature is enabled, determine variant and value
    let variant = criteria_result.variant;
    let value = get_variant_value(&feature, variant.clone());

    EvaluationResult {
        flag_key,
        value,
        variant,
        reason: criteria_result.reason,
        error_code: None,
        metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_evaluation_with_variant() {
        let mut context_attrs = HashMap::new();
        context_attrs.insert("role".to_string(), json!("admin"));

        let context = FeatureEvaluationContext {
            flag_key: "test-feature".to_string(),
            context: ContextObject {
                bucketing_key: "user123".to_string(),
                environment_id: "env1".to_string(),
                attributes: context_attrs,
            },
        };

        let feature = Feature {
            id: "test-id".to_string(),
            key: "test-feature".to_string(),
            feature_type: "Contextual".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![FeatureStage {
                environment_id: "env1".to_string(),
                enabled: true,
                bucketing_key: None,
                criterias: vec![StageCriterion {
                    context_key: "role".to_string(),
                    context: StageContext {
                        key: "role".to_string(),
                        entries: vec!["admin".to_string()],
                    },
                    rollout_percentage: 100,
                    serve: Some("treatment".to_string()),
                    operator: Operator::In,
                }],
            }],
            variants: vec![
                FeatureVariant {
                    control: "control".to_string(),
                    value: json!(false),
                },
                FeatureVariant {
                    control: "treatment".to_string(),
                    value: json!("Enhanced UI"),
                },
            ],
        };

        let result = evaluate(context, feature);
        assert_eq!(result.variant, Some("treatment".to_string()));
        assert_eq!(result.value, json!("Enhanced UI"));
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);
        assert_eq!(result.error_code, None);
    }

    #[test]
    fn test_disabled_feature() {
        let context = FeatureEvaluationContext {
            flag_key: "test-feature".to_string(),
            context: ContextObject {
                bucketing_key: "user123".to_string(),
                environment_id: "env1".to_string(),
                attributes: HashMap::new(),
            },
        };

        let feature = Feature {
            id: "test-id".to_string(),
            key: "test-feature".to_string(),
            feature_type: "Simple".to_string(),
            active: true,
            enabled: false,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        };

        let result = evaluate(context, feature);
        assert_eq!(result.value, json!(false));
        assert_eq!(result.reason, EvaluationReason::Static);
    }

    // ============================================
    // Operator Tests
    // ============================================

    #[test]
    fn test_operator_in() {
        assert!(matches_operator(
            &Operator::In,
            "admin",
            &vec!["admin".to_string(), "user".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::In,
            "guest",
            &vec!["admin".to_string(), "user".to_string()]
        ));
    }

    #[test]
    fn test_operator_not_in() {
        assert!(matches_operator(
            &Operator::NotIn,
            "guest",
            &vec!["admin".to_string(), "user".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::NotIn,
            "admin",
            &vec!["admin".to_string(), "user".to_string()]
        ));
    }

    #[test]
    fn test_operator_equals() {
        assert!(matches_operator(
            &Operator::Equals,
            "admin",
            &vec!["admin".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::Equals,
            "user",
            &vec!["admin".to_string()]
        ));
    }

    #[test]
    fn test_operator_not_equals() {
        assert!(matches_operator(
            &Operator::NotEquals,
            "user",
            &vec!["admin".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::NotEquals,
            "admin",
            &vec!["admin".to_string()]
        ));
    }

    #[test]
    fn test_operator_greater_than() {
        assert!(matches_operator(
            &Operator::GreaterThan,
            "100",
            &vec!["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::GreaterThan,
            "30",
            &vec!["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::GreaterThan,
            "50",
            &vec!["50".to_string()]
        ));
        // Test with decimals
        assert!(matches_operator(
            &Operator::GreaterThan,
            "10.5",
            &vec!["10.2".to_string()]
        ));
    }

    #[test]
    fn test_operator_less_than() {
        assert!(matches_operator(
            &Operator::LessThan,
            "30",
            &vec!["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::LessThan,
            "100",
            &vec!["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::LessThan,
            "50",
            &vec!["50".to_string()]
        ));
    }

    #[test]
    fn test_operator_greater_than_or_equal() {
        assert!(matches_operator(
            &Operator::GreaterThanOrEqual,
            "100",
            &vec!["50".to_string()]
        ));
        assert!(matches_operator(
            &Operator::GreaterThanOrEqual,
            "50",
            &vec!["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::GreaterThanOrEqual,
            "30",
            &vec!["50".to_string()]
        ));
    }

    #[test]
    fn test_operator_less_than_or_equal() {
        assert!(matches_operator(
            &Operator::LessThanOrEqual,
            "30",
            &vec!["50".to_string()]
        ));
        assert!(matches_operator(
            &Operator::LessThanOrEqual,
            "50",
            &vec!["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::LessThanOrEqual,
            "100",
            &vec!["50".to_string()]
        ));
    }

    #[test]
    fn test_operator_contains() {
        assert!(matches_operator(
            &Operator::Contains,
            "admin@example.com",
            &vec!["@example.com".to_string()]
        ));
        assert!(matches_operator(
            &Operator::Contains,
            "hello world test",
            &vec!["world".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::Contains,
            "user@test.com",
            &vec!["@example.com".to_string()]
        ));
    }

    #[test]
    fn test_operator_starts_with() {
        assert!(matches_operator(
            &Operator::StartsWith,
            "admin@example.com",
            &vec!["admin".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::StartsWith,
            "user@example.com",
            &vec!["admin".to_string()]
        ));
    }

    #[test]
    fn test_operator_ends_with() {
        assert!(matches_operator(
            &Operator::EndsWith,
            "admin@example.com",
            &vec![".com".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::EndsWith,
            "admin@example.org",
            &vec![".com".to_string()]
        ));
    }

    #[test]
    fn test_operator_regex() {
        // Match email pattern
        assert!(matches_operator(
            &Operator::Regex,
            "test@example.com",
            &vec![r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::Regex,
            "invalid-email",
            &vec![r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()]
        ));

        // Match phone number pattern
        assert!(matches_operator(
            &Operator::Regex,
            "+1-555-123-4567",
            &vec![r"^\+\d{1,3}-\d{3}-\d{3}-\d{4}$".to_string()]
        ));
    }

    #[test]
    fn test_operator_regex_invalid_pattern() {
        // Invalid regex pattern should return false
        assert!(!matches_operator(
            &Operator::Regex,
            "test",
            &vec!["[invalid(".to_string()]
        ));
    }

    #[test]
    fn test_operator_semver_greater_than() {
        assert!(matches_operator(
            &Operator::SemverGreaterThan,
            "2.0.0",
            &vec!["1.5.0".to_string()]
        ));
        assert!(matches_operator(
            &Operator::SemverGreaterThan,
            "1.6.0",
            &vec!["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "1.4.0",
            &vec!["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "1.5.0",
            &vec!["1.5.0".to_string()]
        ));
    }

    #[test]
    fn test_operator_semver_less_than() {
        assert!(matches_operator(
            &Operator::SemverLessThan,
            "1.4.0",
            &vec!["1.5.0".to_string()]
        ));
        assert!(matches_operator(
            &Operator::SemverLessThan,
            "0.9.0",
            &vec!["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverLessThan,
            "2.0.0",
            &vec!["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverLessThan,
            "1.5.0",
            &vec!["1.5.0".to_string()]
        ));
    }

    #[test]
    fn test_operator_semver_invalid() {
        // Invalid semver should return false
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "invalid",
            &vec!["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "1.5.0",
            &vec!["invalid".to_string()]
        ));
    }

    #[test]
    fn test_evaluation_with_operator_greater_than() {
        let mut context_attrs = HashMap::new();
        context_attrs.insert("age".to_string(), json!("25"));

        let context = FeatureEvaluationContext {
            flag_key: "age-restricted-feature".to_string(),
            context: ContextObject {
                bucketing_key: "user456".to_string(),
                environment_id: "env1".to_string(),
                attributes: context_attrs,
            },
        };

        let feature = Feature {
            id: "test-id".to_string(),
            key: "age-restricted-feature".to_string(),
            feature_type: "Contextual".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![FeatureStage {
                environment_id: "env1".to_string(),
                enabled: true,
                bucketing_key: None,
                criterias: vec![StageCriterion {
                    context_key: "age".to_string(),
                    context: StageContext {
                        key: "age".to_string(),
                        entries: vec!["18".to_string()],
                    },
                    rollout_percentage: 100,
                    serve: Some("adult-content".to_string()),
                    operator: Operator::GreaterThanOrEqual,
                }],
            }],
            variants: vec![
                FeatureVariant {
                    control: "control".to_string(),
                    value: json!(false),
                },
                FeatureVariant {
                    control: "adult-content".to_string(),
                    value: json!("Access granted"),
                },
            ],
        };

        let result = evaluate(context, feature);
        assert_eq!(result.variant, Some("adult-content".to_string()));
        assert_eq!(result.value, json!("Access granted"));
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);
    }

    #[test]
    fn test_evaluation_with_operator_regex() {
        let mut context_attrs = HashMap::new();
        context_attrs.insert("email".to_string(), json!("admin@company.com"));

        let context = FeatureEvaluationContext {
            flag_key: "corporate-feature".to_string(),
            context: ContextObject {
                bucketing_key: "user789".to_string(),
                environment_id: "env1".to_string(),
                attributes: context_attrs,
            },
        };

        let feature = Feature {
            id: "test-id".to_string(),
            key: "corporate-feature".to_string(),
            feature_type: "Contextual".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![FeatureStage {
                environment_id: "env1".to_string(),
                enabled: true,
                bucketing_key: None,
                criterias: vec![StageCriterion {
                    context_key: "email".to_string(),
                    context: StageContext {
                        key: "email".to_string(),
                        entries: vec![r".*@company\.com$".to_string()],
                    },
                    rollout_percentage: 100,
                    serve: Some("corporate-variant".to_string()),
                    operator: Operator::Regex,
                }],
            }],
            variants: vec![
                FeatureVariant {
                    control: "control".to_string(),
                    value: json!(false),
                },
                FeatureVariant {
                    control: "corporate-variant".to_string(),
                    value: json!("Corporate feature enabled"),
                },
            ],
        };

        let result = evaluate(context, feature);
        assert_eq!(result.variant, Some("corporate-variant".to_string()));
        assert_eq!(result.value, json!("Corporate feature enabled"));
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);
    }
}
