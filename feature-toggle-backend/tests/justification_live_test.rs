//! Tunes the justification check against the live TypeSafe API. Run with:
//! cargo test -p feature-toggle-backend --test justification_live_test -- --ignored --nocapture
//!
//! Labelled reasons live in `tests/fixtures/ai/justification.json`. The test
//! prints every answer and a confusion summary, and asserts only a minimum
//! accuracy. Record the result in `docs/ai-judgments/tasks/AI-20-*.md`.

use feature_toggle_backend::config::TypesafeConfig;
use feature_toggle_backend::judgment::build_client;
use feature_toggle_backend::judgment::justification::{self, ReasonKind};
use serde::Deserialize;

const MIN_ACCURACY: f64 = 0.75;

#[derive(Deserialize)]
struct Case {
    name: String,
    reason_kind: ReasonKind,
    #[serde(default)]
    feature_key: Option<String>,
    reason: String,
    expected: String,
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn live_justification_accuracy() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live tuning test");
        return;
    };
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/ai/justification.json"))
        .expect("fixture parses");
    assert!(cases.len() >= 20, "need at least 20 labelled cases");

    // (expected, predicted) -> count
    let mut counts = [[0usize; 2]; 2];
    let index = |verdict: &str| usize::from(verdict == "weak");
    let mut correct = 0;
    for case in &cases {
        let (verdict, detail) = if justification::rule_check(&case.reason) {
            ("ok".to_string(), "rule".to_string())
        } else {
            let parts = justification::build(
                case.reason_kind,
                case.feature_key.as_deref(),
                case.reason.trim(),
            );
            let response = client
                .evaluate(parts.state, parts.questions)
                .await
                .unwrap_or_else(|err| panic!("live call failed for '{}': {err}", case.name));
            let derived = justification::derive(&response.answers);
            (
                derived["verdict"].as_str().unwrap_or("?").to_string(),
                format!(
                    "concrete {} placeholder {}",
                    derived["probability"],
                    response
                        .answers
                        .noul("placeholder_text")
                        .map_or("?".to_string(), |value| format!("{value:.2}"))
                ),
            )
        };
        println!(
            "{:<5} -> {:<5} {} | {} | {}",
            case.expected,
            verdict,
            if verdict == case.expected {
                "ok "
            } else {
                "BAD"
            },
            case.name,
            detail
        );
        if verdict == case.expected {
            correct += 1;
        }
        counts[index(&case.expected)][index(&verdict)] += 1;
    }

    println!("\nconfusion (rows expected, columns predicted)");
    println!("{:<5} {:>5} {:>5}", "", "ok", "weak");
    println!("{:<5} {:>5} {:>5}", "ok", counts[0][0], counts[0][1]);
    println!("{:<5} {:>5} {:>5}", "weak", counts[1][0], counts[1][1]);
    let accuracy = correct as f64 / cases.len() as f64;
    println!("accuracy {correct}/{} = {accuracy:.2}", cases.len());
    assert!(
        accuracy >= MIN_ACCURACY,
        "accuracy {accuracy:.2} is below the minimum {MIN_ACCURACY}"
    );
}
