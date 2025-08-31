use evaluation_engine::{
    Context, Feature, FeatureEvaluationContext, FeatureStage, StageContext, StageCriterion,
};

fn mk_ctx(feature: &str, env: &str, pairs: &[(&str, &str)]) -> FeatureEvaluationContext {
    FeatureEvaluationContext {
        feature: feature.to_string(),
        environment_id: env.to_string(),
        context: pairs
            .iter()
            .map(|(k, v)| Context {
                key: (*k).into(),
                value: (*v).into(),
            })
            .collect(),
    }
}

fn stage(
    env: &str,
    status: &str,
    bucketing: Option<&str>,
    criterias: Vec<StageCriterion>,
) -> FeatureStage {
    FeatureStage {
        environment_id: env.to_string(),
        status: status.to_string(),
        bucketing_key: bucketing.map(|s| s.to_string()),
        criterias,
    }
}

fn criterion(context_key: &str, allowed: &[&str], pct: i32) -> StageCriterion {
    StageCriterion {
        context_key: context_key.to_string(),
        context: StageContext {
            key: context_key.to_string(),
            entries: allowed.iter().map(|s| (*s).into()).collect(),
        },
        rollout_percentage: pct,
    }
}

#[test]
fn evaluate_returns_false_when_feature_disabled() {
    let ctx = mk_ctx("feat", "env-a", &[]);
    let feature = Feature {
        enabled: false,
        dependencies: vec![],
        stages: vec![],
    };
    assert!(!evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_requires_matching_environment_stage() {
    let ctx = mk_ctx("feat", "env-a", &[]);
    let stg = stage("env-b", "DEPLOYED", None, vec![]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
    };
    assert!(!evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_requires_stage_enabled() {
    let ctx = mk_ctx("feat", "env-a", &[]);
    let stg = stage("env-a", "NOT_DEPLOYED", None, vec![]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
    };
    assert!(!evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_passes_when_no_criteria_and_enabled_stage() {
    let ctx = mk_ctx("feat", "env-a", &[]);
    let stg = stage("env-a", "DEPLOYED", None, vec![]);
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
    };
    assert!(evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_fails_without_bucketing_identity_when_criteria_present() {
    // criterias present but no user.id in context; default bucketing key is user.id
    let ctx = mk_ctx("feat", "env-a", &[("irrelevant", "x")]);
    let stg = stage(
        "env-a",
        "DEPLOYED",
        None,
        vec![criterion("country", &["US"], 100)],
    );
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
    };
    assert!(!evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_respects_custom_bucketing_key() {
    // Provide custom bucketing key and value in context
    let ctx = mk_ctx("feat", "env-a", &[("userId", "alice"), ("country", "US")]);
    let stg = stage(
        "env-a",
        "DEPLOYED",
        Some("userId"),
        vec![criterion("country", &["US"], 100)],
    );
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
    };
    assert!(evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_respects_rollout_percentage_thresholds() {
    // Use a deterministic sticky value so hash bucket is stable; try two identities and assert
    let mk = |user: &str| mk_ctx("my-feature", "prod", &[("user.id", user), ("segment", "A")]);
    let stg = |pct: i32| stage("prod", "DEPLOYED", None, vec![criterion("segment", &["A"], pct)]);

    // With 0% rollout nobody should pass
    let f0 = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg(0)],
    };
    assert!(!evaluation_engine::evaluate(mk("user-1"), f0.clone()));
    assert!(!evaluation_engine::evaluate(mk("user-2"), f0));

    // With 100% rollout everybody with matching criteria passes
    let f100 = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg(100)],
    };
    assert!(evaluation_engine::evaluate(mk("user-1"), f100.clone()));
    assert!(evaluation_engine::evaluate(mk("user-2"), f100));
}

#[test]
fn evaluate_requires_matching_context_value() {
    let ctx = mk_ctx("feat", "env-a", &[("user.id", "bob"), ("country", "UK")]);
    let stg = stage(
        "env-a",
        "DEPLOYED",
        None,
        vec![criterion("country", &["US", "CA"], 100)],
    );
    let feature = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stg],
    };
    assert!(!evaluation_engine::evaluate(ctx, feature));
}

#[test]
fn evaluate_all_dependencies_must_pass() {
    // Build a dependency tree: root depends on dep1 (true) and dep2 (false) => overall false
    let ctx = mk_ctx("root", "env", &[("user.id", "u"), ("ctx", "x")]);

    let dep1 = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stage("env", "DEPLOYED", None, vec![])],
    };
    let dep2 = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stage("env", "NOT_DEPLOYED", None, vec![])],
    };

    let root = Feature {
        enabled: true,
        dependencies: vec![dep1, dep2],
        stages: vec![stage("env", "DEPLOYED", None, vec![])],
    };

    assert!(!evaluation_engine::evaluate(ctx, root));
}

#[test]
fn evaluate_nested_dependencies_true() {
    let ctx = mk_ctx("root", "env", &[("user.id", "id1")]);
    // dep inner chain that all pass
    let leaf = Feature {
        enabled: true,
        dependencies: vec![],
        stages: vec![stage("env", "DEPLOYED", None, vec![])],
    };
    let mid = Feature {
        enabled: true,
        dependencies: vec![leaf],
        stages: vec![stage("env", "DEPLOYED", None, vec![])],
    };
    let root = Feature {
        enabled: true,
        dependencies: vec![mid],
        stages: vec![stage("env", "DEPLOYED", None, vec![])],
    };
    assert!(evaluation_engine::evaluate(ctx, root));
}
