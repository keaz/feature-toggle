use crate::database::Error;
use crate::database::handle_error;
use mockall::automock;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserFlagAssignmentRow {
    pub user_id: String,
    pub feature_id: Uuid,
    pub environment_id: Uuid,
    pub assigned: bool,
    pub variant: Option<String>,
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
        variant: Option<String>,
    ) -> Result<(), Error>;

    /// Upserts all rows in one statement. Keys `(user_id, feature_id,
    /// environment_id)` must be unique within `rows`; Postgres rejects an
    /// `ON CONFLICT DO UPDATE` that touches the same row twice.
    async fn upsert_many(&self, rows: &[UserFlagAssignmentRow]) -> Result<(), Error>;

    /// Returns true only when every feature and every environment in the given
    /// lists exists and belongs to `team_id`. Duplicate ids are allowed.
    async fn all_owned_by_team(
        &self,
        team_id: Uuid,
        feature_ids: &[Uuid],
        environment_ids: &[Uuid],
    ) -> Result<bool, Error>;

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
        variant: Option<String>,
    ) -> Result<(), Error> {
        let res = sqlx::query(
            r#"INSERT INTO user_flag_assignments (user_id, feature_id, environment_id, assigned, variant)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (user_id, feature_id, environment_id)
               DO UPDATE SET assigned = EXCLUDED.assigned, variant = EXCLUDED.variant, assigned_at = now()"#,
        )
        .bind(user_id)
        .bind(feature_id)
        .bind(environment_id)
        .bind(assigned)
        .bind(variant.as_deref())
        .execute(&self.pool)
        .await;

        handle_error(None, res).map(|_| ())
    }

    async fn upsert_many(&self, rows: &[UserFlagAssignmentRow]) -> Result<(), Error> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut user_ids = Vec::with_capacity(rows.len());
        let mut feature_ids = Vec::with_capacity(rows.len());
        let mut environment_ids = Vec::with_capacity(rows.len());
        let mut assigned = Vec::with_capacity(rows.len());
        let mut variants: Vec<Option<String>> = Vec::with_capacity(rows.len());
        for row in rows {
            user_ids.push(row.user_id.clone());
            feature_ids.push(row.feature_id);
            environment_ids.push(row.environment_id);
            assigned.push(row.assigned);
            variants.push(row.variant.clone());
        }

        let res = sqlx::query(
            r#"INSERT INTO user_flag_assignments (user_id, feature_id, environment_id, assigned, variant)
               SELECT u.user_id, u.feature_id, u.environment_id, u.assigned, u.variant
               FROM UNNEST($1::text[], $2::uuid[], $3::uuid[], $4::bool[], $5::text[])
                    AS u(user_id, feature_id, environment_id, assigned, variant)
               ON CONFLICT (user_id, feature_id, environment_id)
               DO UPDATE SET assigned = EXCLUDED.assigned, variant = EXCLUDED.variant, assigned_at = now()"#,
        )
        .bind(&user_ids)
        .bind(&feature_ids)
        .bind(&environment_ids)
        .bind(&assigned)
        .bind(&variants)
        .execute(&self.pool)
        .await;

        handle_error(None, res).map(|_| ())
    }

    async fn all_owned_by_team(
        &self,
        team_id: Uuid,
        feature_ids: &[Uuid],
        environment_ids: &[Uuid],
    ) -> Result<bool, Error> {
        let mut feature_ids = feature_ids.to_vec();
        feature_ids.sort_unstable();
        feature_ids.dedup();
        let mut environment_ids = environment_ids.to_vec();
        environment_ids.sort_unstable();
        environment_ids.dedup();

        let res = sqlx::query_scalar::<_, bool>(
            r#"SELECT
                   (SELECT COUNT(*) FROM features WHERE team_id = $1 AND id = ANY($2)) = cardinality($2::uuid[])
               AND (SELECT COUNT(*) FROM environments WHERE team_id = $1 AND id = ANY($3)) = cardinality($3::uuid[])"#,
        )
        .bind(team_id)
        .bind(&feature_ids)
        .bind(&environment_ids)
        .fetch_one(&self.pool)
        .await;

        handle_error(None, res)
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
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned, ufa.variant
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
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned, ufa.variant
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
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned, ufa.variant
                       FROM user_flag_assignments ufa
                       JOIN features f ON f.id = ufa.feature_id
                       WHERE f.team_id = $1 AND ufa.environment_id = $2 AND EXISTS (
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
                    r#"SELECT ufa.user_id, ufa.feature_id, ufa.environment_id, ufa.assigned, ufa.variant
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
