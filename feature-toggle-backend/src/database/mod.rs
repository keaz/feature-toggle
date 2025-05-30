mod entity;
pub mod repository;

use std::env;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub async fn init_pg_pool() -> PgPool {
    let db_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("Failed to connect to Postgres")
}
