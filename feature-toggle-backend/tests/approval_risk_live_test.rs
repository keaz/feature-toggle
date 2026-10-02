//! Tunes the approval risk thresholds against the live TypeSafe API. Run with:
//! cargo test -p feature-toggle-backend --test approval_risk_live_test -- --ignored --nocapture
//!
//! Labelled cases live in `tests/fixtures/ai/approval_risk.json`. The test
//! prints every answer and a confusion summary, and asserts only a minimum
//! accuracy. Record the result in `docs/ai-judgments/tasks/AI-10-*.md`.

use std::collections::BTreeMap;

use chrono::Utc;
use feature_toggle_backend::config::TypesafeConfig;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::approval::approval_repository;
use feature_toggle_backend::database::entity::{Feature, FeaturePipelineStage, FeatureType};
use feature_toggle_backend::judgment::approval_risk::{ApprovalRiskHandler, build_input};
use feature_toggle_backend::judgment::build_client;
use feature_toggle_backend::judgment::service::JudgmentHandler;
use feature_toggle_backend::model::{Environment, ID};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const MIN_ACCURACY: f64 = 0.7;
const LEVELS: [&str; 3] = ["low", "medium", "high"];

#[derive(Deserialize)]
struct Case {
    name: String,
    expected: String,
    feature: FeatureFixture,
    environment: EnvironmentFixture,
    change: ChangeFixture,
    blast_radius: Value,
}

#[derive(Deserialize)]
struct FeatureFixture {
    key: String,
    description: String,
    purpose: String,
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct EnvironmentFixture {
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct ChangeFixture {
    from_status: String,
    to_status: String,
    diff: Value,
}

fn feature(fixture: &FeatureFixture) -> Feature {
    Feature {
        id: Uuid::new_v4(),
        key: fixture.key.clone(),
        description: Some(fixture.description.clone()),
        feature_type: FeatureType::Simple,
        team_id: Uuid::new_v4(),
        active: true,
        created_at: Utc::now(),
        kill_switch_enabled: false,
        kill_switch_activated_at: None,
        rollback_scheduled_at: None,
        emergency_override_reason: None,
        emergency_override_expires_at: None,
        emergency_override_actor_id: None,
        emergency_override_applied_at: None,
        lifecycle_stage: "active".to_string(),
        owner: Some("owner@example.com".to_string()),
        purpose: Some(fixture.purpose.clone()),
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: fixture.tags.clone(),
        archived_at: None,
        deprecated_at: None,
        deprecation_notice: None,
        last_evaluated_at: None,
        evaluation_count_7d: 0,
        evaluation_count_30d: 0,
        evaluation_count_90d: 0,
        dependencies: vec![],
    }
}

#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY"]
async fn live_approval_risk_accuracy() {
    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live tuning test");
        return;
    };
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/ai/approval_risk.json"))
        .expect("fixture parses");
    assert!(cases.len() >= 20, "need at least 20 labelled cases");

    // The handler only reads its repositories in `apply`, which this test skips.
    let pool = sqlx::PgPool::connect_lazy("postgres://unused@localhost/unused").expect("lazy pool");
    let handler = ApprovalRiskHandler::new(
        activity_log_repository(pool.clone()),
        approval_repository(pool),
    );

    let mut confusion: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut correct = 0;
    for case in &cases {
        let environment = Environment {
            id: ID::from(Uuid::new_v4()),
            name: case.environment.name.clone(),
            team_id: ID::from(Uuid::new_v4()),
            active: true,
            environment_type: case.environment.kind.clone(),
        };
        let stage = FeaturePipelineStage {
            id: Uuid::new_v4(),
            feature_id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            order_index: 0,
            parent_stage_id: None,
            position: "production".to_string(),
            enabled: false,
            status: case.change.from_status.clone(),
        };
        let payload = json!({
            "previous_status": case.change.from_status,
            "next_status": case.change.to_status,
            "diff": case.change.diff,
            "blast_radius": case.blast_radius,
        });
        let input = build_input(&feature(&case.feature), &stage, &environment, &payload);
        let parts = handler.build(&input);
        let response = client
            .evaluate(parts.state, parts.questions)
            .await
            .unwrap_or_else(|err| panic!("live call failed for '{}': {err}", case.name));
        let derived = handler.derive(&input, &response.answers);
        let level = derived["level"].as_str().unwrap_or("?").to_string();

        println!(
            "{:<8} -> {:<8} {} | {}",
            case.expected,
            level,
            if level == case.expected { "ok " } else { "BAD" },
            case.name
        );
        println!("         signals {}", derived["signals"]);
        if level == case.expected {
            correct += 1;
        }
        *confusion.entry((case.expected.clone(), level)).or_default() += 1;
    }

    println!("\nconfusion (rows expected, columns predicted)");
    println!(
        "{:<8} {:>5} {:>7} {:>5}",
        "", LEVELS[0], LEVELS[1], LEVELS[2]
    );
    for expected in LEVELS {
        let counts: Vec<usize> = LEVELS
            .iter()
            .map(|predicted| {
                confusion
                    .get(&(expected.to_string(), predicted.to_string()))
                    .copied()
                    .unwrap_or(0)
            })
            .collect();
        println!(
            "{expected:<8} {:>5} {:>7} {:>5}",
            counts[0], counts[1], counts[2]
        );
    }
    let accuracy = correct as f64 / cases.len() as f64;
    println!("accuracy {correct}/{} = {accuracy:.2}", cases.len());
    assert!(
        accuracy >= MIN_ACCURACY,
        "accuracy {accuracy:.2} is below the minimum {MIN_ACCURACY}"
    );
}

/// End to end against the seeded test DB and the live API: creating a
/// stage-change approval request produces a `done` judgment and an activity entry.
/// Needs `DATABASE_URL` for a migrated and seeded DB (`feture_toggle_test`).
#[tokio::test]
#[ignore = "calls the live TypeSafe API; needs TYPESAFE_API_KEY and the seeded test DB"]
async fn live_stage_change_request_gets_a_done_assessment() {
    use std::sync::Arc;
    use std::time::Duration;

    use feature_toggle_backend::database::ai::{
        TeamAiSettings, ai_judgment_repository, team_ai_settings_repository,
    };
    use feature_toggle_backend::database::entity::FeatureType;
    use feature_toggle_backend::database::feature::{
        CreateFeature, CreateFeatureStage, feature_repository,
    };
    use feature_toggle_backend::database::{environment, init_pg_pool, role, user};
    use feature_toggle_backend::judgment::service::JudgmentService;
    use feature_toggle_backend::logic::approval::approval_logic_with_notifications;
    use feature_toggle_backend::logic::feature::{
        StageChangeRequestType, feature_logic_with_approval,
    };

    const TEAM_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
    const POLICY_ENVIRONMENT_ID: &str = "9f9f9f9f-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
    const REQUESTER_ID: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    let Some(client) = build_client(&TypesafeConfig::default()) else {
        eprintln!("TYPESAFE_API_KEY is not set; skipping the live end-to-end test");
        return;
    };
    let pool = init_pg_pool().await;
    let team_id = Uuid::parse_str(TEAM_ID).unwrap();
    let environment_id = Uuid::parse_str(POLICY_ENVIRONMENT_ID).unwrap();
    let requester = Uuid::parse_str(REQUESTER_ID).unwrap();

    let settings_repo = team_ai_settings_repository(pool.clone());
    let previous = settings_repo.get(team_id).await.unwrap().settings;
    settings_repo
        .upsert(
            team_id,
            TeamAiSettings {
                approval_risk: true,
                ..previous.clone()
            },
            None,
        )
        .await
        .unwrap();

    let activity_repo = activity_log_repository(pool.clone());
    let approvals = approval_repository(pool.clone());
    let service = Arc::new(
        JudgmentService::new(
            client,
            ai_judgment_repository(pool.clone()),
            settings_repo.clone_box(),
        )
        .with_handler(Arc::new(ApprovalRiskHandler::new(
            activity_repo.clone_box(),
            approvals.clone_box(),
        ))),
    );

    let features = feature_repository(pool.clone());
    let environment_logic = feature_toggle_backend::logic::environment::environment_logic(
        environment::environment_repository(pool.clone()),
        activity_repo.clone_box(),
    );
    let (approval_events_tx, _approval_events_rx) = tokio::sync::broadcast::channel(16);
    let (updates_tx, _updates_rx) = tokio::sync::broadcast::channel(16);
    // Without a pool the seeded policy's 2-approver routing check is skipped.
    let approval_logic = approval_logic_with_notifications(
        approvals.clone_box(),
        features.clone_box(),
        environment_logic.clone(),
        role::role_repository(pool.clone()),
        approval_events_tx,
        updates_tx,
        None,
        Some(service),
    );
    let feature_logic = feature_logic_with_approval(
        features.clone_box(),
        environment_logic,
        activity_repo.clone_box(),
        user::user_repository(pool.clone()),
        Some(approval_logic),
    );

    let stage_id = Uuid::new_v4();
    let feature_id = features
        .create_feature(CreateFeature {
            team_id,
            key: format!("live-risk-{}", Uuid::new_v4()),
            description: Some("Routes card payments through the new payment processor".into()),
            feature_type: FeatureType::Simple,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: Some("Replace the legacy card processor".into()),
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec!["payments".to_string()],
            stages: vec![CreateFeatureStage {
                id: stage_id,
                environment_id,
                order_index: 0,
                parent_stage: None,
                position: "{ \"x\": 640, \"y\": 240 }".to_string(),
                enabled: true,
            }],
            dependencies: vec![],
            variants: None,
        })
        .await
        .expect("feature setup");
    sqlx::query("UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1")
        .bind(stage_id)
        .execute(&pool)
        .await
        .unwrap();

    let outcome = async {
        let feature = feature_logic
            .request_stage_change(
                ID::from(stage_id),
                StageChangeRequestType::DeploymentRequested,
                requester,
            )
            .await
            .expect("stage change is intercepted by the seeded policy");
        let request_id = feature
            .pending_approval_request_id
            .and_then(|id| Uuid::try_from(id).ok())
            .expect("pending approval id");

        let mut status = String::new();
        for _ in 0..60 {
            status = sqlx::query_scalar(
                "SELECT status FROM ai_judgments \
                 WHERE subject_type = 'approval_request' AND subject_id = $1 AND kind = 'approval_risk'",
            )
            .bind(request_id)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .unwrap_or_default();
            if status == "done" || status == "failed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert_eq!(status, "done", "judgment did not reach done");

        let derived: Value = sqlx::query_scalar(
            "SELECT derived FROM ai_judgments WHERE subject_id = $1 AND kind = 'approval_risk'",
        )
        .bind(request_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        println!("derived: {derived}");
        assert!(["low", "medium", "high"].contains(&derived["level"].as_str().unwrap()));

        // `apply` runs right after the row is marked done.
        let mut activities: i64 = 0;
        for _ in 0..20 {
            activities = sqlx::query_scalar(
                "SELECT COUNT(*) FROM activity_log \
                 WHERE activity_type = 'approval_risk_assessed' AND entity_id = $1",
            )
            .bind(feature_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
            if activities > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert_eq!(activities, 1, "approval_risk_assessed activity");
        request_id
    }
    .await;

    // Cleanup. A failed assertion above skips it; the feature key is unique per run.
    sqlx::query("DELETE FROM ai_judgments WHERE subject_id = $1")
        .bind(outcome)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM activity_log WHERE entity_id = $1")
        .bind(feature_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM approval_votes WHERE request_id = $1")
        .bind(outcome)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM approval_requests WHERE feature_id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await
        .unwrap();
    settings_repo.upsert(team_id, previous, None).await.unwrap();
}
