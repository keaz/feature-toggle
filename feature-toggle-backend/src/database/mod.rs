pub mod entity;
pub mod environment;
pub mod pipeline;
mod stage;
pub mod team;
mod feature;

use crate::Error;
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


pub fn handle_error<T>(id: Option<Uuid>, result: Result<T, sqlx::Error>) -> Result<T, Error> {
    if let Ok(record) = result {
        Ok(record)
    } else {
        let error = result.err().unwrap();
        let x = error.to_string();
        match error {
            sqlx::Error::RowNotFound => Err(Error::NotFound(id.unwrap())),
            _ => Err(Error::DatabaseError(error)),
        }
    }
}
