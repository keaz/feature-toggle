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
    Static,            // Feature is statically enabled/disabled (kill switch)
    Default,           // Default value returned (feature not found, stage not found, etc.)
    TargetingMatch,    // Criteria matched for this user
    Split,             // User is in rollout percentage
    Cached,            // Value returned from cache
    DependencyFailed,  // Feature disabled due to dependency failure
    Disabled,          // Stage or feature is disabled
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StageCriterion {
    pub context_key: String,
    pub context: StageContext,
    pub rollout_percentage: i32,
    pub serve: Option<String>,
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

fn passes_stage_criteria(ec: &FeatureEvaluationContext, stage: &FeatureStage) -> CriteriaEvaluationResult {
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
            // Check allowed values
            if crit.context.entries.iter().any(|v| v == &provided) {
                let pct = crit.rollout_percentage.clamp(0, 100) as f32;
                if user_bucket < pct {
                    return CriteriaEvaluationResult {
                        matched: true,
                        variant: crit.serve.clone(),
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

pub fn evaluate(evaluation_context: FeatureEvaluationContext, feature: Feature) -> EvaluationResult {
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
            enabled: false,
            dependencies: vec![],
            stages: vec![],
            variants: vec![],
        };

        let result = evaluate(context, feature);
        assert_eq!(result.value, json!(false));
        assert_eq!(result.reason, EvaluationReason::Static);
    }
}
