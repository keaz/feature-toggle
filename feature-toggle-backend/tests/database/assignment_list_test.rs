use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::user_flag_assignment::user_flag_assignment_repository;
use uuid::Uuid;

// Seeded in init.sql: a team and two of its environments.
const TEAM_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const ENV_A: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const ENV_B: &str = "06f28625-df1d-499f-a4ee-5629a8b6a169";

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap()
}

#[tokio::test]
async fn test_list_by_environment_returns_only_that_environment() {
    let pool = init_pg_pool().await;
    let repository = user_flag_assignment_repository(pool.clone());

    // Own feature with a stage in both environments, so the stage filter
    // alone matches it for either environment.
    let suffix = Uuid::new_v4();
    let feature_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, $2, 'Simple', $3)",
    )
    .bind(feature_id)
    .bind(format!("b17-env-filter-{suffix}"))
    .bind(uuid(TEAM_ID))
    .execute(&pool)
    .await
    .expect("insert feature");
    for (order_index, env) in [ENV_A, ENV_B].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO features_pipeline_stages (id, feature_id, environment_id, order_index, position)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(feature_id)
        .bind(uuid(env))
        .bind(order_index as i32)
        .bind(format!("stage-{order_index}"))
        .execute(&pool)
        .await
        .expect("insert stage");
    }

    let user_id = format!("b17-env-filter-user-{suffix}");
    let upsert_a = repository
        .upsert(&user_id, feature_id, uuid(ENV_A), true, Some("in-a".into()))
        .await;
    let upsert_b = repository
        .upsert(&user_id, feature_id, uuid(ENV_B), true, Some("in-b".into()))
        .await;

    let listed = repository
        .list(uuid(TEAM_ID), None, Some(uuid(ENV_A)))
        .await;

    sqlx::query("DELETE FROM user_flag_assignments WHERE feature_id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await
        .expect("cleanup assignments");
    sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await
        .expect("cleanup feature");

    upsert_a.expect("upsert env A");
    upsert_b.expect("upsert env B");
    let listed = listed.expect("list by environment");

    let other_envs: Vec<_> = listed
        .iter()
        .filter(|r| r.environment_id != uuid(ENV_A))
        .map(|r| (r.user_id.clone(), r.environment_id))
        .collect();
    assert!(
        other_envs.is_empty(),
        "list for env A returned rows of other environments: {other_envs:?}"
    );

    let own: Vec<_> = listed.iter().filter(|r| r.user_id == user_id).collect();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].feature_id, feature_id);
    assert_eq!(own[0].variant.as_deref(), Some("in-a"));
}
