use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Clone)]
pub struct FeatureEvaluationContext {
    pub feature: String,
    pub environment_id: String,
    pub context: Vec<Context>,
}

#[derive(Deserialize, Clone)]
pub struct Context {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Feature {
    pub enabled: bool,
    pub dependencies: Vec<Feature>,
    pub stages: Vec<FeatureStage>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FeatureStage {
    pub environment_id: String,
    pub enabled: bool,
    pub bucketing_key: Option<String>,
    pub criterias: Vec<StageCriterion>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StageContext {
    pub key: String,
    pub entries: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StageCriterion {
    pub context_key: String,
    pub context: StageContext,
    pub rollout_percentage: i32,
}

fn get_context_value<'a>(ctx: &'a FeatureEvaluationContext, key: &str) -> Option<&'a str> {
    ctx.context
        .iter()
        .find(|c| c.key == key)
        .map(|c| c.value.as_str())
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

fn passes_stage_criteria(ec: &FeatureEvaluationContext, stage: &FeatureStage) -> bool {
    // If no criteria defined, treat as pass-through (stage gating only)
    if stage.criterias.is_empty() {
        return true;
    }
    // Determine sticky key
    let sticky_key = stage.bucketing_key.as_deref().unwrap_or("user.id");
    let sticky_val = get_context_value(ec, sticky_key).unwrap_or("");
    if sticky_val.is_empty() {
        // No bucketing identity; conservatively do not enable
        return false;
    }

    // Precompute user bucket percentage
    let mut hasher = Sha256::new();
    hasher.update(ec.feature.as_bytes());
    hasher.update(b":");
    hasher.update(sticky_val.as_bytes());
    let digest = hasher.finalize();
    let user_bucket = hash_to_percentage(&digest); // 0..100

    for crit in &stage.criterias {
        // Find provided value for the actual context key stored in the context
        let ctx_key = &crit.context_key;
        if let Some(provided) = get_context_value(ec, ctx_key) {
            // Check allowed values
            if crit.context.entries.iter().any(|v| v == provided) {
                let pct = crit.rollout_percentage.clamp(0, 100) as f32;
                if user_bucket < pct {
                    return true; // user is within rollout for a matching criterion
                }
            }
        }
    }

    false
}

pub fn evaluate(evaluation_context: FeatureEvaluationContext, feature: Feature) -> bool {
    // Kill switch check: if kill_switch_enabled is false, feature is disabled
    if !feature.enabled {
        return false;
    }

    // All dependencies must evaluate to true
    for dependency in feature.dependencies.clone() {
        if !evaluate(evaluation_context.clone(), dependency) {
            return false;
        }
    }

    // There must be a stage matching the environment_id and it must be enabled
    let stage = match feature
        .stages
        .iter()
        .find(|stage| stage.environment_id == evaluation_context.environment_id)
    {
        None => return false,
        Some(stage) => stage,
    };

    // Stage must be enabled
    if !stage.enabled {
        return false;
    }

    // Stage criteria must pass
    if !passes_stage_criteria(&evaluation_context, stage) {
        return false;
    }

    true
}
