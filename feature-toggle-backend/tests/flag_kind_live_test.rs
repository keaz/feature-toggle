//! Tunes the flag kind classifier against the live TypeSafe API. Run with:
//! cargo test -p feature-toggle-backend --test flag_kind_live_test -- --ignored --nocapture
//!
//! Labelled features live in `tests/fixtures/ai/flag_kind.json`. The test
//! prints every answer and a confusion summary, and asserts only a minimum
//! accuracy. Record the result in `docs/ai-judgments/tasks/AI-30-*.md`.

use std::collections::BTreeMap;

use feature_toggle_backend::config::TypesafeConfig;
use feature_toggle_backend::judgment::build_client;
use feature_toggle_backend::judgment::flag_kind;
use serde::Deserialize;

const MIN_ACCURACY: f64 = 0.75;

#[derive(Deserialize)]
struct Case {
    name: String,
    key: String,
    description: Option<String>,
    purpose: Option<String>,
    tags: Vec<String>,
    feature_type: String,
    /// A kind name, or `unknown` when nothing should be stored.
    expected: String,
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn live_flag_kind_accuracy() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live tuning test");
        return;
    };
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/ai/flag_kind.json")).expect("fixture parses");
    assert!(cases.len() >= 25, "need at least 25 labelled cases");

    // expected -> predicted -> count
    let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut correct = 0;
    for case in &cases {
        let input = flag_kind::build_input(
            &case.key,
            case.description.as_deref(),
            case.purpose.as_deref(),
            &case.tags,
            &case.feature_type,
        );
        let parts = flag_kind::build(&input);
        let response = client
            .evaluate(parts.state, parts.questions)
            .await
            .unwrap_or_else(|err| panic!("live call failed for '{}': {err}", case.name));
        let derived = flag_kind::derive(&response.answers);
        let predicted = derived["kind"].as_str().unwrap_or("unknown").to_string();
        let ok = predicted == case.expected;
        println!(
            "{:<11} -> {:<11} {} | {} | confidence {}",
            case.expected,
            predicted,
            if ok { "ok " } else { "BAD" },
            case.name,
            derived["confidence"]
        );
        if ok {
            correct += 1;
        }
        *counts
            .entry(case.expected.clone())
            .or_default()
            .entry(predicted)
            .or_default() += 1;
    }

    println!("\nconfusion (expected -> predicted: count)");
    for (expected, row) in &counts {
        println!("{expected:<11} {row:?}");
    }
    let accuracy = correct as f64 / cases.len() as f64;
    println!("accuracy {correct}/{} = {accuracy:.2}", cases.len());
    assert!(
        accuracy >= MIN_ACCURACY,
        "accuracy {accuracy:.2} is below the minimum {MIN_ACCURACY}"
    );
}

/// One request carries the kind question and one yes/no question per candidate
/// tag (up to 50). Checks the API accepts that shape and ranks a clearly
/// matching tag above clearly unrelated ones.
#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn live_feature_suggestions_accept_fifty_candidate_tags() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live suggestions test");
        return;
    };
    let mut candidates: Vec<String> = ["payments", "billing", "ui", "email", "search"]
        .into_iter()
        .map(String::from)
        .collect();
    candidates.extend((0..45).map(|index| format!("team-{index}")));
    let input = flag_kind::SuggestionInput {
        key: "kill-switch-payments".into(),
        description: Some("Emergency kill switch that stops all payment processing".into()),
        purpose: None,
        tags: vec!["incident".into()],
    };
    let parts = flag_kind::suggestion_parts(&input, &candidates);
    assert_eq!(parts.questions.len(), 51);
    let response = client
        .evaluate(parts.state, parts.questions)
        .await
        .expect("live call failed");
    let kind = flag_kind::classify(&response.answers);
    let tags = flag_kind::suggested_tags(&response.answers, &candidates);
    println!("kind {kind:?}\ntags {tags:?}");
    assert_eq!(kind.map(|answer| answer.value.as_str()), Some("ops"));
    assert!(tags.iter().any(|item| item.tag == "payments"));
    assert!(tags.iter().all(|item| !item.tag.starts_with("team-")));
}
