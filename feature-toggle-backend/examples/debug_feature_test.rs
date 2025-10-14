use feature_toggle_backend::database::{feature, init_pg_pool};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let pool = init_pg_pool().await;
    let repository = feature::feature_repository(pool);

    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();
    
    // Get all features first
    let all_result = repository
        .get_features(team_id, None, None)
        .await;
    
    if let Ok(all_features) = all_result {
        println!("All features:");
        for feature in &all_features {
            println!("  - '{}' (contains 'Test': {})", feature.key, feature.key.contains("Test"));
        }
        println!("Total features: {}", all_features.len());
    }
    
    // Get filtered features
    let result = repository
        .get_features(team_id, Some("Test".to_string()), None)
        .await;

    if let Ok(features) = result {
        println!("\nFiltered features (key contains 'Test'):");
        for feature in &features {
            println!("  - '{}' (contains 'Test': {})", feature.key, feature.key.contains("Test"));
        }
        println!("Filtered features count: {}", features.len());
        
        let all_contain_test = features.iter().all(|p| p.key.contains("Test"));
        println!("All filtered features contain 'Test': {}", all_contain_test);
    } else {
        println!("Error getting filtered features: {:?}", result);
    }
}