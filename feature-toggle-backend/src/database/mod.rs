pub mod activity_log;
pub mod approval;
pub mod client;
pub mod compound_rules;
pub mod context;
pub mod entity;
pub mod environment;
pub mod feature;
pub mod feature_evaluation;
pub mod jwt_secret;
pub mod jwt_token;
pub mod metrics;
pub mod notification;
pub mod pipeline;
pub mod role;
pub mod team;
pub mod transaction;
pub mod user;
pub mod user_flag_assignment;
pub mod variant_allocations;

use crate::Error;
use log::error;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
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

pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub fn handle_error<T>(id: Option<Uuid>, result: Result<T, sqlx::Error>) -> Result<T, Error> {
    if let Ok(record) = result {
        return Ok(record);
    }

    let error = result.err().unwrap();
    error!("Database error: {error}");

    match &error {
        sqlx::Error::RowNotFound => {
            // When no id context is provided, fall back to DatabaseError to avoid panics
            if let Some(id) = id {
                Err(Error::NotFound(id))
            } else {
                Err(Error::DatabaseError(error))
            }
        }
        sqlx::Error::Database(db_err) => {
            // Map Postgres unique constraint violations to a friendlier validation error
            if let Some(code) = db_err.code()
                && code == "23505"
            {
                // Unique violation
                let field = match db_err.constraint() {
                    Some(c) if c.contains("users_username_key") => "username",
                    Some(c) if c.contains("users_email_key") => "email",
                    _ => "record",
                };
                return Err(Error::RecordAlreadyExists(field.to_string()));
            }
            Err(Error::DatabaseError(error))
        }
        _ => Err(Error::DatabaseError(error)),
    }
}
