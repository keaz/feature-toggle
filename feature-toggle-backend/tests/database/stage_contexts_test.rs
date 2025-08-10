use feature_toggle_backend::database::{feature, init_pg_pool};
use uuid::Uuid;

#[tokio::test]
async fn test_set_and_get_stage_contexts() {
    let pool = init_pg_pool().await;
    let repo = feature::feature_repository(pool);

    // Use seeded stage and contexts from init.sql
    let stage_id = Uuid::parse_str("3eef17bc-9e06-411d-b5f4-7a786e68bb96").unwrap(); // features_pipeline_stages seeded id
    let ctx1 = Uuid::parse_str("cb461425-373b-49d9-9634-9a248612d7b7").unwrap(); // filter-alpha
    let ctx2 = Uuid::parse_str("fcc0dfca-07b0-44ad-8d9a-21f2cd450d10").unwrap(); // filter-beta

    // Set two contexts for stage
    let set_out = repo.set_stage_contexts(stage_id, vec![ctx1, ctx2]).await.expect("set contexts should succeed");
    assert_eq!(set_out.len(), 2);
    let keys: Vec<String> = set_out.iter().map(|c| c.key.clone()).collect();
    assert!(keys.contains(&"filter-alpha".to_string()));
    assert!(keys.contains(&"filter-beta".to_string()));

    // Get and verify order by key
    let got = repo.get_stage_contexts(stage_id).await.expect("get contexts should succeed");
    assert_eq!(got.len(), 2);
    assert!(got.iter().any(|c| c.key == "filter-alpha"));
    assert!(got.iter().any(|c| c.key == "filter-beta"));

    // Clear contexts (empty list) and verify cleared
    let cleared = repo.set_stage_contexts(stage_id, vec![]).await.expect("clear contexts should succeed");
    assert_eq!(cleared.len(), 0);
    let got2 = repo.get_stage_contexts(stage_id).await.unwrap();
    assert_eq!(got2.len(), 0);
}
