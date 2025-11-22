use evaluation_engine::{
    ContextObject, ErrorCode, EvaluationReason, Feature, FeatureEvaluationContext, FeatureStage,
    FeatureVariant, LogicOperator, Operator, RuleCondition, RuleGroup, StageCriterion,
    VariantAllocation,
};
use serde_json::json;
use std::collections::HashMap;

fn mk_ctx(
    flag_key: &str,
    env: &str,
    bucketing_key: &str,
    attrs: &[(&str, &str)],
) -> FeatureEvaluationContext {
    let mut attributes = HashMap::new();
    for (k, v) in attrs {
        attributes.insert((*k).to_string(), json!(*v));
    }

    FeatureEvaluationContext {
        flag_key: flag_key.to_string(),
        context: ContextObject {
            bucketing_key: bucketing_key.to_string(),
            environment_id: env.to_string(),
            attributes,
        },
    }
}

fn stage(
    env: &str,
    enabled: bool,
    bucketing: Option<&str>,
    criterias: Vec<StageCriterion>,
) -> FeatureStage {
    FeatureStage {
        environment_id: env.to_string(),
        enabled,
        bucketing_key: bucketing.map(|s| s.to_string()),
        criterias,
    }
}

fn rule(context_key: &str, operator: Operator, value: serde_json::Value) -> RuleCondition {
    RuleCondition {
        context_key: context_key.to_string(),
        operator,
        value,
    }
}

fn criterion(rules: Vec<RuleCondition>, variant: Option<&str>, priority: i32) -> StageCriterion {
    StageCriterion {
        priority,
        rule_groups: if rules.is_empty() {
            vec![]
        } else {
            vec![RuleGroup {
                logic_operator: LogicOperator::And,
                conditions: rules,
            }]
        },
        variant_allocations: variant
            .map(|v| {
                vec![VariantAllocation {
                    variant_control: v.to_string(),
                    weight: 100,
                }]
            })
            .unwrap_or_default(),
    }
}

fn mk_feature(
    id: &str,
    key: &str,
    feature_type: &str,
    active: bool,
    enabled: bool,
    stages: Vec<FeatureStage>,
    variants: Vec<FeatureVariant>,
) -> Feature {
    Feature {
        id: id.to_string(),
        key: key.to_string(),
        feature_type: feature_type.to_string(),
        active,
        enabled,
        dependencies: vec![],
        stages,
        variants,
    }
}

#[test]
fn evaluate_returns_false_when_feature_disabled() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let feature = mk_feature("test-1", "feat", "Simple", true, false, vec![], vec![]);
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Static);
}

#[test]
fn evaluate_requires_matching_environment_stage() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let stg = stage("env-b", true, None, vec![]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Default);
    assert_eq!(result.error_code, Some(ErrorCode::EnvironmentNotFound));
}

#[test]
fn evaluate_requires_stage_enabled() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let stg = stage("env-a", false, None, vec![]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Disabled);
}

#[test]
fn evaluate_passes_when_no_criteria_and_enabled_stage() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let stg = stage("env-a", true, None, vec![]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::Static);
}

#[test]
fn evaluate_unconditional_criterion_matches() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let crit = criterion(vec![], Some("control"), 0);
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = mk_feature(
        "test-id",
        "test-key",
        "Contextual",
        true,
        true,
        vec![stg],
        vec![FeatureVariant {
            control: "control".to_string(),
            value: json!(true),
        }],
    );

    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.variant, Some("control".to_string()));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_fails_when_user_not_in_allowed_values() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "viewer")]);
    let crit = criterion(
        vec![rule("role", Operator::In, json!(["admin", "editor"]))],
        None,
        0,
    );
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = mk_feature(
        "test-id",
        "test-key",
        "Contextual",
        true,
        true,
        vec![stg],
        vec![],
    );
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Default);
}

#[test]
fn evaluate_passes_when_user_in_allowed() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion(vec![rule("role", Operator::In, json!(["admin"]))], None, 0);
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = mk_feature(
        "test-id",
        "test-key",
        "Contextual",
        true,
        true,
        vec![stg],
        vec![],
    );
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_with_variant_allocation() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion(
        vec![rule("role", Operator::In, json!(["admin"]))],
        Some("treatment"),
        0,
    );
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
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
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!("Enhanced UI"));
    assert_eq!(result.variant, Some("treatment".to_string()));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_with_json_variant() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("tier", "premium")]);
    let crit = criterion(
        vec![rule("tier", Operator::In, json!(["premium"]))],
        Some("premium-config"),
        0,
    );
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![
            FeatureVariant {
                control: "basic-config".to_string(),
                value: json!({"theme": "light", "features": ["chat"]}),
            },
            FeatureVariant {
                control: "premium-config".to_string(),
                value: json!({"theme": "dark", "features": ["chat", "video", "analytics"]}),
            },
        ],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(
        result.value,
        json!({"theme": "dark", "features": ["chat", "video", "analytics"]})
    );
    assert_eq!(result.variant, Some("premium-config".to_string()));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_dependency_failed() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let stg = stage("env-a", true, None, vec![]);

    let dependency = Feature {
        id: "dep-id".to_string(),
        key: "dep-key".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: false,
        dependencies: vec![],
        stages: vec![],
        variants: vec![],
    };

    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![dependency],
        stages: vec![stg],
        variants: vec![],
    };

    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::DependencyFailed);
}

#[test]
fn evaluate_with_custom_bucketing_key() {
    let ctx = mk_ctx(
        "feat",
        "env-a",
        "user123",
        &[("org_id", "org456"), ("role", "admin")],
    );
    let crit = criterion(
        vec![rule("role", Operator::In, json!(["admin"]))],
        Some("treatment"),
        0,
    );
    let stg = stage("env-a", true, Some("org_id"), vec![crit]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![FeatureVariant {
            control: "treatment".to_string(),
            value: json!(true),
        }],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_multiple_criteria_first_match_wins() {
    let ctx = mk_ctx(
        "feat",
        "env-a",
        "user123",
        &[("role", "admin"), ("tier", "premium")],
    );
    let crit1 = criterion(
        vec![rule("role", Operator::In, json!(["admin"]))],
        Some("admin-variant"),
        0,
    );
    let crit2 = criterion(
        vec![rule("tier", Operator::In, json!(["premium"]))],
        Some("premium-variant"),
        1,
    );
    let stg = stage("env-a", true, None, vec![crit1, crit2]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![
            FeatureVariant {
                control: "admin-variant".to_string(),
                value: json!("Admin Experience"),
            },
            FeatureVariant {
                control: "premium-variant".to_string(),
                value: json!("Premium Experience"),
            },
        ],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    // First matching criterion wins
    assert_eq!(result.variant, Some("admin-variant".to_string()));
    assert_eq!(result.value, json!("Admin Experience"));
}

#[test]
fn evaluate_missing_bucketing_key_attribute() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    // Stage expects "org_id" but it's not provided
    let crit = criterion(
        vec![rule("role", Operator::In, json!(["admin"]))],
        Some("treatment"),
        0,
    );
    let stg = stage("env-a", true, Some("org_id"), vec![crit]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![FeatureVariant {
            control: "treatment".to_string(),
            value: json!(true),
        }],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Default);
}

#[test]
fn evaluate_variant_not_found_returns_default() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion(
        vec![rule("role", Operator::In, json!(["admin"]))],
        Some("non-existent-variant"),
        0,
    );
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
        id: "test-id".to_string(),
        key: "test-key".to_string(),
        feature_type: "Contextual".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![FeatureVariant {
            control: "control".to_string(),
            value: json!(false),
        }],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    // When variant not found, returns default true value
    assert_eq!(result.value, json!(true));
    assert_eq!(result.variant, Some("non-existent-variant".to_string()));
}
