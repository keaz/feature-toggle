use crate::database::entity::Team;
use crate::database::{handle_error, Error};
use mockall::automock;
use sqlx::{PgPool, Postgres, QueryBuilder};
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

#[automock]
#[async_trait::async_trait]
pub trait TeamRepository: Send + Sync {
    async fn get_team_by_id(&self, env_id: Uuid) -> Result<Team, Error>;
    async fn get_teams(
        &self,
        name: Option<String>,
    ) -> Result<Vec<Team>, Error>;
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

pub fn team_repository(pool: PgPool) -> Box<dyn TeamRepository> {
    Box::new(TeamRepositoryImpl::new(pool))
}

#[derive(Clone)]
struct TeamRepositoryImpl {
    pool: PgPool,
}

impl TeamRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TeamRepository for TeamRepositoryImpl {
    async fn get_team_by_id(&self, id: Uuid) -> Result<Team, Error> {
        let result = sqlx::query_as::<_, Team>(
            r#"SELECT id, name, description FROM teams WHERE id = $1"#,
        )
            .bind(id)
            .fetch_one(&self.pool)
            .await;

        handle_error(Some(id), result)
    }

    async fn get_teams(
        &self,
        name: Option<String>,
    ) -> Result<Vec<Team>, Error> {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT id, name, description FROM teams");

        if let Some(filter_name) = name {
            qb.push(" WHERE ");
            let pattern = format!("%{}%", filter_name);
            qb.push("name ILIKE ").push_bind(pattern);
        }

        let query = qb.build_query_as::<Team>();
        let result = query.fetch_all(&self.pool).await;

        let teams = handle_error(None, result)?;
        Ok(teams)
    }

    async fn create_team(&self, input: CreateTeam) -> Result<Team, Error> {
        let id = Uuid::new_v4();
        let result = sqlx::query!(
        r#"INSERT INTO teams (id, name, description) VALUES ($1, $2, $3) RETURNING id, name, description"#,
        id,
        input.name,
        input.description
    )
            .fetch_one(&self.pool)
            .await;

        let handled_error = handle_error(None, result)?;
        Ok(Team {
            id: handled_error.id,
            name: handled_error.name,
            description: handled_error.description,
        })
    }

    async fn update_team(&self, input: UpdateTeam) -> Result<Team, Error> {
        let existing_env = self.get_team_by_id(input.id).await?;
        let result = sqlx::query!(
            r#"UPDATE teams SET name = $1, description = $2 WHERE id = $3 RETURNING id, name, description"#,
            input.name.unwrap_or(existing_env.name),
            input.description.unwrap_or(existing_env.description),
            input.id
        ).fetch_one(&self.pool)
            .await;

        let team = handle_error(Some(input.id), result)?;

        Ok(Team {
            id: team.id,
            name: team.name,
            description: team.description,
        })
    }

    async fn delete_team(&self, id: Uuid) -> Result<(), Error> {
        self.get_team_by_id(id).await?;
        let result = sqlx::query!("DELETE FROM teams WHERE id = $1", id)
            .execute(&self.pool)
            .await;
        let _ = handle_error(Some(id), result)?;
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn TeamRepository> {
        Box::new(self.clone())
    }
}
