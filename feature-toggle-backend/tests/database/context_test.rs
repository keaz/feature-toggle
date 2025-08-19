use feature_toggle_backend::Error;
use feature_toggle_backend::database::context::{self, CreateContextInput, UpdateContextInput};
use feature_toggle_backend::database::init_pg_pool;
use uuid::Uuid;

#[tokio::test]
async fn test_get_context_not_found() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let res = repo.get_context_by_id(id).await;
    assert!(matches!(res, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn test_create_and_get_context() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let key = format!("country-{}", Uuid::new_v4());
    let input = CreateContextInput {
        key: key.clone(),
        entries: vec!["US".into(), "UK".into()],
    };

    let created = repo
        .create_context(team_id, input)
        .await
        .expect("create_context should succeed");
    assert_eq!(created.team_id, team_id);
    assert_eq!(created.key, key);
    assert_eq!(created.entries.len(), 2);

    let fetched = repo
        .get_context_by_id(created.id)
        .await
        .expect("get_context_by_id should succeed");
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.entries.len(), 2);
}

#[tokio::test]
async fn test_create_context_duplicate_key_for_team() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let key = format!("device-{}", Uuid::new_v4());

    // First create
    let _ = repo
        .create_context(
            team_id,
            CreateContextInput {
                key: key.clone(),
                entries: vec!["iOS".into()],
            },
        )
        .await
        .expect("first create should succeed");

    // Duplicate for same team should fail
    let dup = repo
        .create_context(
            team_id,
            CreateContextInput {
                key: key.clone(),
                entries: vec!["Android".into()],
            },
        )
        .await;
    assert!(matches!(dup, Err(Error::RecordAlreadyExists(_))));
}

#[tokio::test]
async fn test_update_context_key_and_entries() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let created = repo
        .create_context(
            team_id,
            CreateContextInput {
                key: format!("segment-{}", Uuid::new_v4()),
                entries: vec!["A".into(), "B".into()],
            },
        )
        .await
        .unwrap();

    // Update key and replace entries
    let new_key = format!("segment-updated-{}", Uuid::new_v4());
    let updated = repo
        .update_context(
            created.id,
            UpdateContextInput {
                key: Some(new_key.clone()),
                entries: Some(vec!["B".into(), "C".into()]),
            },
        )
        .await
        .expect("update_context should succeed");

    assert_eq!(updated.key, new_key);
    assert_eq!(updated.entries.len(), 2);
    assert!(updated.entries.iter().any(|e| e.value == "B"));
    assert!(updated.entries.iter().any(|e| e.value == "C"));
}

#[tokio::test]
async fn test_update_context_duplicate_key_violation() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    // Create two contexts
    let ctx1 = repo
        .create_context(
            team_id,
            CreateContextInput {
                key: format!("dup-a-{}", Uuid::new_v4()),
                entries: vec!["1".into()],
            },
        )
        .await
        .unwrap();
    let ctx2 = repo
        .create_context(
            team_id,
            CreateContextInput {
                key: format!("dup-b-{}", Uuid::new_v4()),
                entries: vec!["2".into()],
            },
        )
        .await
        .unwrap();

    // Try to update ctx2 to have same key as ctx1
    let res = repo
        .update_context(
            ctx2.id,
            UpdateContextInput {
                key: Some(ctx1.key.clone()),
                entries: None,
            },
        )
        .await;
    assert!(matches!(res, Err(Error::RecordAlreadyExists(_))));
}

#[tokio::test]
async fn test_get_contexts_list_and_filter() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let k1 = "filter-alpha".to_string();
    let k2 = "filter-beta".to_string();

    // List all (should include the pre-seeded contexts)
    let all = repo
        .get_contexts(team_id, None)
        .await
        .expect("list contexts");
    assert!(all.iter().any(|c| c.key == k1));
    assert!(all.iter().any(|c| c.key == k2));

    // Filter by substring "alpha" should only include the alpha context
    let filtered = repo
        .get_contexts(team_id, Some("alpha".to_string()))
        .await
        .expect("filtered contexts");
    assert!(filtered.iter().any(|c| c.key == k1));
    assert!(filtered.iter().all(|c| c.key.contains("alpha")));
}

#[tokio::test]
async fn test_delete_context() {
    let pool = init_pg_pool().await;
    let repo = context::context_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let created = repo
        .create_context(
            team_id,
            CreateContextInput {
                key: format!("del-{}", Uuid::new_v4()),
                entries: vec!["A".into()],
            },
        )
        .await
        .unwrap();

    let res = repo.delete_context(created.id).await;
    assert!(res.is_ok());

    let get_after = repo.get_context_by_id(created.id).await;
    assert!(matches!(get_after, Err(Error::NotFound(_))));
}
