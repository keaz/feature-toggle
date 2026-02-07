use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

// Evaluation request with dynamic context object
#[derive(Deserialize, Clone, Debug)]
pub struct FeatureEvaluationContext {
    #[serde(rename = "flagKey")]
    pub flag_key: String,
    pub context: ContextObject,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ContextObject {
    // Support both targetingKey (OFREP standard) and bucketingKey (legacy)
    #[serde(alias = "bucketingKey", rename = "targetingKey")]
    pub targeting_key: String,
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
    Static,         // Feature is statically enabled/disabled (kill switch)
    TargetingMatch, // Criteria matched for this user
    Split,          // User is in rollout percentage
    Disabled,       // Stage or feature is disabled
    Unknown,        // Unknown reason (catch-all)
}

impl EvaluationReason {
    /// Returns the reason as a static string (zero-allocation)
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Static => "STATIC",
            Self::TargetingMatch => "TARGETING_MATCH",
            Self::Split => "SPLIT",
            Self::Disabled => "DISABLED",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ParseError,          // Malformed request/JSON
    TargetingKeyMissing, // Required targetingKey missing
    InvalidContext,      // Invalid context structure
    General,             // General evaluation error
    FlagNotFound,        // Flag doesn't exist
}

impl ErrorCode {
    /// Returns the error code as a static string (zero-allocation)
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ParseError => "PARSE_ERROR",
            Self::TargetingKeyMissing => "TARGETING_KEY_MISSING",
            Self::InvalidContext => "INVALID_CONTEXT",
            Self::General => "GENERAL",
            Self::FlagNotFound => "FLAG_NOT_FOUND",
        }
    }
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
    pub criterias: Vec<StageCriterion>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
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
    #[default]
    In,
    NotIn,
    SemverGreaterThan,
    SemverLessThan,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Default)]
pub enum VariantSelectionMode {
    #[default]
    WeightedSplit,
    SpecificVariant,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StageCriterion {
    pub priority: i32,
    #[serde(default)]
    pub rule_groups: Vec<RuleGroup>,
    /// Variant allocations for weighted traffic splits
    /// If present, overrides the simple serve field with weighted distribution
    #[serde(default)]
    pub variant_allocations: Vec<VariantAllocation>,
    /// Mode for variant selection: WEIGHTED_SPLIT or SPECIFIC_VARIANT
    #[serde(default)]
    pub variant_selection_mode: VariantSelectionMode,
    /// The specific variant to return when mode is SPECIFIC_VARIANT
    pub selected_variant_control: Option<String>,
}

// Compound rule structures for AND/OR logic
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogicOperator {
    And,
    Or,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RuleGroup {
    pub logic_operator: LogicOperator,
    pub conditions: Vec<RuleCondition>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RuleCondition {
    pub context_key: String,
    pub operator: Operator,
    pub value: JsonValue, // Can be string, array, etc.
}

/// Variant allocation for weighted traffic splits
/// Stores the weight (percentage) for a specific variant
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VariantAllocation {
    pub variant_control: String,
    pub weight: i32, // 0-100 percentage
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

static REGEX_CACHE: OnceLock<RwLock<HashMap<String, Option<Regex>>>> = OnceLock::new();

fn regex_cache() -> &'static RwLock<HashMap<String, Option<Regex>>> {
    REGEX_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn matches_regex_pattern(provided: &str, pattern: Option<&String>) -> bool {
    let Some(pattern) = pattern else {
        return false;
    };

    if let Ok(cache) = regex_cache().read()
        && let Some(cached) = cache.get(pattern)
    {
        return cached.as_ref().is_some_and(|re| re.is_match(provided));
    }

    let compiled = Regex::new(pattern).ok();
    if let Ok(mut cache) = regex_cache().write() {
        cache
            .entry(pattern.clone())
            .or_insert_with(|| compiled.clone());
    }

    compiled.as_ref().is_some_and(|re| re.is_match(provided))
}

fn allocations_sorted_by_control(allocations: &[VariantAllocation]) -> bool {
    allocations
        .windows(2)
        .all(|pair| pair[0].variant_control <= pair[1].variant_control)
}

/// Selects a variant based on weighted allocations using cumulative distribution
///
/// This function implements deterministic weighted traffic splitting:
/// - Sorts allocations by variant name for consistent ordering
/// - Calculates cumulative weight ranges (e.g., A:0-25, B:25-50, C:50-100)
/// - Returns the variant whose range contains the user_bucket value
///
/// # Arguments
/// * `allocations` - Slice of variant allocations with weights (0-100)
/// * `user_bucket` - User's deterministic bucket value (0.0-100.0)
///
/// # Returns
/// * `Some(String)` - The selected variant control name
/// * `None` - If no allocation matches (shouldn't happen if weights sum to 100)
///
/// # Example
/// ```
/// // allocations = [("A", 25), ("B", 25), ("C", 50)]
/// // user_bucket = 37.5
/// // Cumulative: A:0-25, B:25-50, C:50-100
/// // Result: "B" (because 37.5 falls in 25-50 range)
/// ```
fn select_variant_by_weight(allocations: &[VariantAllocation], user_bucket: f32) -> Option<String> {
    if allocations.is_empty() {
        return None;
    }

    if allocations_sorted_by_control(allocations) {
        let mut cumulative = 0.0;
        let mut last_variant = None;
        for alloc in allocations {
            cumulative += alloc.weight as f32;
            last_variant = Some(alloc.variant_control.as_str());
            if user_bucket < cumulative {
                return Some(alloc.variant_control.clone());
            }
        }
        return last_variant.map(str::to_owned);
    }

    // Sort allocations by variant name for deterministic ordering
    // This ensures the same user always gets the same variant
    let mut sorted = allocations.to_vec();
    sorted.sort_by(|a, b| a.variant_control.cmp(&b.variant_control));

    // Calculate cumulative weights and select variant
    let mut cumulative = 0.0;
    let mut last_variant = None;
    for alloc in &sorted {
        cumulative += alloc.weight as f32;
        last_variant = Some(alloc.variant_control.as_str());
        if user_bucket < cumulative {
            return Some(alloc.variant_control.clone());
        }
    }

    // Fallback: if user_bucket >= total weight, return last variant
    // This handles edge cases where weights don't sum to exactly 100
    last_variant.map(str::to_owned)
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

        Operator::Equals => allowed_values.first().is_some_and(|v| v == provided),

        Operator::NotEquals => allowed_values.first().is_some_and(|v| v != provided),

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

        Operator::Contains => allowed_values.first().is_some_and(|v| provided.contains(v)),

        Operator::StartsWith => allowed_values
            .first()
            .is_some_and(|v| provided.starts_with(v)),

        Operator::EndsWith => allowed_values
            .first()
            .is_some_and(|v| provided.ends_with(v)),

        Operator::Regex => matches_regex_pattern(provided, allowed_values.first()),

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

/// Evaluates a single rule condition
fn evaluate_rule_condition(condition: &RuleCondition, ctx: &ContextObject) -> bool {
    let provided = match get_context_attribute(ctx, &condition.context_key) {
        Some(v) => v,
        None => return false,
    };

    // Convert JsonValue to Vec<String> for operator matching
    let allowed_values: Vec<String> = match &condition.value {
        JsonValue::String(s) => vec![s.clone()],
        JsonValue::Number(n) => vec![n.to_string()],
        JsonValue::Bool(b) => vec![b.to_string()],
        JsonValue::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                JsonValue::String(s) => Some(s.clone()),
                JsonValue::Number(n) => Some(n.to_string()),
                JsonValue::Bool(b) => Some(b.to_string()),
                _ => None,
            })
            .collect(),
        _ => return false,
    };

    matches_operator(&condition.operator, &provided, &allowed_values)
}

/// Evaluates a rule group with AND/OR logic
fn evaluate_rule_group(group: &RuleGroup, ctx: &ContextObject) -> bool {
    if group.conditions.is_empty() {
        return false; // Empty group is considered false
    }

    let results: Vec<bool> = group
        .conditions
        .iter()
        .map(|c| evaluate_rule_condition(c, ctx))
        .collect();

    match group.logic_operator {
        LogicOperator::And => results.iter().all(|&r| r),
        LogicOperator::Or => results.iter().any(|&r| r),
    }
}

/// Evaluates all rule groups for a criterion (groups are OR'd together)
fn evaluate_compound_rules(rule_groups: &[RuleGroup], ctx: &ContextObject) -> bool {
    if rule_groups.is_empty() {
        return false; // No compound rules defined
    }

    // Multiple rule groups are OR'd together
    // e.g., (country=US AND tier=premium) OR (beta_user=true)
    rule_groups
        .iter()
        .any(|group| evaluate_rule_group(group, ctx))
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

    // Only compute a sticky bucket if any criterion uses weighted split mode
    let needs_bucket = stage.criterias.iter().any(|crit| {
        crit.variant_selection_mode == VariantSelectionMode::WeightedSplit
            && !crit.variant_allocations.is_empty()
    });
    let user_bucket = if needs_bucket {
        // Always use targeting_key from evaluation context (OpenFeature standard)
        let sticky_val = &ec.context.targeting_key;

        if sticky_val.is_empty() {
            return CriteriaEvaluationResult {
                matched: false,
                variant: None,
                reason: EvaluationReason::Unknown,
            };
        }

        // Precompute user bucket percentage
        let mut hasher = Sha256::new();
        hasher.update(ec.flag_key.as_bytes());
        hasher.update(b":");
        hasher.update(sticky_val.as_bytes());
        let digest = hasher.finalize();
        Some(hash_to_percentage(&digest)) // 0..100
    } else {
        None
    };

    // Evaluate criteria in order (by priority, lowest first)
    // Note: Criteria should be pre-sorted by the caller (database query sorts by priority ASC)
    for crit in &stage.criterias {
        let matches = if !crit.rule_groups.is_empty() {
            // Use compound rules if defined
            evaluate_compound_rules(&crit.rule_groups, &ec.context)
        } else {
            // No rules means always match
            true
        };

        if matches {
            let selected_variant = match crit.variant_selection_mode {
                VariantSelectionMode::SpecificVariant => {
                    // Return the specific variant for all users
                    crit.selected_variant_control.clone()
                }
                VariantSelectionMode::WeightedSplit => {
                    // Use weighted distribution based on user bucket
                    if !crit.variant_allocations.is_empty() {
                        user_bucket.and_then(|bucket| {
                            select_variant_by_weight(&crit.variant_allocations, bucket)
                        })
                    } else {
                        None
                    }
                }
            };

            return CriteriaEvaluationResult {
                matched: true,
                variant: selected_variant,
                reason: EvaluationReason::TargetingMatch,
            };
        }
    }

    CriteriaEvaluationResult {
        matched: false,
        variant: None,
        reason: EvaluationReason::Unknown,
    }
}

fn get_variant_value(feature: &Feature, variant_control: Option<String>) -> JsonValue {
    if let Some(control) = variant_control
        && let Some(variant) = feature.variants.iter().find(|v| v.control == control)
    {
        return variant.value.clone();
    }

    // Default value if no variant found
    JsonValue::Bool(true)
}

fn evaluate_with_memo(
    evaluation_context: &FeatureEvaluationContext,
    feature: &Feature,
    memo: &mut HashMap<String, EvaluationResult>,
) -> EvaluationResult {
    if let Some(cached) = memo.get(&feature.id) {
        return cached.clone();
    }

    // Evaluation walkthrough:
    // 1) Kill switch: if the feature is disabled, short-circuit to false.
    // 2) Resolve dependencies recursively; any failed dependency disables this feature.
    // 3) Select the stage matching the requested environment; if missing or disabled, return default/disabled.
    // 4) Evaluate criteria in priority order. Each criterion can have compound rule groups (OR of groups; AND within a group).
    //    If a criterion matches, optionally select a variant via weighted allocations; otherwise return default false.
    // 5) When a variant is selected, return its value; otherwise default to true for contextual flags.
    let flag_key = evaluation_context.flag_key.clone();

    // Kill switch check: if enabled is false, feature is disabled
    if !feature.enabled {
        let result = EvaluationResult {
            flag_key,
            value: JsonValue::Bool(false),
            variant: None,
            reason: EvaluationReason::Static,
            error_code: None,
            metadata: None,
        };
        memo.insert(feature.id.clone(), result.clone());
        return result;
    }

    // All dependencies must evaluate to true
    for dependency in &feature.dependencies {
        let dep_result = evaluate_with_memo(evaluation_context, dependency, memo);
        if !dep_result.value.as_bool().unwrap_or(false) {
            let result = EvaluationResult {
                flag_key,
                value: JsonValue::Bool(false),
                variant: None,
                reason: EvaluationReason::Disabled,
                error_code: None,
                metadata: None,
            };
            memo.insert(feature.id.clone(), result.clone());
            return result;
        }
    }

    // There must be a stage matching the environment_id and it must be enabled
    let stage = match feature
        .stages
        .iter()
        .find(|stage| stage.environment_id == evaluation_context.context.environment_id)
    {
        None => {
            let result = EvaluationResult {
                flag_key,
                value: JsonValue::Bool(false),
                variant: None,
                reason: EvaluationReason::Unknown,
                error_code: Some(ErrorCode::FlagNotFound),
                metadata: None,
            };
            memo.insert(feature.id.clone(), result.clone());
            return result;
        }
        Some(stage) => stage,
    };

    // Stage must be enabled
    if !stage.enabled {
        let result = EvaluationResult {
            flag_key,
            value: JsonValue::Bool(false),
            variant: None,
            reason: EvaluationReason::Disabled,
            error_code: None,
            metadata: None,
        };
        memo.insert(feature.id.clone(), result.clone());
        return result;
    }

    // Evaluate stage criteria
    let criteria_result = passes_stage_criteria(evaluation_context, stage);

    if !criteria_result.matched {
        let result = EvaluationResult {
            flag_key,
            value: JsonValue::Bool(false),
            variant: None,
            reason: criteria_result.reason,
            error_code: None,
            metadata: None,
        };
        memo.insert(feature.id.clone(), result.clone());
        return result;
    }

    // Feature is enabled, determine variant and value
    let variant = criteria_result.variant;
    let value = get_variant_value(feature, variant.clone());

    let result = EvaluationResult {
        flag_key,
        value,
        variant,
        reason: criteria_result.reason,
        error_code: None,
        metadata: None,
    };
    memo.insert(feature.id.clone(), result.clone());
    result
}

pub fn evaluate(
    evaluation_context: &FeatureEvaluationContext,
    feature: &Feature,
) -> EvaluationResult {
    let mut memo = HashMap::new();
    evaluate_with_memo(evaluation_context, feature, &mut memo)
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
                targeting_key: "user123".to_string(),
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
                criterias: vec![StageCriterion {
                    priority: 0,
                    rule_groups: vec![RuleGroup {
                        logic_operator: LogicOperator::And,
                        conditions: vec![RuleCondition {
                            context_key: "role".to_string(),
                            operator: Operator::In,
                            value: json!(["admin"]),
                        }],
                    }],
                    variant_allocations: vec![VariantAllocation {
                        variant_control: "treatment".to_string(),
                        weight: 100,
                    }],
                    variant_selection_mode: VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
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

        let result = evaluate(&context, &feature);
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
                targeting_key: "user123".to_string(),
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

        let result = evaluate(&context, &feature);
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
            &["admin".to_string(), "user".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::In,
            "guest",
            &["admin".to_string(), "user".to_string()]
        ));
    }

    #[test]
    fn test_operator_not_in() {
        assert!(matches_operator(
            &Operator::NotIn,
            "guest",
            &["admin".to_string(), "user".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::NotIn,
            "admin",
            &["admin".to_string(), "user".to_string()]
        ));
    }

    #[test]
    fn test_operator_equals() {
        assert!(matches_operator(
            &Operator::Equals,
            "admin",
            &["admin".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::Equals,
            "user",
            &["admin".to_string()]
        ));
    }

    #[test]
    fn test_operator_not_equals() {
        assert!(matches_operator(
            &Operator::NotEquals,
            "user",
            &["admin".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::NotEquals,
            "admin",
            &["admin".to_string()]
        ));
    }

    #[test]
    fn test_operator_greater_than() {
        assert!(matches_operator(
            &Operator::GreaterThan,
            "100",
            &["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::GreaterThan,
            "30",
            &["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::GreaterThan,
            "50",
            &["50".to_string()]
        ));
        // Test with decimals
        assert!(matches_operator(
            &Operator::GreaterThan,
            "10.5",
            &["10.2".to_string()]
        ));
    }

    #[test]
    fn test_operator_less_than() {
        assert!(matches_operator(
            &Operator::LessThan,
            "30",
            &["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::LessThan,
            "100",
            &["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::LessThan,
            "50",
            &["50".to_string()]
        ));
    }

    #[test]
    fn test_operator_greater_than_or_equal() {
        assert!(matches_operator(
            &Operator::GreaterThanOrEqual,
            "100",
            &["50".to_string()]
        ));
        assert!(matches_operator(
            &Operator::GreaterThanOrEqual,
            "50",
            &["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::GreaterThanOrEqual,
            "30",
            &["50".to_string()]
        ));
    }

    #[test]
    fn test_operator_less_than_or_equal() {
        assert!(matches_operator(
            &Operator::LessThanOrEqual,
            "30",
            &["50".to_string()]
        ));
        assert!(matches_operator(
            &Operator::LessThanOrEqual,
            "50",
            &["50".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::LessThanOrEqual,
            "100",
            &["50".to_string()]
        ));
    }

    #[test]
    fn test_operator_contains() {
        assert!(matches_operator(
            &Operator::Contains,
            "admin@example.com",
            &["@example.com".to_string()]
        ));
        assert!(matches_operator(
            &Operator::Contains,
            "hello world test",
            &["world".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::Contains,
            "user@test.com",
            &["@example.com".to_string()]
        ));
    }

    #[test]
    fn test_operator_starts_with() {
        assert!(matches_operator(
            &Operator::StartsWith,
            "admin@example.com",
            &["admin".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::StartsWith,
            "user@example.com",
            &["admin".to_string()]
        ));
    }

    #[test]
    fn test_operator_ends_with() {
        assert!(matches_operator(
            &Operator::EndsWith,
            "admin@example.com",
            &[".com".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::EndsWith,
            "admin@example.org",
            &[".com".to_string()]
        ));
    }

    #[test]
    fn test_operator_regex() {
        // Match email pattern
        assert!(matches_operator(
            &Operator::Regex,
            "test@example.com",
            &[r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::Regex,
            "invalid-email",
            &[r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()]
        ));

        // Match phone number pattern
        assert!(matches_operator(
            &Operator::Regex,
            "+1-555-123-4567",
            &[r"^\+\d{1,3}-\d{3}-\d{3}-\d{4}$".to_string()]
        ));
    }

    #[test]
    fn test_operator_regex_invalid_pattern() {
        // Invalid regex pattern should return false
        assert!(!matches_operator(
            &Operator::Regex,
            "test",
            &["[invalid(".to_string()]
        ));
    }

    #[test]
    fn test_operator_semver_greater_than() {
        assert!(matches_operator(
            &Operator::SemverGreaterThan,
            "2.0.0",
            &["1.5.0".to_string()]
        ));
        assert!(matches_operator(
            &Operator::SemverGreaterThan,
            "1.6.0",
            &["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "1.4.0",
            &["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "1.5.0",
            &["1.5.0".to_string()]
        ));
    }

    #[test]
    fn test_operator_semver_less_than() {
        assert!(matches_operator(
            &Operator::SemverLessThan,
            "1.4.0",
            &["1.5.0".to_string()]
        ));
        assert!(matches_operator(
            &Operator::SemverLessThan,
            "0.9.0",
            &["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverLessThan,
            "2.0.0",
            &["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverLessThan,
            "1.5.0",
            &["1.5.0".to_string()]
        ));
    }

    #[test]
    fn test_operator_semver_invalid() {
        // Invalid semver should return false
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "invalid",
            &["1.5.0".to_string()]
        ));
        assert!(!matches_operator(
            &Operator::SemverGreaterThan,
            "1.5.0",
            &["invalid".to_string()]
        ));
    }

    #[test]
    fn test_evaluation_with_operator_greater_than() {
        let mut context_attrs = HashMap::new();
        context_attrs.insert("age".to_string(), json!("25"));

        let context = FeatureEvaluationContext {
            flag_key: "age-restricted-feature".to_string(),
            context: ContextObject {
                targeting_key: "user456".to_string(),
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
                criterias: vec![StageCriterion {
                    priority: 0,
                    rule_groups: vec![RuleGroup {
                        logic_operator: LogicOperator::And,
                        conditions: vec![RuleCondition {
                            context_key: "age".to_string(),
                            operator: Operator::GreaterThanOrEqual,
                            value: json!("18"),
                        }],
                    }],
                    variant_allocations: vec![VariantAllocation {
                        variant_control: "adult-content".to_string(),
                        weight: 100,
                    }],
                    variant_selection_mode: VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
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

        let result = evaluate(&context, &feature);
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
                targeting_key: "user789".to_string(),
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
                criterias: vec![StageCriterion {
                    priority: 0,
                    rule_groups: vec![RuleGroup {
                        logic_operator: LogicOperator::And,
                        conditions: vec![RuleCondition {
                            context_key: "email".to_string(),
                            operator: Operator::Regex,
                            value: json!(r".*@company\.com$"),
                        }],
                    }],
                    variant_allocations: vec![VariantAllocation {
                        variant_control: "corporate-variant".to_string(),
                        weight: 100,
                    }],
                    variant_selection_mode: VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
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

        let result = evaluate(&context, &feature);
        assert_eq!(result.variant, Some("corporate-variant".to_string()));
        assert_eq!(result.value, json!("Corporate feature enabled"));
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);
    }

    // Compound rule tests
    #[test]
    fn test_compound_rule_and_logic_all_match() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("US"));
        attrs.insert("tier".to_string(), json!("premium"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![
                RuleCondition {
                    context_key: "country".to_string(),
                    operator: Operator::Equals,
                    value: json!("US"),
                },
                RuleCondition {
                    context_key: "tier".to_string(),
                    operator: Operator::Equals,
                    value: json!("premium"),
                },
            ],
        };

        assert!(evaluate_rule_group(&group, &ctx));
    }

    #[test]
    fn test_compound_rule_and_logic_partial_match() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("US"));
        attrs.insert("tier".to_string(), json!("free"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![
                RuleCondition {
                    context_key: "country".to_string(),
                    operator: Operator::Equals,
                    value: json!("US"),
                },
                RuleCondition {
                    context_key: "tier".to_string(),
                    operator: Operator::Equals,
                    value: json!("premium"),
                },
            ],
        };

        assert!(!evaluate_rule_group(&group, &ctx)); // Should fail - not all conditions match
    }

    #[test]
    fn test_compound_rule_or_logic_any_match() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("UK"));
        attrs.insert("beta_user".to_string(), json!("true"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::Or,
            conditions: vec![
                RuleCondition {
                    context_key: "country".to_string(),
                    operator: Operator::Equals,
                    value: json!("US"),
                },
                RuleCondition {
                    context_key: "beta_user".to_string(),
                    operator: Operator::Equals,
                    value: json!("true"),
                },
            ],
        };

        assert!(evaluate_rule_group(&group, &ctx)); // Should pass - at least one condition matches
    }

    #[test]
    fn test_compound_rule_or_logic_no_match() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("UK"));
        attrs.insert("beta_user".to_string(), json!("false"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::Or,
            conditions: vec![
                RuleCondition {
                    context_key: "country".to_string(),
                    operator: Operator::Equals,
                    value: json!("US"),
                },
                RuleCondition {
                    context_key: "beta_user".to_string(),
                    operator: Operator::Equals,
                    value: json!("true"),
                },
            ],
        };

        assert!(!evaluate_rule_group(&group, &ctx)); // Should fail - no conditions match
    }

    #[test]
    fn test_compound_rule_with_numeric_operators() {
        let mut attrs = HashMap::new();
        attrs.insert("age".to_string(), json!("25"));
        attrs.insert("account_value".to_string(), json!("5000"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![
                RuleCondition {
                    context_key: "age".to_string(),
                    operator: Operator::GreaterThanOrEqual,
                    value: json!("18"),
                },
                RuleCondition {
                    context_key: "account_value".to_string(),
                    operator: Operator::GreaterThan,
                    value: json!("1000"),
                },
            ],
        };

        assert!(evaluate_rule_group(&group, &ctx));
    }

    #[test]
    fn test_compound_rule_with_in_operator() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("CA"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![RuleCondition {
                context_key: "country".to_string(),
                operator: Operator::In,
                value: json!(["US", "CA", "UK"]),
            }],
        };

        assert!(evaluate_rule_group(&group, &ctx));
    }

    #[test]
    fn test_compound_rule_with_string_operators() {
        let mut attrs = HashMap::new();
        attrs.insert("email".to_string(), json!("user@example.com"));
        attrs.insert("plan".to_string(), json!("enterprise"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![
                RuleCondition {
                    context_key: "email".to_string(),
                    operator: Operator::EndsWith,
                    value: json!("@example.com"),
                },
                RuleCondition {
                    context_key: "plan".to_string(),
                    operator: Operator::Contains,
                    value: json!("enterprise"),
                },
            ],
        };

        assert!(evaluate_rule_group(&group, &ctx));
    }

    #[test]
    fn test_multiple_rule_groups_or_behavior() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("FR"));
        attrs.insert("beta_user".to_string(), json!("true"));

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let rule_groups = vec![
            // First group: (country=US AND tier=premium) - Won't match
            RuleGroup {
                logic_operator: LogicOperator::And,
                conditions: vec![
                    RuleCondition {
                        context_key: "country".to_string(),
                        operator: Operator::Equals,
                        value: json!("US"),
                    },
                    RuleCondition {
                        context_key: "tier".to_string(),
                        operator: Operator::Equals,
                        value: json!("premium"),
                    },
                ],
            },
            // Second group: beta_user=true - Will match
            RuleGroup {
                logic_operator: LogicOperator::And,
                conditions: vec![RuleCondition {
                    context_key: "beta_user".to_string(),
                    operator: Operator::Equals,
                    value: json!("true"),
                }],
            },
        ];

        assert!(evaluate_compound_rules(&rule_groups, &ctx)); // Should pass - second group matches
    }

    #[test]
    fn test_empty_rule_group() {
        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: HashMap::new(),
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![],
        };

        assert!(!evaluate_rule_group(&group, &ctx)); // Empty group should return false
    }

    #[test]
    fn test_empty_rule_groups_array() {
        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: HashMap::new(),
        };

        assert!(!evaluate_compound_rules(&[], &ctx)); // No groups should return false
    }

    #[test]
    fn test_compound_rule_missing_context_attribute() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("US"));
        // tier attribute is missing

        let ctx = ContextObject {
            targeting_key: "user123".to_string(),
            environment_id: "env1".to_string(),
            attributes: attrs,
        };

        let group = RuleGroup {
            logic_operator: LogicOperator::And,
            conditions: vec![
                RuleCondition {
                    context_key: "country".to_string(),
                    operator: Operator::Equals,
                    value: json!("US"),
                },
                RuleCondition {
                    context_key: "tier".to_string(), // This will be missing
                    operator: Operator::Equals,
                    value: json!("premium"),
                },
            ],
        };

        assert!(!evaluate_rule_group(&group, &ctx)); // Should fail - missing attribute
    }

    #[test]
    fn test_full_evaluation_with_compound_rules() {
        let mut attrs = HashMap::new();
        attrs.insert("country".to_string(), json!("US"));
        attrs.insert("tier".to_string(), json!("premium"));

        let context = FeatureEvaluationContext {
            flag_key: "premium-feature".to_string(),
            context: ContextObject {
                targeting_key: "user123".to_string(),
                environment_id: "env1".to_string(),
                attributes: attrs,
            },
        };

        let feature = Feature {
            id: "f1".to_string(),
            key: "premium-feature".to_string(),
            feature_type: "CONTEXTUAL".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![FeatureStage {
                environment_id: "env1".to_string(),
                enabled: true,
                criterias: vec![StageCriterion {
                    priority: 0,
                    rule_groups: vec![RuleGroup {
                        logic_operator: LogicOperator::And,
                        conditions: vec![
                            RuleCondition {
                                context_key: "country".to_string(),
                                operator: Operator::Equals,
                                value: json!("US"),
                            },
                            RuleCondition {
                                context_key: "tier".to_string(),
                                operator: Operator::Equals,
                                value: json!("premium"),
                            },
                        ],
                    }],
                    variant_allocations: vec![VariantAllocation {
                        variant_control: "premium-variant".to_string(),
                        weight: 100,
                    }],
                    variant_selection_mode: VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
                }],
            }],
            variants: vec![FeatureVariant {
                control: "premium-variant".to_string(),
                value: json!("Premium features unlocked"),
            }],
        };

        let result = evaluate(&context, &feature);
        assert_eq!(result.variant, Some("premium-variant".to_string()));
        assert_eq!(result.value, json!("Premium features unlocked"));
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);
    }

    // ============================================
    // Weighted Variant Selection Tests
    // ============================================

    #[test]
    fn test_select_variant_by_weight_basic() {
        let allocations = vec![
            VariantAllocation {
                variant_control: "variant-a".to_string(),
                weight: 25,
            },
            VariantAllocation {
                variant_control: "variant-b".to_string(),
                weight: 25,
            },
            VariantAllocation {
                variant_control: "variant-c".to_string(),
                weight: 50,
            },
        ];

        // Test bucket falls in first range (0-25)
        assert_eq!(
            select_variant_by_weight(&allocations, 10.0),
            Some("variant-a".to_string())
        );

        // Test bucket falls in second range (25-50)
        assert_eq!(
            select_variant_by_weight(&allocations, 37.5),
            Some("variant-b".to_string())
        );

        // Test bucket falls in third range (50-100)
        assert_eq!(
            select_variant_by_weight(&allocations, 75.0),
            Some("variant-c".to_string())
        );

        // Test edge case: exactly at boundary
        assert_eq!(
            select_variant_by_weight(&allocations, 25.0),
            Some("variant-b".to_string())
        );
    }

    #[test]
    fn test_select_variant_by_weight_deterministic_ordering() {
        // Test that variants are sorted alphabetically for deterministic results
        let allocations = vec![
            VariantAllocation {
                variant_control: "z-last".to_string(),
                weight: 50,
            },
            VariantAllocation {
                variant_control: "a-first".to_string(),
                weight: 25,
            },
            VariantAllocation {
                variant_control: "m-middle".to_string(),
                weight: 25,
            },
        ];

        // After sorting: a-first (0-25), m-middle (25-50), z-last (50-100)
        assert_eq!(
            select_variant_by_weight(&allocations, 10.0),
            Some("a-first".to_string())
        );
        assert_eq!(
            select_variant_by_weight(&allocations, 37.5),
            Some("m-middle".to_string())
        );
        assert_eq!(
            select_variant_by_weight(&allocations, 75.0),
            Some("z-last".to_string())
        );
    }

    #[test]
    fn test_select_variant_by_weight_unequal_weights() {
        let allocations = vec![
            VariantAllocation {
                variant_control: "small".to_string(),
                weight: 10,
            },
            VariantAllocation {
                variant_control: "large".to_string(),
                weight: 90,
            },
        ];

        // Test small range (0-10)
        assert_eq!(
            select_variant_by_weight(&allocations, 5.0),
            Some("large".to_string()) // After sorting: large (0-90), small (90-100)
        );

        // Test large range
        assert_eq!(
            select_variant_by_weight(&allocations, 50.0),
            Some("large".to_string())
        );

        assert_eq!(
            select_variant_by_weight(&allocations, 95.0),
            Some("small".to_string())
        );
    }

    #[test]
    fn test_select_variant_by_weight_edge_cases() {
        let allocations = vec![
            VariantAllocation {
                variant_control: "variant-a".to_string(),
                weight: 50,
            },
            VariantAllocation {
                variant_control: "variant-b".to_string(),
                weight: 50,
            },
        ];

        // Test at 0
        assert_eq!(
            select_variant_by_weight(&allocations, 0.0),
            Some("variant-a".to_string())
        );

        // Test at 100 (should return last variant as fallback)
        assert_eq!(
            select_variant_by_weight(&allocations, 100.0),
            Some("variant-b".to_string())
        );

        // Test slightly over 100 (edge case handling)
        assert_eq!(
            select_variant_by_weight(&allocations, 100.1),
            Some("variant-b".to_string())
        );
    }

    #[test]
    fn test_select_variant_by_weight_empty_allocations() {
        let allocations: Vec<VariantAllocation> = vec![];
        assert_eq!(select_variant_by_weight(&allocations, 50.0), None);
    }

    #[test]
    fn test_select_variant_by_weight_single_variant() {
        let allocations = vec![VariantAllocation {
            variant_control: "only-variant".to_string(),
            weight: 100,
        }];

        assert_eq!(
            select_variant_by_weight(&allocations, 0.0),
            Some("only-variant".to_string())
        );
        assert_eq!(
            select_variant_by_weight(&allocations, 50.0),
            Some("only-variant".to_string())
        );
        assert_eq!(
            select_variant_by_weight(&allocations, 99.9),
            Some("only-variant".to_string())
        );
    }

    #[test]
    fn test_select_variant_by_weight_partial_total() {
        // Test when weights don't sum to 100 (e.g., 80 total)
        let allocations = vec![
            VariantAllocation {
                variant_control: "variant-a".to_string(),
                weight: 30,
            },
            VariantAllocation {
                variant_control: "variant-b".to_string(),
                weight: 50,
            },
        ];

        // Buckets 0-30 -> variant-a
        assert_eq!(
            select_variant_by_weight(&allocations, 15.0),
            Some("variant-a".to_string())
        );

        // Buckets 30-80 -> variant-b
        assert_eq!(
            select_variant_by_weight(&allocations, 60.0),
            Some("variant-b".to_string())
        );

        // Bucket 80-100 -> fallback to last variant
        assert_eq!(
            select_variant_by_weight(&allocations, 90.0),
            Some("variant-b".to_string())
        );
    }

    #[test]
    fn test_weighted_variant_evaluation_integration() {
        // Integration test: Full evaluation with weighted variants
        let mut context_attrs = HashMap::new();
        context_attrs.insert("tier".to_string(), json!("premium"));

        let context = FeatureEvaluationContext {
            flag_key: "ab-test-feature".to_string(),
            context: ContextObject {
                targeting_key: "user-42".to_string(), // This will generate a deterministic bucket
                environment_id: "env1".to_string(),
                attributes: context_attrs,
            },
        };

        let feature = Feature {
            id: "test-id".to_string(),
            key: "ab-test-feature".to_string(),
            feature_type: "Contextual".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![FeatureStage {
                environment_id: "env1".to_string(),
                enabled: true,
                criterias: vec![StageCriterion {
                    priority: 0,
                    rule_groups: vec![],
                    variant_allocations: vec![
                        VariantAllocation {
                            variant_control: "control".to_string(),
                            weight: 25,
                        },
                        VariantAllocation {
                            variant_control: "variant-a".to_string(),
                            weight: 25,
                        },
                        VariantAllocation {
                            variant_control: "variant-b".to_string(),
                            weight: 50,
                        },
                    ],
                    variant_selection_mode: VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
                }],
            }],
            variants: vec![
                FeatureVariant {
                    control: "control".to_string(),
                    value: json!({"ui": "original"}),
                },
                FeatureVariant {
                    control: "variant-a".to_string(),
                    value: json!({"ui": "redesign-a"}),
                },
                FeatureVariant {
                    control: "variant-b".to_string(),
                    value: json!({"ui": "redesign-b"}),
                },
            ],
        };

        let result = evaluate(&context, &feature);

        // The result should be one of the three variants (deterministic based on bucketing_key)
        assert!(result.variant.is_some());
        let variant = result.variant.unwrap();
        assert!(
            variant == "control" || variant == "variant-a" || variant == "variant-b",
            "Expected one of the configured variants, got: {}",
            variant
        );
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);
    }

    #[test]
    fn test_weighted_variants_with_partial_rollout() {
        // Test weighted variants combined with rollout percentage
        let mut context_attrs = HashMap::new();
        context_attrs.insert("region".to_string(), json!("us-west"));

        let context = FeatureEvaluationContext {
            flag_key: "regional-test".to_string(),
            context: ContextObject {
                targeting_key: "user-100".to_string(), // Choose a key that falls in low bucket
                environment_id: "env1".to_string(),
                attributes: context_attrs,
            },
        };

        let feature = Feature {
            id: "test-id".to_string(),
            key: "regional-test".to_string(),
            feature_type: "Contextual".to_string(),
            active: true,
            enabled: true,
            dependencies: vec![],
            stages: vec![FeatureStage {
                environment_id: "env1".to_string(),
                enabled: true,
                criterias: vec![StageCriterion {
                    priority: 0,
                    rule_groups: vec![],
                    variant_allocations: vec![
                        VariantAllocation {
                            variant_control: "control".to_string(),
                            weight: 50,
                        },
                        VariantAllocation {
                            variant_control: "treatment".to_string(),
                            weight: 50,
                        },
                    ],
                    variant_selection_mode: VariantSelectionMode::WeightedSplit,
                    selected_variant_control: None,
                }],
            }],
            variants: vec![
                FeatureVariant {
                    control: "control".to_string(),
                    value: json!(false),
                },
                FeatureVariant {
                    control: "treatment".to_string(),
                    value: json!(true),
                },
            ],
        };

        let result = evaluate(&context, &feature);

        // Should get a variant (deterministic)
        assert!(result.variant.is_some());
        let variant = result.variant.unwrap();
        assert!(
            variant == "control" || variant == "treatment",
            "Expected control or treatment, got: {}",
            variant
        );
    }
}
