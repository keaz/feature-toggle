use crate::database::handle_error;
use crate::database::Error;
use mockall::automock;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserFlagAssignmentRow {
    pub user_id: String,
    pub feature_id: Uuid,
    pub environment_id: Uuid,
    pub assigned: bool,
}

#[automock]
#[async_trait::async_trait]
pub trait UserFlagAssignmentRepository: Send + Sync {
    async fn upsert(
        &self,
        user_id: &str,
        feature_id: Uuid,
        environment_id: Uuid,
        assigned: bool,
    ) -> Result<(), Error>;

    async fn list(
        &self,
        team_id: Uuid,
        feature_id: Option<Uuid>,
        environment_id: Option<Uuid>,
    ) -> Result<Vec<UserFlagAssignmentRow>, Error>;

    fn clone_box(&self) -> Box<dyn UserFlagAssignmentRepository>;
}

impl Clone for Box<dyn UserFlagAssignmentRepository> {
    fn clone(&self) -> Box<dyn UserFlagAssignmentRepository> {
        self.clone_box()
    }
}

pub fn user_flag_assignment_repository(pool: PgPool) -> Box<dyn UserFlagAssignmentRepository> {
    Box::new(UserFlagAssignmentRepositoryImpl::new(pool))
}

struct UserFlagAssignmentRepositoryImpl {
    pool: PgPool,
}

impl UserFlagAssignmentRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserFlagAssignmentRepository for UserFlagAssignmentRepositoryImpl {
    async fn upsert(
        &self,
        user_id: &str,
        feature_id: Uuid,
        environment_id: Uuid,
        assigned: bool,
    ) -> Result<(), Error> {
        let res = sqlx::query(
            r#"INSERT INTO user_flag_assignments (user_id, feature_id, environment_id, assigned)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (user_id, feature_id, environment_id)
               DO UPDATE SET assigned = EXCLUDED.assigned, assigned_at = now()"#,
        )
        .bind(user_id)
        .bind(feature_id)
        .bind(environment_id)
        .bind(assigned)
        .execute(&self.pool)
        .await;

        handle_error(None, res).map(|_| ())
    }

    async fn list(
        &self,
        team_id: Uuid,
        feature_id: Option<Uuid>,
        environment_id: Option<Uuid>,
    ) -> Result<Vec<UserFlagAssignmentRow>, Error> {
        let out = match (feature_id, environment_id) {
            (Some(fid), Some(eid)) => {
                let res = sqlx::query_as!(
                    UserFlagAssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND ufa.feature_id = $2 AND ufa.environment_id = $3"#,
                    team_id,
                    fid,
                    eid
                )
                .fetch_all(&self.pool)
                .await;
                handle_error(None, res)?
            }
            (Some(fid), None) => {
                let res = sqlx::query_as!(
                    UserFlagAssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND ufa.feature_id = $2"#,
                    team_id,
                    fid
                )
                .fetch_all(&self.pool)
                .await;
                handle_error(None, res)?
            }
            (None, Some(eid)) => {
                let res = sqlx::query_as!(
                    UserFlagAssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND EXISTS (
                           SELECT 1 FROM features_pipeline_stages s
                           WHERE s.feature_id = f.id AND s.environment_id = $2
                       )"#,
                    team_id,
                    eid
                )
                .fetch_all(&self.pool)
                .await;
                handle_error(None, res)?
            }
            (None, None) => {
                let res = sqlx::query_as!(
                    UserFlagAssignmentRow,
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1"#,
                    team_id
                )
                .fetch_all(&self.pool)
                .await;
                handle_error(None, res)?
            }
        };

        Ok(out)
    }

    fn clone_box(&self) -> Box<dyn UserFlagAssignmentRepository> {
        Box::new(Self::new(self.pool.clone()))
    }
}
