use evaluation_engine::{
    ContextObject, ErrorCode, EvaluationReason, Feature, FeatureEvaluationContext, FeatureStage,
    FeatureVariant, StageContext, StageCriterion,
};
use serde_json::json;
use std::collections::HashMap;

fn mk_ctx(flag_key: &str, env: &str, bucketing_key: &str, attrs: &[(&str, &str)]) -> FeatureEvaluationContext {
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

fn criterion(context_key: &str, allowed: &[&str], pct: i32, serve: Option<&str>) -> StageCriterion {
    StageCriterion {
        context_key: context_key.to_string(),
        context: StageContext {
            key: context_key.to_string(),
            entries: allowed.iter().map(|s| (*s).into()).collect(),
        },
        rollout_percentage: pct,
        serve: serve.map(|s| s.to_string()),
    }
}

#[test]
fn evaluate_returns_false_when_feature_disabled() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let feature = Feature {
        enabled: false,
        dependencies: vec![],
        stages: vec![],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Static);
}

#[test]
fn evaluate_requires_matching_environment_stage() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[]);
    let stg = stage("env-b", true, None, vec![]);
    let feature = Feature {
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
fn evaluate_fails_when_user_not_in_allowed_values() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "viewer")]);
    let crit = criterion("role", &["admin", "editor"], 100, None);
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Default);
}

#[test]
fn evaluate_passes_when_user_in_allowed_and_rollout_100() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion("role", &["admin"], 100, None);
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_with_variant() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion("role", &["admin"], 100, Some("treatment"));
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
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
    let crit = criterion("tier", &["premium"], 100, Some("premium-config"));
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
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
        enabled: false,
        dependencies: vec![],
        stages: vec![],
        variants: vec![],
    };

    let feature = Feature {
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
    let ctx = mk_ctx("feat", "env-a", "user123", &[("org_id", "org456"), ("role", "admin")]);
    let crit = criterion("role", &["admin"], 100, None);
    let stg = stage("env-a", true, Some("org_id"), vec![crit]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(true));
    assert_eq!(result.reason, EvaluationReason::TargetingMatch);
}

#[test]
fn evaluate_multiple_criteria_first_match_wins() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin"), ("tier", "premium")]);
    let crit1 = criterion("role", &["admin"], 100, Some("admin-variant"));
    let crit2 = criterion("tier", &["premium"], 100, Some("premium-variant"));
    let stg = stage("env-a", true, None, vec![crit1, crit2]);
    let feature = Feature {
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
    let crit = criterion("role", &["admin"], 100, None);
    let stg = stage("env-a", true, Some("org_id"), vec![crit]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    assert_eq!(result.value, json!(false));
    assert_eq!(result.reason, EvaluationReason::Default);
}

#[test]
fn evaluate_variant_not_found_returns_default() {
    let ctx = mk_ctx("feat", "env-a", "user123", &[("role", "admin")]);
    let crit = criterion("role", &["admin"], 100, Some("non-existent-variant"));
    let stg = stage("env-a", true, None, vec![crit]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
        variants: vec![
            FeatureVariant {
                control: "control".to_string(),
                value: json!(false),
            },
        ],
    };
    let result = evaluation_engine::evaluate(ctx, feature);
    // When variant not found, returns default true value
    assert_eq!(result.value, json!(true));
    assert_eq!(result.variant, Some("non-existent-variant".to_string()));
}
