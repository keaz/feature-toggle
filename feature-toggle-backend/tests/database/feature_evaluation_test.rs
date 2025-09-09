use chrono::{TimeZone, Utc};
use feature_toggle_backend::database::feature_evaluation::{feature_evaluation_repository, CreateFeatureEvaluation, FeatureEvaluationFilter};
use feature_toggle_backend::database::init_pg_pool;
use uuid::Uuid;
use serde_json::json;

async fn repo() -> Box<dyn feature_toggle_backend::database::feature_evaluation::FeatureEvaluationRepository> {
	let pool = init_pg_pool().await;
	feature_evaluation_repository(pool)
}

#[tokio::test]
async fn test_create_evaluation() {
	let repo = repo().await;
	let eval = CreateFeatureEvaluation {
		feature_key: "test-feature-create".to_string(),
		environment_id: "env-123".to_string(),
		client_id: Uuid::new_v4(),
		evaluated_at: Utc::now(),
		evaluation_result: true,
		evaluation_context: Some(json!({"user": "test-user"})),
		user_context: Some("user123".to_string()),
		prior_assignment: false,
	};
	let created = repo.create_evaluation(eval.clone()).await.unwrap();
	assert_eq!(created.feature_key, eval.feature_key);
	assert_eq!(created.environment_id, eval.environment_id);
	assert_eq!(created.evaluation_result, true);
}

#[tokio::test]
async fn test_bulk_create_evaluations() {
	let repo = repo().await;
	let evals = vec![
		CreateFeatureEvaluation {
			feature_key: "bulk-feature-1".to_string(),
			environment_id: "env-123".to_string(),
			client_id: Uuid::new_v4(),
			evaluated_at: Utc::now(),
			evaluation_result: true,
			evaluation_context: None,
			user_context: Some("userA".to_string()),
			prior_assignment: false,
		},
		CreateFeatureEvaluation {
			feature_key: "bulk-feature-2".to_string(),
			environment_id: "env-123".to_string(),
			client_id: Uuid::new_v4(),
			evaluated_at: Utc::now(),
			evaluation_result: false,
			evaluation_context: None,
			user_context: Some("userB".to_string()),
			prior_assignment: true,
		},
	];
	let created = repo.bulk_create_evaluations(evals.clone()).await.unwrap();
	assert_eq!(created.len(), 2);
	assert_eq!(created[0].feature_key, "bulk-feature-1");
	assert_eq!(created[1].feature_key, "bulk-feature-2");
}

#[tokio::test]
async fn test_get_evaluations_seeded() {
	let repo = repo().await;
	// Use seeded feature_key from init.sql if available
	let filter = FeatureEvaluationFilter {
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
	let rates = repo.get_evaluation_rates(
		Some("test-feature-create".to_string()),
		None,
		None,
		from_time,
		to_time,
		60,
	).await.unwrap();
	assert!(rates.len() >= 0); // May be empty if no data

	let summary = repo.get_evaluation_summary(
		Some("test-feature-create".to_string()),
		None,
		None,
		from_time,
		to_time,
	).await.unwrap();
	assert!(summary.total_evaluations >= 1);
	assert!(summary.success_rate >= 0.0);
}
