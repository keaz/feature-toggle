use feature_toggle_backend::database::{feature, init_pg_pool};
use feature_toggle_backend::database::feature::CreateStageCriterion;
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

    // We seeded two criteria
    assert_eq!(criteria.len(), 2);

    // Validate content: same stage_id and context keys
    for c in &criteria {
        assert_eq!(c.stage_id, stage_id);
        assert_eq!(c.context_key, "filter");
        // Context should be fully loaded with entries
        assert_eq!(c.context.team_id, Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap());
        assert!(!c.context.entries.is_empty());
        // Rollout percentage should be within [0,100]
        assert!(c.rollout_percentage >= 0 && c.rollout_percentage <= 100);
    }

    // Check that contexts correspond to the seeded keys
    let mut keys: Vec<String> = criteria.iter().map(|c| c.context.key.clone()).collect();
    keys.sort();
    assert_eq!(keys, vec!["filter-alpha".to_string(), "filter-beta".to_string()]);
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

    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap();

    // Prepare a new set of criteria (will replace seeded ones)
    let crit = vec![
        CreateStageCriterion {
            context_key: "filter".to_string(),
            context_id: Uuid::parse_str("cb461425-373b-49d9-9634-9a248612d7b7").unwrap(),
            rollout_percentage: 75,
        },
    ];

    let set_result = repo.set_stage_criteria(stage_id, crit).await;
    assert!(set_result.is_ok());
    let updated = set_result.unwrap();

    // Should now be exactly 1 criterion
    assert_eq!(updated.len(), 1);
    let c = &updated[0];
    assert_eq!(c.stage_id, stage_id);
    assert_eq!(c.context_key, "filter");
    assert_eq!(c.context.key, "filter-alpha");
    assert_eq!(c.rollout_percentage, 75);
}

#[tokio::test]
async fn test_set_stage_criteria_stage_not_found() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    let non_existing_stage = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();

    let crit = vec![CreateStageCriterion {
        context_key: "filter".to_string(),
        context_id: Uuid::parse_str("cb461425-373b-49d9-9634-9a248612d7b7").unwrap(),
        rollout_percentage: 10,
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
