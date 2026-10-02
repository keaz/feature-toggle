//! AI-11: approval risk enforcement against a real database.

use feature_toggle_backend::database::approval::{
    CreateApprovalPolicyInput, CreateApprovalRequestInput,
};
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::CreateFeature;
use feature_toggle_backend::database::{approval, feature, init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::approval::{self as approval_logic, ApprovalRequestEvent};
use feature_toggle_backend::logic::feature::StageChangeRequestType;
use feature_toggle_backend::logic::{environment, feature as feature_logic};
use feature_toggle_backend::model::ID;
use sqlx::PgPool;
use uuid::Uuid;

const SEEDED_TEAM_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const POLICY_ENVIRONMENT_ID: &str = "9f9f9f9f-aaaa-4aaa-aaaa-aaaaaaaaaaaa";
const SEEDED_REQUESTER: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

struct Fixture {
    team_id: Uuid,
    feature_id: Uuid,
    policy_id: Uuid,
    request_id: Uuid,
}

/// A user on the seeded team who holds the seeded "Approver" role.
async fn insert_approver(pool: &PgPool) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email)
         VALUES ($1, $2, 'x', 'AI11', 'Approver', $3)",
    )
    .bind(user_id)
    .bind(format!("ai11_approver_{user_id}"))
    .bind(format!("ai11_approver_{user_id}@example.com"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(Uuid::parse_str(SEEDED_TEAM_ID).unwrap())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO user_roles (user_id, role_id)
         VALUES ($1, '00000000-0000-0000-0000-000000000001')",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
    user_id
}

async fn insert_team(pool: &PgPool, approval_risk_setting: Option<bool>) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'AI-11 test')")
        .bind(team_id)
        .bind(format!("AI-11 team {team_id}"))
        .execute(pool)
        .await
        .expect("failed to insert team");
    if let Some(enabled) = approval_risk_setting {
        sqlx::query("INSERT INTO team_ai_settings (team_id, approval_risk) VALUES ($1, $2)")
            .bind(team_id)
            .bind(enabled)
            .execute(pool)
            .await
            .expect("failed to insert team ai settings");
    }
    team_id
}

/// A pending request, two hours old, under a policy that auto-approves after one hour.
async fn insert_due_request(
    pool: &PgPool,
    mode: &str,
    approval_risk_setting: Option<bool>,
) -> Fixture {
    let team_id = insert_team(pool, approval_risk_setting).await;
    let feature_id = feature::feature_repository(pool.clone())
        .create_feature(CreateFeature {
            team_id,
            key: format!("ai11-feature-{}", Uuid::new_v4()),
            description: None,
            feature_type: FeatureType::Simple,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            stages: vec![],
            dependencies: vec![],
            variants: None,
            flag_kind: None,
        })
        .await
        .expect("feature setup should succeed");

    let repository = approval::approval_repository(pool.clone());
    let policy = repository
        .create_policy(CreateApprovalPolicyInput {
            team_id,
            name: format!("ai11-policy-{}", Uuid::new_v4()),
            description: None,
            applies_to: "all".to_string(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![],
            approver_user_ids: vec![Uuid::new_v4()],
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: Some(1),
            enabled: true,
            ai_risk_mode: mode.to_string(),
        })
        .await
        .expect("policy creation should succeed");
    let request = repository
        .create_request(CreateApprovalRequestInput {
            policy_id: policy.id,
            feature_id,
            environment_id: None,
            change_type: "stage_change".to_string(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: Uuid::parse_str(SEEDED_REQUESTER).unwrap(),
            eligible_approver_ids: vec![],
            routing_reason: None,
            admin_override_enabled: false,
        })
        .await
        .expect("request creation should succeed");
    sqlx::query(
        "UPDATE approval_requests SET created_at = NOW() - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(request.id)
    .execute(pool)
    .await
    .unwrap();

    Fixture {
        team_id,
        feature_id,
        policy_id: policy.id,
        request_id: request.id,
    }
}

async fn insert_judgment(pool: &PgPool, fixture: &Fixture, status: &str, level: &str) {
    let derived = (status == "done").then(|| serde_json::json!({ "level": level }));
    sqlx::query(
        "INSERT INTO ai_judgments
           (id, team_id, subject_type, subject_id, kind, status, input, input_hash, derived)
         VALUES ($1, $2, 'approval_request', $3, 'approval_risk', $4, '{}'::jsonb, 'h', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.team_id)
    .bind(fixture.request_id)
    .bind(status)
    .bind(derived)
    .execute(pool)
    .await
    .expect("failed to insert judgment");
}

async fn cleanup(pool: &PgPool, fixture: &Fixture) {
    let _ = sqlx::query("DELETE FROM approval_requests WHERE id = $1")
        .bind(fixture.request_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM approval_policies WHERE id = $1")
        .bind(fixture.policy_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(fixture.feature_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(fixture.team_id)
        .execute(pool)
        .await;
}

async fn is_due(pool: &PgPool, request_id: Uuid) -> bool {
    approval::approval_repository(pool.clone())
        .list_requests_due_for_auto_approval()
        .await
        .expect("due query should succeed")
        .iter()
        .any(|request| request.id == request_id)
}

#[tokio::test]
async fn gate_auto_approve_excludes_only_done_high_risk_with_the_team_setting_on() {
    let pool = init_pg_pool().await;

    // (mode, team setting, judgment as (status, level), expected due)
    let cases: [(&str, Option<bool>, Option<(&str, &str)>, bool); 9] = [
        (
            "gate_auto_approve",
            Some(true),
            Some(("done", "high")),
            false,
        ),
        (
            "gate_auto_approve",
            Some(true),
            Some(("pending", "high")),
            true,
        ),
        (
            "gate_auto_approve",
            Some(true),
            Some(("failed", "high")),
            true,
        ),
        ("gate_auto_approve", Some(true), None, true),
        (
            "gate_auto_approve",
            Some(true),
            Some(("done", "medium")),
            true,
        ),
        (
            "gate_auto_approve",
            Some(false),
            Some(("done", "high")),
            true,
        ),
        ("gate_auto_approve", None, Some(("done", "high")), true),
        ("advisory", Some(true), Some(("done", "high")), true),
        (
            "require_extra_approver",
            Some(true),
            Some(("done", "high")),
            true,
        ),
    ];

    for (mode, setting, judgment, expected_due) in cases {
        let fixture = insert_due_request(&pool, mode, setting).await;
        if let Some((status, level)) = judgment {
            insert_judgment(&pool, &fixture, status, level).await;
        }
        let due = is_due(&pool, fixture.request_id).await;
        cleanup(&pool, &fixture).await;
        assert_eq!(
            due, expected_due,
            "mode={mode} setting={setting:?} judgment={judgment:?}"
        );
    }
}

#[tokio::test]
async fn a_gated_request_becomes_due_again_when_the_policy_leaves_gate_mode() {
    let pool = init_pg_pool().await;
    let fixture = insert_due_request(&pool, "gate_auto_approve", Some(true)).await;
    insert_judgment(&pool, &fixture, "done", "high").await;
    assert!(!is_due(&pool, fixture.request_id).await);

    sqlx::query("UPDATE approval_policies SET ai_risk_mode = 'advisory' WHERE id = $1")
        .bind(fixture.policy_id)
        .execute(&pool)
        .await
        .unwrap();
    let due = is_due(&pool, fixture.request_id).await;
    cleanup(&pool, &fixture).await;
    assert!(due);
}

#[tokio::test]
async fn the_override_is_set_once_and_only_on_a_pending_request() {
    let pool = init_pg_pool().await;
    let repository = approval::approval_repository(pool.clone());
    let fixture = insert_due_request(&pool, "require_extra_approver", Some(true)).await;

    let stored = repository
        .get_request_by_id(fixture.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.required_approvers_override, None);

    assert!(
        repository
            .set_required_approvers_override(fixture.request_id, 2)
            .await
            .unwrap()
    );
    assert!(
        !repository
            .set_required_approvers_override(fixture.request_id, 5)
            .await
            .unwrap()
    );
    let stored = repository
        .get_request_by_id(fixture.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.required_approvers_override, Some(2));

    for status in ["approved", "rejected", "cancelled"] {
        sqlx::query(
            "UPDATE approval_requests SET status = $2, required_approvers_override = NULL WHERE id = $1",
        )
        .bind(fixture.request_id)
        .bind(status)
        .execute(&pool)
        .await
        .unwrap();
        let changed = repository
            .set_required_approvers_override(fixture.request_id, 2)
            .await
            .unwrap();
        let stored = repository
            .get_request_by_id(fixture.request_id)
            .await
            .unwrap()
            .unwrap();
        assert!(!changed, "{status} request must not change");
        assert_eq!(stored.required_approvers_override, None, "{status}");
        assert_eq!(stored.status.as_str(), status);
    }

    cleanup(&pool, &fixture).await;
}

/// The seeded policy needs 2 approvals. An override of 3 keeps the request
/// pending after two approvals, on both the transaction and non-transaction vote paths.
async fn two_approvals_leave_an_overridden_request_pending(use_pool: bool) {
    let pool = init_pg_pool().await;
    let feature_repository = feature::feature_repository(pool.clone());
    let activity_log_repository =
        feature_toggle_backend::database::activity_log::activity_log_repository(pool.clone());
    let environment_logic = environment::environment_logic(
        feature_toggle_backend::database::environment::environment_repository(pool.clone()),
        activity_log_repository.clone_box(),
    );
    let approval_repository = approval::approval_repository(pool.clone());
    let role_repository = role::role_repository(pool.clone());
    let (approval_events_tx, _approval_events_rx) =
        tokio::sync::broadcast::channel::<ApprovalRequestEvent>(16);
    let (feature_updates_tx, _feature_updates_rx) =
        tokio::sync::broadcast::channel::<FeatureUpdate>(16);
    let approval_logic = if use_pool {
        approval_logic::approval_logic_with_pool(
            pool.clone(),
            approval_repository.clone(),
            feature_repository.clone_box(),
            environment_logic.clone(),
            role_repository.clone(),
            approval_events_tx,
            feature_updates_tx,
        )
    } else {
        approval_logic::approval_logic(
            approval_repository.clone(),
            feature_repository.clone_box(),
            environment_logic.clone(),
            role_repository.clone(),
            approval_events_tx,
            feature_updates_tx,
        )
    };
    let feature_logic = feature_logic::feature_logic_with_approval(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        feature_toggle_backend::database::user::user_repository(pool.clone()),
        Some(approval_logic.clone()),
    );

    let stage_id = Uuid::new_v4();
    let feature_id = feature_repository
        .create_feature(CreateFeature {
            team_id: Uuid::parse_str(SEEDED_TEAM_ID).unwrap(),
            key: format!("ai11-vote-feature-{}", Uuid::new_v4()),
            description: None,
            feature_type: FeatureType::Simple,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
            stages: vec![
                feature_toggle_backend::database::feature::CreateFeatureStage {
                    id: stage_id,
                    environment_id: Uuid::parse_str(POLICY_ENVIRONMENT_ID).unwrap(),
                    order_index: 0,
                    parent_stage: None,
                    position: "{ \"x\": 640, \"y\": 240 }".to_string(),
                    enabled: true,
                },
            ],
            dependencies: vec![],
            variants: None,
            flag_kind: None,
        })
        .await
        .expect("feature setup should succeed");
    let requester = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email)
         VALUES ($1, $2, 'x', 'AI11', 'Requester', $3)",
    )
    .bind(requester)
    .bind(format!("ai11_requester_{requester}"))
    .bind(format!("ai11_requester_{requester}@example.com"))
    .execute(&pool)
    .await
    .unwrap();
    // Fresh approvers: the seeded ones are not all members of the policy's team,
    // which the pool-based logic requires.
    let approver_one = insert_approver(&pool).await;
    let approver_two = insert_approver(&pool).await;

    sqlx::query("UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1")
        .bind(stage_id)
        .execute(&pool)
        .await
        .unwrap();
    let feature = feature_logic
        .request_stage_change(
            ID::from(stage_id),
            StageChangeRequestType::DeploymentRequested,
            requester,
        )
        .await
        .expect("stage change should be intercepted by approval policy");
    let request_id = feature
        .pending_approval_request_id
        .clone()
        .and_then(|id| Uuid::try_from(id).ok())
        .expect("pending approval id should be set");

    assert!(
        approval_repository
            .set_required_approvers_override(request_id, 3)
            .await
            .unwrap()
    );

    let first = approval_logic
        .approve_request(request_id, approver_one, None)
        .await
        .unwrap();
    assert_eq!(first.status.as_str(), "pending");
    let second = approval_logic
        .approve_request(request_id, approver_two, None)
        .await
        .unwrap();
    assert_eq!(second.approved_count, 2);
    assert_eq!(
        second.status.as_str(),
        "pending",
        "override of 3 must outlast the policy's 2 approvals"
    );

    let _ = sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM approval_requests WHERE requested_by = $1")
        .bind(requester)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![requester, approver_one, approver_two])
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn override_keeps_the_request_pending_through_the_transaction_vote_path() {
    two_approvals_leave_an_overridden_request_pending(true).await;
}

#[tokio::test]
async fn override_keeps_the_request_pending_through_the_plain_vote_path() {
    two_approvals_leave_an_overridden_request_pending(false).await;
}
