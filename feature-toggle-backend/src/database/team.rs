use crate::database::entity::Team;
use crate::database::{Error, handle_error};
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

pub struct CreateTeam {
    pub name: String,
    pub description: String,
}

pub struct UpdateTeam {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// Repository trait for team operations.
///
/// Standard methods are automocked for unit testing.
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait TeamRepository: Send + Sync {
    async fn get_team_by_id(&self, id: Uuid) -> Result<Team, Error>;
    async fn get_teams(&self, name: Option<String>) -> Result<Vec<Team>, Error>;
    async fn create_team(&self, input: CreateTeam) -> Result<Team, Error>;
    async fn update_team(&self, input: UpdateTeam) -> Result<Team, Error>;
    async fn delete_team(&self, id: Uuid) -> Result<(), Error>;
    fn clone_box(&self) -> Box<dyn TeamRepository>;
}

impl Clone for Box<dyn TeamRepository> {
    fn clone(&self) -> Box<dyn TeamRepository> {
        self.clone_box()
    }
}

/// Extension trait for transaction-aware repository operations.
/// These methods accept a mutable connection reference for use within transactions.
#[async_trait::async_trait]
pub trait TeamRepositoryTx: TeamRepository {
    async fn get_team_by_id_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<Team, Error>;
    async fn create_team_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateTeam,
    ) -> Result<Team, Error>;
    async fn update_team_tx(
        &self,
        conn: &mut PgConnection,
        input: UpdateTeam,
    ) -> Result<Team, Error>;
    async fn delete_team_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error>;
}

pub fn team_repository(pool: PgPool) -> Box<dyn TeamRepository> {
    Box::new(TeamRepositoryImpl::new(pool))
}

/// Returns a repository that also implements TeamRepositoryTx for transaction support.
pub fn team_repository_tx(pool: PgPool) -> TeamRepositoryImpl {
    TeamRepositoryImpl::new(pool)
}

#[derive(Clone)]
pub struct TeamRepositoryImpl {
    pool: PgPool,
}

impl TeamRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn get_team_by_id_internal(conn: &mut PgConnection, id: Uuid) -> Result<Team, Error> {
        let result =
            sqlx::query_as::<_, Team>(r#"SELECT id, name, description FROM teams WHERE id = $1"#)
                .bind(id)
                .fetch_one(&mut *conn)
                .await;

        handle_error(Some(id), result)
    }

    async fn create_team_internal(
        conn: &mut PgConnection,
        input: CreateTeam,
    ) -> Result<Team, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
            r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3) RETURNING id, name, description"#,
            id,
            input.name,
            input.description
        )
        .fetch_one(&mut *conn)
        .await;

        let handled_error = handle_error(None, result)?;
        Ok(Team {
            id: handled_error.id,
            name: handled_error.name,
            description: handled_error.description,
        })
    }

    async fn update_team_internal(
        conn: &mut PgConnection,
        input: UpdateTeam,
        existing: Team,
    ) -> Result<Team, Error> {
        let result = sqlx::query!(
            r#"UPDATE teams SET name = $1, description = $2 WHERE id = $3 RETURNING id, name, description"#,
            input.name.unwrap_or(existing.name),
            input.description.unwrap_or(existing.description),
            input.id
        )
        .fetch_one(&mut *conn)
        .await;

        let team = handle_error(Some(input.id), result)?;

        Ok(Team {
            id: team.id,
            name: team.name,
            description: team.description,
        })
    }

    async fn delete_team_internal(conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        let result = sqlx::query!("DELETE FROM teams WHERE id = $1", id)
            .execute(&mut *conn)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl TeamRepository for TeamRepositoryImpl {
    async fn get_team_by_id(&self, id: Uuid) -> Result<Team, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::get_team_by_id_internal(&mut conn, id).await
    }

    async fn get_teams(&self, name: Option<String>) -> Result<Vec<Team>, Error> {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT id, name, description FROM teams");

        if let Some(filter_name) = name {
            qb.push(" WHERE ");
            let pattern = format!("%{filter_name}%");
            qb.push("name ILIKE ").push_bind(pattern);
        }
        qb.push(" ORDER BY name");

        let query = qb.build_query_as::<Team>();
        let result = query.fetch_all(&self.pool).await;

        let teams = handle_error(None, result)?;
        Ok(teams)
    }

    async fn create_team(&self, input: CreateTeam) -> Result<Team, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::create_team_internal(&mut conn, input).await
    }

    async fn update_team(&self, input: UpdateTeam) -> Result<Team, Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        let existing = Self::get_team_by_id_internal(&mut conn, input.id).await?;
        Self::update_team_internal(&mut conn, input, existing).await
    }

    async fn delete_team(&self, id: Uuid) -> Result<(), Error> {
        let mut conn = self.pool.acquire().await.map_err(Error::DatabaseError)?;
        Self::get_team_by_id_internal(&mut conn, id).await?;
        Self::delete_team_internal(&mut conn, id).await
    }

    fn clone_box(&self) -> Box<dyn TeamRepository> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl TeamRepositoryTx for TeamRepositoryImpl {
    async fn get_team_by_id_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<Team, Error> {
        Self::get_team_by_id_internal(conn, id).await
    }

    async fn create_team_tx(
        &self,
        conn: &mut PgConnection,
        input: CreateTeam,
    ) -> Result<Team, Error> {
        Self::create_team_internal(conn, input).await
    }

    async fn update_team_tx(
        &self,
        conn: &mut PgConnection,
        input: UpdateTeam,
    ) -> Result<Team, Error> {
        let existing = Self::get_team_by_id_internal(conn, input.id).await?;
        Self::update_team_internal(conn, input, existing).await
    }

    async fn delete_team_tx(&self, conn: &mut PgConnection, id: Uuid) -> Result<(), Error> {
        Self::get_team_by_id_internal(conn, id).await?;
        Self::delete_team_internal(conn, id).await
    }
}
