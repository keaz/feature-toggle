use evaluation_engine::{
    ContextObject, ErrorCode, EvaluationReason, Feature, FeatureEvaluationContext, FeatureStage,
    FeatureVariant, LogicOperator, Operator, RuleCondition, RuleGroup, StageCriterion,
    VariantAllocation, VariantSelectionMode,
};
use serde_json::json;
use std::collections::HashMap;

fn mk_ctx(
    flag_key: &str,
    env: &str,
    targeting_key: &str,
    attrs: &[(&str, &str)],
) -> FeatureEvaluationContext {
    let mut attributes = HashMap::new();
    for (k, v) in attrs {
        attributes.insert((*k).to_string(), json!(*v));
    }

    FeatureEvaluationContext {
        flag_key: flag_key.to_string(),
        context: ContextObject {
            targeting_key: targeting_key.to_string(),
            environment_id: env.to_string(),
            attributes,
        },
    }
}

fn stage(
    env: &str,
    enabled: bool,
    _bucketing: Option<&str>,
    criterias: Vec<StageCriterion>,
) -> FeatureStage {
    FeatureStage {
        environment_id: env.to_string(),
        enabled,
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
        variant_selection_mode: VariantSelectionMode::WeightedSplit,
        selected_variant_control: None,
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Unknown);
    assert_eq!(result.error_code, Some(ErrorCode::FlagNotFound));
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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

    let result = evaluation_engine::evaluate(&ctx, &feature);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Unknown);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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

    let result = evaluation_engine::evaluate(&ctx, &feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Disabled);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
    // First matching criterion wins
    assert_eq!(result.variant, Some("admin-variant".to_string()));
    assert_eq!(result.value, json!("Admin Experience"));
}

#[test]
fn evaluate_missing_bucketing_key_attribute() {
    // This test now verifies that targeting_key is always used (OpenFeature standard)
    // Previously tested custom bucketing key fallback - no longer supported
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion(
        vec![rule("role", Operator::In, json!(["admin"]))],
        Some("treatment"),
        0,
    );
    // No custom bucketing key - always uses targeting_key from context
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
            control: "treatment".to_string(),
            value: json!(true),
        }],
    };
    let result = evaluation_engine::evaluate(&ctx, &feature);
    // Should succeed because targeting_key is always available
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
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
    let result = evaluation_engine::evaluate(&ctx, &feature);
    // When variant not found, returns default true value
    assert_eq!(result.value, json!(true));
    assert_eq!(result.variant, Some("non-existent-variant".to_string()));
}

#[test]
fn evaluate_dependency_block_includes_reason_details() {
    let ctx = mk_ctx("feature-root", "env-a", "user123", &[]);

    let blocked_dependency = Feature {
        id: "dep-1".to_string(),
        key: "feature-dependency".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: false,
        dependencies: vec![],
        stages: vec![],
        variants: vec![],
    };

    let root = Feature {
        id: "root-1".to_string(),
        key: "feature-root".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![blocked_dependency],
        stages: vec![stage("env-a", true, None, vec![])],
        variants: vec![],
    };

    let result = evaluation_engine::evaluate(&ctx, &root);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Disabled);

    let metadata = result.metadata.expect("expected dependency block metadata");
    let dependency_block = metadata
        .get("dependencyBlock")
        .expect("missing dependencyBlock metadata");

    assert_eq!(
        dependency_block
            .get("dependencyKey")
            .and_then(|value| value.as_str()),
        Some("feature-dependency")
    );
    assert_eq!(
        dependency_block
            .get("code")
            .and_then(|value| value.as_str()),
        Some("DEPENDENCY_DISABLED")
    );
}

#[test]
fn evaluate_allowed_dependency_chain_returns_true() {
    let ctx = mk_ctx("feature-root", "env-a", "user123", &[]);

    let feature_c = Feature {
        id: "feature-c".to_string(),
        key: "feature-c".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stage("env-a", true, None, vec![])],
        variants: vec![],
    };

    let feature_b = Feature {
        id: "feature-b".to_string(),
        key: "feature-b".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![feature_c],
        stages: vec![stage("env-a", true, None, vec![])],
        variants: vec![],
    };

    let feature_a = Feature {
        id: "feature-a".to_string(),
        key: "feature-root".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![feature_b],
        stages: vec![stage("env-a", true, None, vec![])],
        variants: vec![],
    };

    let result = evaluation_engine::evaluate(&ctx, &feature_a);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::Static);
    assert!(result.metadata.is_none());
}

#[test]
fn evaluate_dependency_cycle_is_detected_and_blocked() {
    let ctx = mk_ctx("feature-a", "env-a", "user123", &[]);

    let self_dependency = Feature {
        id: "feature-a".to_string(),
        key: "feature-a".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![],
        stages: vec![stage("env-a", true, None, vec![])],
        variants: vec![],
    };

    let feature_a = Feature {
        id: "feature-a".to_string(),
        key: "feature-a".to_string(),
        feature_type: "Simple".to_string(),
        active: true,
        enabled: true,
        dependencies: vec![self_dependency],
        stages: vec![stage("env-a", true, None, vec![])],
        variants: vec![],
    };

    let result = evaluation_engine::evaluate(&ctx, &feature_a);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Disabled);

    let metadata = result.metadata.expect("expected cycle metadata");
    let dependency_block = metadata
        .get("dependencyBlock")
        .expect("missing dependencyBlock metadata");
    assert_eq!(
        dependency_block
            .get("code")
            .and_then(|value| value.as_str()),
        Some("DEPENDENCY_CYCLE_DETECTED")
    );
    assert!(
        dependency_block.get("cyclePath").is_some(),
        "cyclePath should be included for cycle errors"
    );
}

// ============================================
// Dependency bucketing (B13)
// ============================================

fn weighted_criterion(allocations: &[(&str, i32)]) -> StageCriterion {
    StageCriterion {
        priority: 0,
        rule_groups: vec![],
        variant_allocations: allocations
            .iter()
            .map(|(control, weight)| VariantAllocation {
                variant_control: (*control).to_string(),
                weight: *weight,
            })
            .collect(),
        variant_selection_mode: VariantSelectionMode::WeightedSplit,
        selected_variant_control: None,
    }
}

fn variant(control: &str, value: serde_json::Value) -> FeatureVariant {
    FeatureVariant {
        control: control.to_string(),
        value,
    }
}

/// D: a 50/50 boolean split ({off: false, on: true}).
fn weighted_boolean_dependency() -> Feature {
    mk_feature(
        "dep-d",
        "flag-d",
        "Contextual",
        true,
        true,
        vec![stage(
            "env-a",
            true,
            None,
            vec![weighted_criterion(&[("off", 50), ("on", 50)])],
        )],
        vec![variant("off", json!(false)), variant("on", json!(true))],
    )
}

#[test]
fn dependency_is_bucketed_with_its_own_key() {
    let dependency = weighted_boolean_dependency();
    let mut dependent = mk_feature(
        "dep-f",
        "flag-f",
        "Contextual",
        true,
        true,
        vec![stage("env-a", true, None, vec![])],
        vec![],
    );
    dependent.dependencies = vec![dependency.clone()];

    let mut dependent_true = 0;
    for i in 0..1000 {
        let targeting_key = format!("user-{i}");
        let f_result = evaluation_engine::evaluate(
            &mk_ctx("flag-f", "env-a", &targeting_key, &[]),
            &dependent,
        );
        let d_result = evaluation_engine::evaluate(
            &mk_ctx("flag-d", "env-a", &targeting_key, &[]),
            &dependency,
        );

        // F has no criteria of its own, so F is true exactly when D passes.
        assert_eq!(
            f_result.value, d_result.value,
            "dependency result inside F must equal evaluating D directly for {targeting_key}"
        );
        if f_result.value == json!(true) {
            dependent_true += 1;
        }
    }

    // Sanity: the 50/50 split actually splits the population.
    assert!(
        (350..=650).contains(&dependent_true),
        "expected roughly half of users to pass D, got {dependent_true}"
    );
}

#[test]
fn dependent_weighted_variants_are_independent_of_dependency_bucket() {
    let dependency = weighted_boolean_dependency();
    let mut dependent = mk_feature(
        "dep-f",
        "flag-f",
        "Contextual",
        true,
        true,
        vec![stage(
            "env-a",
            true,
            None,
            vec![weighted_criterion(&[("a", 50), ("b", 50)])],
        )],
        vec![variant("a", json!(true)), variant("b", json!(true))],
    );
    dependent.dependencies = vec![dependency];

    let mut served = HashMap::new();
    for i in 0..1000 {
        let targeting_key = format!("user-{i}");
        let result = evaluation_engine::evaluate(
            &mk_ctx("flag-f", "env-a", &targeting_key, &[]),
            &dependent,
        );
        if let Some(variant) = result.variant {
            *served.entry(variant).or_insert(0) += 1;
        }
    }

    for expected in ["a", "b"] {
        let count = served.get(expected).copied().unwrap_or(0);
        assert!(
            count >= 100,
            "variant {expected} should be served to a fair share of users, got {count} ({served:?})"
        );
    }
}

/// Golden values recorded before the B13 change. A flag evaluated directly
/// (the root of an evaluation) must keep bucketing on `flagKey:targetingKey`
/// exactly as before, with or without dependencies.
#[test]
fn root_bucketing_is_unchanged() {
    let allocations = [("v1", 25), ("v2", 25), ("v3", 50)];
    let standalone = mk_feature(
        "root-id",
        "checkout-redesign",
        "Contextual",
        true,
        true,
        vec![stage(
            "env-a",
            true,
            None,
            vec![weighted_criterion(&allocations)],
        )],
        vec![
            variant("v1", json!("one")),
            variant("v2", json!("two")),
            variant("v3", json!("three")),
        ],
    );

    // Same flag, now with an always-true dependency that has its own split.
    let always_true_dependency = mk_feature(
        "always-id",
        "always-on",
        "Contextual",
        true,
        true,
        vec![stage(
            "env-a",
            true,
            None,
            vec![weighted_criterion(&[("x", 50), ("y", 50)])],
        )],
        vec![variant("x", json!(true)), variant("y", json!(true))],
    );
    let mut with_dependency = standalone.clone();
    with_dependency.dependencies = vec![always_true_dependency];

    for (targeting_key, expected_variant) in GOLDEN_ROOT_VARIANTS {
        let ctx = mk_ctx("checkout-redesign", "env-a", targeting_key, &[]);
        for feature in [&standalone, &with_dependency] {
            let result = evaluation_engine::evaluate(&ctx, feature);
            assert_eq!(
                result.variant.as_deref(),
                Some(*expected_variant),
                "root bucketing changed for {targeting_key}"
            );
            assert_eq!(result.flag_key, "checkout-redesign");
        }
    }
}

const GOLDEN_ROOT_VARIANTS: &[(&str, &str)] = &[
    ("user-1", "v3"),
    ("user-2", "v2"),
    ("user-3", "v3"),
    ("user-4", "v3"),
    ("user-5", "v1"),
    ("user-6", "v3"),
    ("user-7", "v1"),
    ("user-8", "v3"),
    ("alice", "v3"),
    ("bob", "v3"),
    ("carol", "v1"),
    ("dave", "v3"),
    ("3f2c9a10-7b1e-4c55-9d2e-0a8b1c2d3e4f", "v2"),
    ("org-42:user-7", "v1"),
];

// ============================================
// Non-boolean dependencies (B19)
// ============================================

/// A dependency passes only when it evaluates to the JSON boolean `true`.
/// A dependency that serves a string (or any other non-boolean) value blocks
/// its dependents with `DEPENDENCY_EVALUATION_FAILED`. The backend rejects such
/// configurations on save; this pins the engine side of that contract.
#[test]
fn non_boolean_dependency_blocks_dependent() {
    let string_dependency = mk_feature(
        "dep-theme",
        "theme",
        "Contextual",
        true,
        true,
        vec![stage(
            "env-a",
            true,
            None,
            vec![criterion(vec![], Some("dark"), 0)],
        )],
        vec![
            variant("dark", json!("dark")),
            variant("light", json!("light")),
        ],
    );
    let mut dependent = mk_feature(
        "dep-f",
        "flag-f",
        "Simple",
        true,
        true,
        vec![stage("env-a", true, None, vec![])],
        vec![],
    );
    dependent.dependencies = vec![string_dependency.clone()];

    let direct = evaluation_engine::evaluate(
        &mk_ctx("theme", "env-a", "user123", &[]),
        &string_dependency,
    );
    assert_eq!(direct.value, json!("dark"));
    assert_eq!(direct.reason, EvaluationReason::TargetingMatch);

    let result =
        evaluation_engine::evaluate(&mk_ctx("flag-f", "env-a", "user123", &[]), &dependent);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Disabled);
    let block = &result.metadata.expect("dependency block metadata")["dependencyBlock"];
    assert_eq!(block["dependencyKey"], json!("theme"));
    assert_eq!(block["code"], json!("DEPENDENCY_EVALUATION_FAILED"));
}
