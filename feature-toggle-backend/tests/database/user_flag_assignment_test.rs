use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::user_flag_assignment::{
    UserFlagAssignmentRow, user_flag_assignment_repository,
};
use uuid::Uuid;

// Seeded in init.sql: team, "Test Feature" and two environments of that team.
const TEAM_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const FEATURE_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const ENV_A: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const ENV_B: &str = "06f28625-df1d-499f-a4ee-5629a8b6a169";

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap()
}

fn row(user_id: &str, environment_id: &str, variant: Option<&str>) -> UserFlagAssignmentRow {
    UserFlagAssignmentRow {
        user_id: user_id.to_string(),
        feature_id: uuid(FEATURE_ID),
        environment_id: uuid(environment_id),
        assigned: true,
        variant: variant.map(str::to_string),
    }
}

async fn stored_rows(
    pool: &sqlx::PgPool,
    user_ids: &[String],
) -> Vec<(String, Uuid, bool, Option<String>)> {
    sqlx::query_as::<_, (String, Uuid, bool, Option<String>)>(
        "SELECT user_id, environment_id, assigned, variant FROM user_flag_assignments
         WHERE user_id = ANY($1) ORDER BY user_id, environment_id",
    )
    .bind(user_ids)
    .fetch_all(pool)
    .await
    .expect("query stored assignments")
}

#[tokio::test]
async fn test_upsert_many_inserts_and_updates_rows() {
    let pool = init_pg_pool().await;
    let repository = user_flag_assignment_repository(pool.clone());

    let suffix = Uuid::new_v4();
    let user_1 = format!("p03-upsert-many-1-{suffix}");
    let user_2 = format!("p03-upsert-many-2-{suffix}");
    let users = vec![user_1.clone(), user_2.clone()];

    let inserted = repository
        .upsert_many(&[
            row(&user_1, ENV_A, Some("control")),
            row(&user_1, ENV_B, None),
        ])
        .await;

    let after_insert = stored_rows(&pool, &users).await;

    let mut not_assigned = row(&user_1, ENV_B, None);
    not_assigned.assigned = false;
    let updated = repository
        .upsert_many(&[
            row(&user_1, ENV_A, Some("treatment")),
            not_assigned,
            row(&user_2, ENV_A, None),
        ])
        .await;

    let after_update = stored_rows(&pool, &users).await;

    sqlx::query("DELETE FROM user_flag_assignments WHERE user_id = ANY($1)")
        .bind(&users)
        .execute(&pool)
        .await
        .expect("cleanup assignments");

    inserted.expect("insert should succeed");
    updated.expect("update should succeed");

    let mut expected_insert = vec![
        (
            user_1.clone(),
            uuid(ENV_A),
            true,
            Some("control".to_string()),
        ),
        (user_1.clone(), uuid(ENV_B), true, None),
    ];
    expected_insert.sort();
    let mut after_insert = after_insert;
    after_insert.sort();
    assert_eq!(after_insert, expected_insert);

    let mut expected_update = vec![
        (
            user_1.clone(),
            uuid(ENV_A),
            true,
            Some("treatment".to_string()),
        ),
        (user_1.clone(), uuid(ENV_B), false, None),
        (user_2.clone(), uuid(ENV_A), true, None),
    ];
    expected_update.sort();
    let mut after_update = after_update;
    after_update.sort();
    assert_eq!(after_update, expected_update);
}

#[tokio::test]
async fn test_upsert_many_with_no_rows_is_a_no_op() {
    let pool = init_pg_pool().await;
    let repository = user_flag_assignment_repository(pool);

    repository
        .upsert_many(&[])
        .await
        .expect("empty upsert should succeed");
}

#[tokio::test]
async fn test_all_owned_by_team_checks_every_id() {
    let pool = init_pg_pool().await;
    let repository = user_flag_assignment_repository(pool);
    let team_id = uuid(TEAM_ID);

    // Duplicates of owned ids are fine.
    let owned = repository
        .all_owned_by_team(
            team_id,
            &[uuid(FEATURE_ID), uuid(FEATURE_ID)],
            &[uuid(ENV_A), uuid(ENV_B), uuid(ENV_A)],
        )
        .await
        .expect("ownership query");
    assert!(owned);

    // One unknown feature id among owned ones fails the whole check.
    let unknown_feature = repository
        .all_owned_by_team(team_id, &[uuid(FEATURE_ID), Uuid::new_v4()], &[uuid(ENV_A)])
        .await
        .expect("ownership query");
    assert!(!unknown_feature);

    // One unknown environment id among owned ones fails the whole check.
    let unknown_env = repository
        .all_owned_by_team(team_id, &[uuid(FEATURE_ID)], &[uuid(ENV_A), Uuid::new_v4()])
        .await
        .expect("ownership query");
    assert!(!unknown_env);

    // Owned ids checked against another team fail.
    let other_team = repository
        .all_owned_by_team(Uuid::new_v4(), &[uuid(FEATURE_ID)], &[uuid(ENV_A)])
        .await
        .expect("ownership query");
    assert!(!other_team);
}
