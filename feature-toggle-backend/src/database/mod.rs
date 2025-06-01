pub mod entity;
pub mod environment;
pub mod pipeline;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::env;
use uuid::Uuid;

pub async fn init_pg_pool() -> PgPool {
    let db_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("Failed to connect to Postgres")
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Record does not exists for the id {0}")]
    NotFound(Uuid),
    #[error("Database error occurred")]
    DatabaseError(#[source] sqlx::Error),
}

pub fn handle_error<T>(id: Option<Uuid>, result: Result<T, sqlx::Error>) -> Result<T, Error> {
    if result.is_ok() {
        Ok(result.unwrap())
    } else {
        let error = result.err().unwrap();
        match error {
            sqlx::Error::RowNotFound => Err(Error::NotFound(id.unwrap())),
            _ => Err(Error::DatabaseError(error)),
        }
    }
}
