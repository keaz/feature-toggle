use crate::database::{Error, handle_error};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct RolloutTemplateRow {
    pub id: Uuid,
    pub team_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub template_type: String,
    pub config: serde_json::Value,
    pub created_by: Option<Uuid>,
    pub is_system: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateRolloutTemplate {
    pub team_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub template_type: String,
    pub config: serde_json::Value,
    pub created_by: Option<Uuid>,
}

#[derive(Clone)]
pub struct RolloutTemplateRepository {
    pool: sqlx::PgPool,
}

impl RolloutTemplateRepository {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_custom_for_team(
        &self,
        team_id: Uuid,
    ) -> Result<Vec<RolloutTemplateRow>, Error> {
        let result = sqlx::query_as::<_, RolloutTemplateRow>(
            r#"
            SELECT id, team_id, name, description, template_type, config, created_by,
                   is_system, created_at, updated_at
            FROM rollout_templates
            WHERE team_id = $1 AND is_system = FALSE
            ORDER BY lower(name), created_at DESC
            "#,
        )
        .bind(team_id)
        .fetch_all(&self.pool)
        .await;

        handle_error(None, result)
    }

    pub async fn get_custom_for_team(
        &self,
        template_id: Uuid,
        team_id: Uuid,
    ) -> Result<RolloutTemplateRow, Error> {
        let result = sqlx::query_as::<_, RolloutTemplateRow>(
            r#"
            SELECT id, team_id, name, description, template_type, config, created_by,
                   is_system, created_at, updated_at
            FROM rollout_templates
            WHERE id = $1 AND team_id = $2 AND is_system = FALSE
            "#,
        )
        .bind(template_id)
        .bind(team_id)
        .fetch_one(&self.pool)
        .await;

        handle_error(Some(template_id), result)
    }

    pub async fn create_custom_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateRolloutTemplate,
    ) -> Result<RolloutTemplateRow, Error> {
        let result = sqlx::query_as::<_, RolloutTemplateRow>(
            r#"
            INSERT INTO rollout_templates (
                team_id, name, description, template_type, config, created_by, is_system
            )
            VALUES ($1, $2, $3, $4, $5, $6, FALSE)
            RETURNING id, team_id, name, description, template_type, config, created_by,
                      is_system, created_at, updated_at
            "#,
        )
        .bind(input.team_id)
        .bind(input.name)
        .bind(input.description)
        .bind(input.template_type)
        .bind(input.config)
        .bind(input.created_by)
        .fetch_one(&mut *conn)
        .await;

        handle_error(None, result)
    }
}

pub fn rollout_template_repository(pool: sqlx::PgPool) -> RolloutTemplateRepository {
    RolloutTemplateRepository::new(pool)
}
