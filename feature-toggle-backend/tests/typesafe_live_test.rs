//! Calls the live TypeSafe API. Run with:
//! cargo test -p feature-toggle-backend --test typesafe_live_test -- --ignored

use std::collections::BTreeMap;

use feature_toggle_backend::config::TypesafeConfig;
use feature_toggle_backend::judgment::build_client;
use feature_toggle_backend::judgment::types::Question;
use serde_json::json;

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn live_noul_detects_urgency() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live smoke test");
        return;
    };
    let mut questions = BTreeMap::new();
    questions.insert(
        "is_urgent".to_string(),
        Question::noul("Does this convey urgency?"),
    );

    let response = client
        .evaluate(
            json!("Help! My payouts have been failing for 3 days."),
            questions,
        )
        .await
        .expect("live TypeSafe call failed");

    assert_eq!(response.model, "jev-1.13.0");
    let urgent = response.answers.noul("is_urgent").expect("noul answer");
    assert!(urgent > 0.5, "expected urgency above 0.5, got {urgent}");
}
