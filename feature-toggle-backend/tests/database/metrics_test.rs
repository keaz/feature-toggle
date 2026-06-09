use chrono::{Duration, Utc};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::metrics::{
    CreateMetric, CreateMetricEvent, MetricType, metric_repository,
};
use uuid::Uuid;

fn seeded_team() -> Uuid {
    Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap()
}

fn seed_environment() -> Uuid {
    Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap()
}

#[tokio::test]
async fn test_conversion_metric_dedupes_user() {
    let pool = init_pg_pool().await;
    let repo = metric_repository(pool);
    let metric_key = format!("conversion-metric-{}", Uuid::new_v4());
    let feature_key = format!("feature-metric-{}", Uuid::new_v4());

    let metric = repo
        .create_metric(CreateMetric {
            team_id: seeded_team(),
            key: metric_key,
            name: "Signup conversion (test)".into(),
            description: Some("dedupe check".into()),
            metric_type: MetricType::Conversion,
            unit: Some("count".into()),
            success_criteria: None,
        })
        .await
        .expect("failed to create metric");

    let events = vec![
        CreateMetricEvent {
            metric_id: metric.id,
            feature_key: Some(feature_key.clone()),
            environment_id: Some(seed_environment()),
            user_context: "user-1".into(),
            variant: Some("control".into()),
            value: 1.0,
            metadata: None,
            occurred_at: Utc::now(),
            is_conversion: true,
        },
        CreateMetricEvent {
            metric_id: metric.id,
            feature_key: Some(feature_key.clone()),
            environment_id: Some(seed_environment()),
            user_context: "user-1".into(),
            variant: Some("control".into()),
            value: 1.0,
            metadata: None,
            occurred_at: Utc::now(),
            is_conversion: true,
        },
    ];

    let stored = repo
        .insert_metric_events(events)
        .await
        .expect("failed to store events");
    assert_eq!(stored.len(), 1, "duplicate conversions should be deduped");
}

#[tokio::test]
async fn test_metric_aggregation_round_trip() {
    let pool = init_pg_pool().await;
    let repo = metric_repository(pool.clone());
    let metric_key = format!("numeric-metric-{}", Uuid::new_v4());
    let feature_key = format!("feature-agg-{}", Uuid::new_v4());
    let now = Utc::now();

    let metric = repo
        .create_metric(CreateMetric {
            team_id: seeded_team(),
            key: metric_key,
            name: "Checkout value (test)".into(),
            description: Some("aggregation test metric".into()),
            metric_type: MetricType::Numeric,
            unit: Some("$".into()),
            success_criteria: None,
        })
        .await
        .expect("failed to create metric");

    let events = vec![
        CreateMetricEvent {
            metric_id: metric.id,
            feature_key: Some(feature_key.clone()),
            environment_id: Some(seed_environment()),
            user_context: "agg-user-1".into(),
            variant: Some("control".into()),
            value: 10.0,
            metadata: None,
            occurred_at: now - Duration::minutes(10),
            is_conversion: false,
        },
        CreateMetricEvent {
            metric_id: metric.id,
            feature_key: Some(feature_key.clone()),
            environment_id: Some(seed_environment()),
            user_context: "agg-user-2".into(),
            variant: Some("treatment".into()),
            value: 25.0,
            metadata: None,
            occurred_at: now - Duration::minutes(5),
            is_conversion: false,
        },
    ];

    repo.insert_metric_events(events)
        .await
        .expect("failed to store events");

    repo.upsert_aggregations(now - Duration::hours(1), now + Duration::minutes(1), "hour")
        .await
        .expect("aggregation upsert failed");

    let results = repo
        .get_metric_results(
            &feature_key,
            Some(seeded_team()),
            Some(seed_environment()),
            now - Duration::hours(1),
            now + Duration::hours(1),
        )
        .await
        .expect("failed to fetch results");

    assert!(
        !results.is_empty(),
        "expected aggregated metric rows to be present"
    );
    let total_sample: i64 = results.iter().map(|r| r.sample_size).sum();
    assert!(
        total_sample >= 2,
        "expected at least two samples aggregated, got {}",
        total_sample
    );

    let wrong_team_results = repo
        .get_metric_results(
            &feature_key,
            Some(Uuid::new_v4()),
            Some(seed_environment()),
            now - Duration::hours(1),
            now + Duration::hours(1),
        )
        .await
        .expect("failed to fetch wrong-team results");
    assert!(
        wrong_team_results.is_empty(),
        "expected team filter to hide metric rows from other teams"
    );
}
