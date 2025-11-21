use feature_toggle_backend::database::feature::CreateStageCriterion;
use feature_toggle_backend::database::{feature, init_pg_pool};
use uuid::Uuid;

#[tokio::test]
async fn test_get_stage_criteria_returns_seeded_values() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // Known seeded stage id (from init.sql)
    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap();

    let result = repo.get_stage_criteria(stage_id).await;
    assert!(result.is_ok());
    let criteria = result.unwrap();

    assert!(!criteria.is_empty());
    assert!(criteria.iter().all(|c| c.stage_id == stage_id));

    let mut priorities: Vec<i32> = criteria.iter().map(|c| c.priority).collect();
    priorities.sort_unstable();
    assert_eq!(priorities, vec![0, 1]);
}

#[tokio::test]
async fn test_get_stage_criteria_empty() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // This stage id exists and is seeded but choose one without criteria
    let stage_without_criteria = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let result = repo.get_stage_criteria(stage_without_criteria).await;
    assert!(result.is_ok());
    let criteria = result.unwrap();
    assert!(criteria.is_empty());
}

#[tokio::test]
async fn test_set_stage_criteria_replaces_existing() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // Use a different stage that doesn't interfere with variant tests
    let stage_id = Uuid::parse_str("1ab6ca79-a4fc-44ba-87e2-12884edf17f7").unwrap();

    // First set some initial criteria
    let initial_crit = vec![
        CreateStageCriterion {
            priority: 0,
        },
        CreateStageCriterion {
            priority: 1,
        },
    ];

    let _ = repo.set_stage_criteria(stage_id, initial_crit).await;

    // Now replace them with a single criterion
    let crit = vec![CreateStageCriterion {
        priority: 0,
    }];

    let set_result = repo.set_stage_criteria(stage_id, crit).await;
    assert!(set_result.is_ok());
    let updated = set_result.unwrap();

    // Should now be exactly 1 criterion
    assert_eq!(updated.len(), 1);
    let c = &updated[0];
    assert_eq!(c.stage_id, stage_id);
}

#[tokio::test]
async fn test_set_stage_criteria_stage_not_found() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    let non_existing_stage = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();

    let crit = vec![CreateStageCriterion {
        priority: 0,
    }];

    let result = repo.set_stage_criteria(non_existing_stage, crit).await;
    assert!(result.is_err());
    let err = result.err().unwrap();
    // Should be NotFound for the stage id
    match err {
        feature_toggle_backend::Error::NotFound(id) => assert_eq!(id, non_existing_stage),
        _ => panic!("expected NotFound error"),
    }
}
