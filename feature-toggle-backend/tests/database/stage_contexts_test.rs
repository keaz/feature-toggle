use feature_toggle_backend::Error;
use feature_toggle_backend::database::context::{CreateContextInput, context_repository};
use feature_toggle_backend::database::team::{CreateTeam, team_repository};
use feature_toggle_backend::database::{feature, init_pg_pool};
use std::sync::OnceLock;
use uuid::Uuid;

fn stage_context_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
async fn test_set_and_get_stage_contexts() {
    let _guard = stage_context_test_lock().lock().await;
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // Use seeded stage and contexts from init.sql
    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(); // features_pipeline_stages seeded id
    let ctx1 = Uuid::parse_str("cb461425-373b-49d9-9634-9a248612d7b7").unwrap(); // filter-alpha
    let ctx2 = Uuid::parse_str("fcc0dfca-07b0-44ad-8d9a-21f2cd450d10").unwrap(); // filter-beta

    // Set two contexts for stage
    let set_out = repo
        .set_stage_contexts(stage_id, vec![ctx1, ctx2])
        .await
        .expect("set contexts should succeed");
    assert_eq!(set_out.len(), 2);
    let keys: Vec<String> = set_out.iter().map(|c| c.key.clone()).collect();
    assert!(keys.contains(&"filter-alpha".to_string()));
    assert!(keys.contains(&"filter-beta".to_string()));

    // Get and verify order by key
    let got = repo
        .get_stage_contexts(stage_id)
        .await
        .expect("get contexts should succeed");
    assert_eq!(got.len(), 2);
    assert!(got.iter().any(|c| c.key == "filter-alpha"));
    assert!(got.iter().any(|c| c.key == "filter-beta"));

    // Clear contexts (empty list) and verify cleared
    let cleared = repo
        .set_stage_contexts(stage_id, vec![])
        .await
        .expect("clear contexts should succeed");
    assert_eq!(cleared.len(), 0);
    let got2 = repo.get_stage_contexts(stage_id).await.unwrap();
    assert_eq!(got2.len(), 0);
}

#[tokio::test]
async fn test_set_stage_contexts_rejects_cross_team_context() {
    let _guard = stage_context_test_lock().lock().await;
    let pool = init_pg_pool().await;
    let feature_repo = feature::feature_repository(pool.clone());
    let team_repo = team_repository(pool.clone());
    let context_repo = context_repository(pool.clone());

    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap();
    let valid_context = Uuid::parse_str("cb461425-373b-49d9-9634-9a248612d7b7").unwrap();

    let foreign_team = team_repo
        .create_team(CreateTeam {
            name: format!("Foreign Stage Context Team {}", Uuid::new_v4()),
            description: "foreign team for stage-context integrity test".to_string(),
        })
        .await
        .expect("foreign team should be created");

    let foreign_context = context_repo
        .create_context(
            foreign_team.id,
            CreateContextInput {
                key: format!("foreign-context-{}", Uuid::new_v4()),
                entries: vec![],
            },
        )
        .await
        .expect("foreign context should be created");

    let valid = feature_repo
        .set_stage_contexts(stage_id, vec![valid_context])
        .await
        .expect("valid seeded context should be accepted");
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].id, valid_context);

    let invalid = feature_repo
        .set_stage_contexts(stage_id, vec![foreign_context.id])
        .await;
    assert!(matches!(invalid, Err(Error::DatabaseError(_))));

    let after = feature_repo
        .get_stage_contexts(stage_id)
        .await
        .expect("stage contexts should still be readable");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, valid_context);

    let cleared = feature_repo
        .set_stage_contexts(stage_id, vec![])
        .await
        .expect("stage contexts should be reset after the test");
    assert!(cleared.is_empty());

    team_repo
        .delete_team(foreign_team.id)
        .await
        .expect("foreign team cleanup should succeed");
}
