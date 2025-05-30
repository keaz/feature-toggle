use sqlx::PgPool;
use uuid::Uuid;
use feature_toggle_shared::graphql::{CreateEnvironmentInput, UpdateEnvironmentInput};
use crate::database::entity::Environment;

pub async fn get_environment_by_id(
    pool: &PgPool,
    env_id: Uuid,
) -> Result<Environment, sqlx::Error> {
    sqlx::query_as::<_, Environment>(
        "SELECT id, name FROM environments WHERE id = $1",
    )
        .bind(env_id)
        .fetch_one(pool)
        .await
}

pub async fn create_environment(
    pool: &PgPool,
    input: &CreateEnvironmentInput,
) -> Result<Environment, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO environments (id, name, active) VALUES ($1, $2, true)",
        id,
        input.name.clone()
    )
        .execute(pool)
        .await?;

    Ok(Environment {
        id: id.into(),
        name: input.name.clone(),
    })
}

pub async fn update_environment(
    pool: &PgPool,
    input: UpdateEnvironmentInput,
) -> Result<Environment, sqlx::Error> {
    let uuid = Uuid::try_from(input.id).unwrap();
    sqlx::query!(
        "UPDATE environments SET name = $1 WHERE id = $2",
        input.name,
        uuid
    )
        .execute(pool)
        .await?;

    Ok(Environment {
        id: uuid,
        name: input.name,
    })
}

pub async fn delete_environment(
    pool: &PgPool,
    id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM environments WHERE id = $1",
        id
    )
        .execute(pool)
        .await?;

    Ok(())
}

