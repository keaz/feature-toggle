use chrono::Utc;
use feature_toggle_backend::database::feature_evaluation::{
    CreateFeatureEvaluation, FeatureEvaluationFilter, feature_evaluation_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use serde_json::json;
use uuid::Uuid;

async fn repo()
-> Box<dyn feature_toggle_backend::database::feature_evaluation::FeatureEvaluationRepository> {
    let pool = init_pg_pool().await;
    feature_evaluation_repository(pool)
}

#[tokio::test]
async fn test_create_evaluation() {
    let repo = repo().await;
    let unique_key = format!("test-feature-create-{}", Uuid::new_v4());
    let seeded_client_id = Uuid::parse_str("a1b2c3d4-0000-4000-8000-000000000001").unwrap();
    let eval = CreateFeatureEvaluation {
        feature_key: unique_key.clone(),
        environment_id: "env-123".to_string(),
        client_id: seeded_client_id,
        evaluated_at: Utc::now(),
        #[allow(deprecated)]
        evaluation_result: true,
        evaluation_context: Some(json!({"user": "test-user"})),
        user_context: Some("user123".to_string()),
        prior_assignment: false,
        evaluation_success: true,
        evaluation_value: Some(json!(true)),
        variant: None,
    };
    let created = repo.create_evaluation(eval.clone()).await.unwrap();
    assert_eq!(created.feature_key, eval.feature_key);
    assert_eq!(created.environment_id, eval.environment_id);
    assert!(created.evaluation_result);
}

#[tokio::test]
async fn test_bulk_create_evaluations() {
    let repo = repo().await;
    let key1 = format!("bulk-feature-1-{}", Uuid::new_v4());
    let key2 = format!("bulk-feature-2-{}", Uuid::new_v4());
    let seeded_client_id = Uuid::parse_str("a1b2c3d4-0000-4000-8000-000000000001").unwrap();
    let evals = vec![
        CreateFeatureEvaluation {
            feature_key: key1.clone(),
            environment_id: "env-123".to_string(),
            client_id: seeded_client_id,
            evaluated_at: Utc::now(),
            #[allow(deprecated)]
            evaluation_result: true,
            evaluation_context: None,
            user_context: Some("userA".to_string()),
            prior_assignment: false,
            evaluation_success: true,
            evaluation_value: Some(json!(true)),
            variant: None,
        },
        CreateFeatureEvaluation {
            feature_key: key2.clone(),
            environment_id: "env-123".to_string(),
            client_id: seeded_client_id,
            evaluated_at: Utc::now(),
            #[allow(deprecated)]
            evaluation_result: false,
            evaluation_context: None,
            user_context: Some("userB".to_string()),
            prior_assignment: true,
            evaluation_success: true,
            evaluation_value: Some(json!(false)),
            variant: None,
        },
    ];
    let created = repo.bulk_create_evaluations(evals.clone()).await.unwrap();
    assert_eq!(created.len(), 2);
    assert_eq!(created[0].feature_key, key1);
    assert_eq!(created[1].feature_key, key2);
}

#[tokio::test]
async fn test_duplicate_create_evaluation_is_idempotent() {
    let repo = repo().await;
    let unique_key = format!("test-feature-idempotent-{}", Uuid::new_v4());
    let seeded_client_id = Uuid::parse_str("a1b2c3d4-0000-4000-8000-000000000001").unwrap();
    let eval = CreateFeatureEvaluation {
        feature_key: unique_key.clone(),
        environment_id: "env-123".to_string(),
        client_id: seeded_client_id,
        evaluated_at: Utc::now(),
        #[allow(deprecated)]
        evaluation_result: true,
        evaluation_context: Some(json!({"user": "retry-user"})),
        user_context: Some("retry-user".to_string()),
        prior_assignment: false,
        evaluation_success: true,
        evaluation_value: Some(json!(true)),
        variant: Some("control".to_string()),
    };

    let created = repo.create_evaluation(eval.clone()).await.unwrap();
    let duplicated = repo.create_evaluation(eval).await.unwrap();

    assert_eq!(created.feature_key, duplicated.feature_key);
    assert_eq!(created.environment_id, duplicated.environment_id);
    assert_eq!(created.user_context, duplicated.user_context);

    let filter = FeatureEvaluationFilter {
        team_id: None,
        feature_key: Some(unique_key),
        environment_id: None,
        client_id: None,
        user_context: None,
        prior_assignment: None,
        from_date: None,
        to_date: None,
        limit: Some(10),
        offset: Some(0),
    };
    let evals = repo.get_evaluations(filter).await.unwrap();
    assert_eq!(
        evals.len(),
        1,
        "duplicate ingest should not create a second row"
    );
}

#[tokio::test]
async fn test_get_evaluations_seeded() {
    let repo = repo().await;
    // Use seeded feature_key from init.sql if available
    let filter = FeatureEvaluationFilter {
        team_id: None,
        feature_key: Some("test-feature-create".to_string()),
        environment_id: None,
        client_id: None,
        user_context: None,
        prior_assignment: None,
        from_date: None,
        to_date: None,
        limit: Some(10),
        offset: Some(0),
    };
    let evals = repo.get_evaluations(filter).await.unwrap();
    assert!(evals.iter().any(|e| e.feature_key == "test-feature-create"));
}

#[tokio::test]
async fn test_get_evaluation_count() {
    let repo = repo().await;
    let filter = FeatureEvaluationFilter {
        team_id: None,
        feature_key: Some("test-feature-create".to_string()),
        environment_id: None,
        client_id: None,
        user_context: None,
        prior_assignment: None,
        from_date: None,
        to_date: None,
        limit: None,
        offset: None,
    };
    let count = repo.get_evaluation_count(filter).await.unwrap();
    assert!(count >= 1);
}

#[tokio::test]
async fn test_get_evaluation_rates_and_summary() {
    let repo = repo().await;
    let now = Utc::now();
    let from_time = now - chrono::Duration::days(1);
    let to_time = now + chrono::Duration::days(1);
    let rates = repo
        .get_evaluation_rates(
            Some("test-feature-create".to_string()),
            None,
            None,
            None,
            from_time,
            to_time,
            60,
        )
        .await
        .unwrap();
    assert!(rates.iter().all(|point| {
        point.evaluation_count >= 0 && point.success_count >= 0 && point.prior_assignment_count >= 0
    }));

    let summary = repo
        .get_evaluation_summary(
            Some("test-feature-create".to_string()),
            None,
            None,
            None,
            from_time,
            to_time,
        )
        .await
        .unwrap();
    assert!(summary.total_evaluations >= 1);
    assert!(summary.success_rate >= 0.0);
}

#[tokio::test]
async fn test_team_and_canonical_environment_scoped_analytics() {
    let repo = repo().await;
    let seeded_team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    let seeded_environment_id = seeded_team_id.to_string();
    let now = Utc::now();
    let from_time = now - chrono::Duration::days(1);
    let to_time = now + chrono::Duration::days(1);

    let scoped_count = repo
        .get_evaluation_count(FeatureEvaluationFilter {
            team_id: Some(seeded_team_id),
            feature_key: Some("test-feature-create".to_string()),
            environment_id: Some(seeded_environment_id.clone()),
            client_id: None,
            user_context: None,
            prior_assignment: None,
            from_date: Some(from_time),
            to_date: Some(to_time),
            limit: None,
            offset: None,
        })
        .await
        .unwrap();

    assert!(scoped_count >= 1);

    let summary = repo
        .get_evaluation_summary(
            Some("test-feature-create".to_string()),
            Some(seeded_environment_id),
            None,
            Some(seeded_team_id),
            from_time,
            to_time,
        )
        .await
        .unwrap();

    assert!(summary.total_evaluations >= 1);
    assert!(summary.top_feature_key.is_some());
}
