use async_graphql::SimpleObject;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SENTINEL_UUID: Uuid = Uuid::nil();

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub team_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Pipeline {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    pub team_id: Uuid,
    pub stages: Vec<PipelineStage>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct PipelineStage {
    pub id: Uuid,
    pub pipeline_id: Uuid,
    pub environment_id: Uuid,
    pub order_index: i32,
    pub parent_stage_id: Option<Uuid>,
    pub position: String,
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
    pub contextual_types: Option<Vec<ContextualType>>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct ContextualType {
    pub id: Uuid,
    pub name: String,
    pub entries: Vec<ContextualEntry>,
}

#[derive(SimpleObject, Clone, Debug, Serialize, Deserialize)]
pub struct ContextualEntry {
    pub id: Uuid,
    pub value: String,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ClientType {
    Web,
    Backend,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct Client {
    pub id: Uuid,
    pub team_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub client_type: ClientType,
    pub api_key: String,
    pub web_origins: Option<Vec<String>>, // Populated when loading with joins
}

pub trait DBStage: Send + Sync {
    fn get_id(&self) -> Uuid;
    fn order_index(&self) -> i32;
    fn parent_stage_id(&self) -> Option<Uuid>;
    fn environment_id(&self) -> Uuid;
    fn position(&self) -> String;
    fn enabled(&self) -> bool;
}

impl DBStage for PipelineStage {
    fn get_id(&self) -> Uuid {
        self.id
    }

    fn order_index(&self) -> i32 {
        self.order_index
    }

    fn parent_stage_id(&self) -> Option<Uuid> {
        self.parent_stage_id
    }

    fn environment_id(&self) -> Uuid {
        self.environment_id
    }

    fn position(&self) -> String {
        self.position.clone()
    }

    fn enabled(&self) -> bool {
        true // Pipeline stages are always enabled
    }
}

impl DBStage for FeaturePipelineStage {
    fn get_id(&self) -> Uuid {
        self.id
    }

    fn order_index(&self) -> i32 {
        self.order_index
    }

    fn parent_stage_id(&self) -> Option<Uuid> {
        self.parent_stage_id
    }

    fn environment_id(&self) -> Uuid {
        self.environment_id
    }

    fn position(&self) -> String {
        self.position.clone()
    }

    fn enabled(&self) -> bool {
        self.enabled
    }
}
