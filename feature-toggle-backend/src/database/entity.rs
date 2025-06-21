use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SENTINEL_UUID: Uuid = Uuid::nil();

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Pipeline {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub team_id: Uuid,
    pub stages: Vec<Stage>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone, Copy)]
pub struct Stage {
    pub id: Uuid,
    pub pipeline_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Feature {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub team_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub stages: Vec<FeaturePipelineStage>,
    pub dependencies: Vec<FeatureDependency>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct FeaturePipelineStage {
    pub id: Uuid,
    pub feature_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage_id: Option<Uuid>,
    pub position: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct FeatureDependency {
    pub feature_id: Uuid,
    pub depends_on_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FeatureType {
    Simple,
    Contextual,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub description: String,
}
