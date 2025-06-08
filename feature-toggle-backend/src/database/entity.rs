use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Stage {
    pub id: Uuid,
    pub pipeline_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i8,
    pub parent_stage_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Feature {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub feature_type: FeatureType,
    pub pipeline_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FeatureType {
    Boolean,
    Percentage,
    Contextual,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub description: String,
}