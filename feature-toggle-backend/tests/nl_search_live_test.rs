//! Tunes natural-language search against the live TypeSafe API. Run with:
//! cargo test -p feature-toggle-backend --test nl_search_live_test -- --ignored --nocapture
//!
//! Needs `TYPESAFE_API_KEY` and `DATABASE_URL` (a migrated database). The test
//! creates a fixture team with the features in `tests/fixtures/ai/nl_search.json`,
//! runs every query through the production search path, prints each result, and
//! removes the team again. It asserts only a minimum filter accuracy. Record the
//! result in `docs/ai-judgments/tasks/AI-40-*.md`.

use std::collections::{BTreeMap, BTreeSet};

use feature_toggle_backend::config::TypesafeConfig;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::environment::environment_repository;
use feature_toggle_backend::database::feature::{CreateFeature, feature_repository};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::user::user_repository;
use feature_toggle_backend::judgment::build_client;
use feature_toggle_backend::judgment::nl_search::{self, AppliedFilters};
use feature_toggle_backend::logic::environment::environment_logic;
use feature_toggle_backend::logic::feature::feature_logic;
use feature_toggle_backend::model::{FlagKind, FlagKindFilter, LifecycleStage};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const MIN_FILTER_ACCURACY: f64 = 0.75;

#[derive(Deserialize)]
struct Fixture {
    features: Vec<FixtureFeature>,
    queries: Vec<FixtureQuery>,
}

#[derive(Deserialize)]
struct FixtureFeature {
    key: String,
    description: String,
    purpose: Option<String>,
    tags: Vec<String>,
    owner: String,
    lifecycle_stage: String,
    feature_type: String,
    flag_kind: Option<String>,
    #[serde(default)]
    stale: bool,
    #[serde(default)]
    expired: bool,
    evaluations_30d: i64,
}

#[derive(Deserialize)]
struct FixtureQuery {
    query: String,
    /// Filters the query must produce, by snake_case name.
    expected: BTreeMap<String, Value>,
    /// Filter names that may appear in addition to `expected`.
    #[serde(default)]
    optional: Vec<String>,
    /// Whether the query names a topic (so the rerank call runs).
    topic: bool,
    /// The exact set of result keys when the filters are right.
    keys_exact: Option<Vec<String>>,
    /// The first result must be one of these when a topic query returns results.
    top_any: Option<Vec<String>>,
}

fn applied_as_json(filters: &AppliedFilters) -> BTreeMap<String, Value> {
    let mut applied = BTreeMap::new();
    if let Some(stage) = filters.lifecycle_stage {
        let name = match stage {
            LifecycleStage::Draft => "draft",
            LifecycleStage::Active => "active",
            LifecycleStage::Deprecated => "deprecated",
            LifecycleStage::Archived => "archived",
        };
        applied.insert("lifecycle_stage".to_string(), json!(name));
    }
    if let Some(stale) = filters.stale {
        applied.insert("stale".into(), json!(stale));
    }
    if let Some(expired) = filters.expired {
        applied.insert("expired".into(), json!(expired));
    }
    if let Some(feature_type) = filters.feature_type {
        let name = match feature_type {
            feature_toggle_backend::model::FeatureType::Simple => "simple",
            feature_toggle_backend::model::FeatureType::Contextual => "contextual",
        };
        applied.insert("feature_type".into(), json!(name));
    }
    if let Some(value) = &filters.dependency_status {
        applied.insert("dependency_status".into(), json!(value));
    }
    if let Some(value) = &filters.approval_status {
        applied.insert("approval_status".into(), json!(value));
    }
    if let Some(kind) = filters.flag_kind {
        let name = match kind {
            FlagKindFilter::Kind(kind) => kind.as_str(),
            FlagKindFilter::Unclassified => "unclassified",
        };
        applied.insert("flag_kind".into(), json!(name));
    }
    if let Some(value) = &filters.tag {
        applied.insert("tag".into(), json!(value));
    }
    if let Some(value) = &filters.owner {
        applied.insert("owner".into(), json!(value));
    }
    applied
}

fn filters_match(query: &FixtureQuery, applied: &BTreeMap<String, Value>) -> bool {
    query
        .expected
        .iter()
        .all(|(name, value)| applied.get(name) == Some(value))
        && applied
            .keys()
            .all(|name| query.expected.contains_key(name) || query.optional.contains(name))
}

async fn create_fixture_team(pool: &sqlx::PgPool, features: &[FixtureFeature]) -> Uuid {
    let repo = feature_repository(pool.clone());
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("nl-search-live-{team_id}"))
        .bind("fixture team for the live NL search test")
        .execute(pool)
        .await
        .expect("insert team");
    for item in features {
        let id = repo
            .create_feature(CreateFeature {
                team_id,
                key: item.key.clone(),
                description: Some(item.description.clone()),
                feature_type: match item.feature_type.as_str() {
                    "Contextual" => FeatureType::Contextual,
                    _ => FeatureType::Simple,
                },
                lifecycle_stage: item.lifecycle_stage.clone(),
                owner: Some(item.owner.clone()),
                purpose: item.purpose.clone(),
                reference_url: None,
                expires_at: None,
                cleanup_reason: None,
                tags: item.tags.clone(),
                flag_kind: item
                    .flag_kind
                    .as_deref()
                    .map(|kind| kind.parse::<FlagKind>().expect("fixture kind is valid")),
                stages: vec![],
                dependencies: vec![],
                variants: None,
            })
            .await
            .expect("create fixture feature");
        sqlx::query(
            r#"UPDATE features SET
                 evaluation_count_30d = $2,
                 evaluation_count_90d = $2,
                 created_at = CASE WHEN $3 THEN NOW() - INTERVAL '100 days' ELSE created_at END,
                 expires_at = CASE WHEN $4 THEN NOW() - INTERVAL '2 days' ELSE expires_at END,
                 archived_at = CASE WHEN lifecycle_stage = 'archived' THEN NOW() ELSE archived_at END,
                 last_evaluated_at = CASE WHEN $2 > 0 THEN NOW() ELSE NULL END
               WHERE id = $1"#,
        )
        .bind(id)
        .bind(item.evaluations_30d)
        .bind(item.stale)
        .bind(item.expired)
        .execute(pool)
        .await
        .expect("update fixture feature");
    }
    team_id
}

async fn delete_fixture_team(pool: &sqlx::PgPool, team_id: Uuid) {
    let _ = sqlx::query("DELETE FROM features WHERE team_id = $1")
        .bind(team_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await;
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY and DATABASE_URL"]
async fn live_nl_search_accuracy() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live tuning test");
        return;
    };
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/ai/nl_search.json")).expect("fixture parses");
    assert!(fixture.queries.len() >= 20, "need at least 20 queries");

    let pool = init_pg_pool().await;
    let team_id = create_fixture_team(&pool, &fixture.features).await;
    let features = feature_repository(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let logic = feature_logic(
        features.clone_box(),
        environment_logic(environment_repository(pool.clone()), activity.clone_box()),
        activity.clone_box(),
        user_repository(pool.clone()),
    );

    let mut filter_ok = 0;
    let mut topic_ok = 0;
    let mut key_checks = 0;
    let mut key_ok = 0;
    let mut total_calls = 0;
    let mut max_calls = 0;
    let mut tokens = 0;
    for query in &fixture.queries {
        let outcome = nl_search::search(
            client.as_ref(),
            features.as_ref(),
            logic.as_ref(),
            team_id,
            &query.query,
            nl_search::DEFAULT_LIMIT,
        )
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(err) => {
                println!("ERR  {} | {err:?}", query.query);
                continue;
            }
        };
        total_calls += outcome.calls;
        max_calls = max_calls.max(outcome.calls);
        tokens += outcome.input_tokens;
        let applied = applied_as_json(&outcome.filters);
        let filters_good = filters_match(query, &applied);
        let topic_good = (outcome.calls == 2) == query.topic;
        filter_ok += usize::from(filters_good);
        topic_ok += usize::from(topic_good);

        let result_keys: Vec<&str> = outcome
            .results
            .iter()
            .map(|(feature, _)| feature.key.as_str())
            .collect();
        let mut key_note = String::new();
        if filters_good {
            if let Some(exact) = &query.keys_exact {
                key_checks += 1;
                let wanted: BTreeSet<&str> = exact.iter().map(String::as_str).collect();
                let got: BTreeSet<&str> = result_keys.iter().copied().collect();
                let good = wanted == got;
                key_ok += usize::from(good);
                key_note = format!(" keys:{}", if good { "ok" } else { "BAD" });
            }
            if let Some(top) = &query.top_any {
                key_checks += 1;
                let good = result_keys
                    .first()
                    .is_some_and(|first| top.iter().any(|key| key == first));
                key_ok += usize::from(good);
                key_note = format!(" top:{}", if good { "ok" } else { "BAD" });
            }
        }
        if std::env::var("NL_SEARCH_DEBUG").is_ok() {
            for (id, answer) in &outcome.filter_answers.0 {
                println!("    {id}: {}", serde_json::to_string(answer).unwrap());
            }
        }
        println!(
            "filters:{} topic:{}{} | {} | applied {} | results {:?}",
            if filters_good { "ok " } else { "BAD" },
            if topic_good { "ok " } else { "BAD" },
            key_note,
            query.query,
            serde_json::to_string(&applied).unwrap(),
            result_keys,
        );
    }

    delete_fixture_team(&pool, team_id).await;

    let total = fixture.queries.len();
    let accuracy = filter_ok as f64 / total as f64;
    println!(
        "\nfilters {filter_ok}/{total} = {accuracy:.2}; topic {topic_ok}/{total}; result checks {key_ok}/{key_checks}; calls {total_calls} (max {max_calls}); input tokens {tokens}"
    );
    assert!(max_calls <= 2, "a search made more than two calls");
    assert!(
        accuracy >= MIN_FILTER_ACCURACY,
        "filter accuracy {accuracy:.2} is below the minimum {MIN_FILTER_ACCURACY}"
    );
}
