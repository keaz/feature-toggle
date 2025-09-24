use std::time::Duration;
use tokio::time::timeout;
use feature_toggle_backend::{
    database::{init_pg_pool, feature_evaluation},
    logic::feature_evaluation::feature_evaluation_logic,
    grpc::{FeatureEvaluationSvc, pb}
};
use uuid::Uuid;
use chrono::Utc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize database connection
    let pool = init_pg_pool().await;
    
    // Create feature evaluation logic
    let repo = feature_evaluation::feature_evaluation_repository(pool.clone());
    let logic = feature_evaluation_logic(repo);
    
    // Test creating an evaluation record
    let client_id = Uuid::new_v4();
    let feature_key = "test-feature".to_string();
    let environment_id = "prod".to_string();
    
    println!("Testing feature evaluation tracking...");
    
    // Test single evaluation recording
    let result = logic.record_evaluation(
        feature_key.clone(),
        environment_id.clone(),
        client_id,
        Utc::now(),
        true,
        Some(serde_json::json!({"user.id": "user123", "country": "US"})),
        Some("user123".to_string()),
    ).await;
    
    match result {
        Ok(evaluation) => {
            println!("✓ Successfully recorded evaluation: {:?}", evaluation.id);
            println!("  Feature: {}", evaluation.feature_key);
            println!("  Environment: {}", evaluation.environment_id);
            println!("  Result: {}", evaluation.evaluation_result);
            println!("  User Context: {:?}", evaluation.user_context);
        }
        Err(e) => {
            println!("✗ Failed to record evaluation: {}", e);
            return Err(e.into());
        }
    }
    
    // Test bulk evaluation recording
    let bulk_evaluations = vec![
        feature_evaluation::CreateFeatureEvaluation {
            feature_key: "feature-1".to_string(),
            environment_id: "staging".to_string(),
            client_id,
            evaluated_at: Utc::now(),
            evaluation_result: true,
            evaluation_context: Some(serde_json::json!({"user.id": "user456"})),
            user_context: Some("user456".to_string()),
        },
        feature_evaluation::CreateFeatureEvaluation {
            feature_key: "feature-2".to_string(),
            environment_id: "staging".to_string(),
            client_id,
            evaluated_at: Utc::now(),
            evaluation_result: false,
            evaluation_context: Some(serde_json::json!({"user.id": "user789"})),
            user_context: Some("user789".to_string()),
        },
    ];
    
    let bulk_result = logic.record_evaluations_bulk(bulk_evaluations).await;
    match bulk_result {
        Ok(evaluations) => {
            println!("✓ Successfully recorded {} bulk evaluations", evaluations.len());
        }
        Err(e) => {
            println!("✗ Failed to record bulk evaluations: {}", e);
            return Err(e.into());
        }
    }
    
    // Test querying evaluations
    let filter = feature_evaluation::FeatureEvaluationFilter {
        feature_key: None,
        environment_id: None,
        client_id: Some(client_id),
        user_context: None,
        from_date: None,
        to_date: None,
        limit: Some(10),
        offset: Some(0),
    };
    
    let query_result = logic.get_evaluations(filter).await;
    match query_result {
        Ok(evaluations) => {
            println!("✓ Successfully queried {} evaluations", evaluations.len());
            for eval in evaluations {
                println!("  - {} in {} -> {}", eval.feature_key, eval.environment_id, eval.evaluation_result);
            }
        }
        Err(e) => {
            println!("✗ Failed to query evaluations: {}", e);
            return Err(e.into());
        }
    }
    
    println!("\n🎉 All tests passed! Feature evaluation tracking is working correctly.");
    println!("\nNext steps:");
    println!("1. Start the feature-toggle-backend server");
    println!("2. Start the feature-edge-server");
    println!("3. Make evaluation requests through the edge server");
    println!("4. The edge server will periodically push evaluation events to the backend");
    println!("5. Check the feature_evaluations table to see tracked evaluations");
    
    Ok(())
}
