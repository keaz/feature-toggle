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
async fn enforcing_modes_exclude_only_done_high_risk_with_the_team_setting_on() {
    let pool = init_pg_pool().await;

    // (mode, team setting, judgment as (status, level), expected due).
    // User decision 2026-10-03: require_extra_approver also blocks
    // auto-approval for a done high-risk assessment, like gate_auto_approve.
    let cases: [(&str, Option<bool>, Option<(&str, &str)>, bool); 13] = [
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
            false,
        ),
        (
            "require_extra_approver",
            Some(true),
            Some(("pending", "high")),
            true,
        ),
        (
            "require_extra_approver",
            Some(true),
            Some(("done", "medium")),
            true,
        ),
        (
            "require_extra_approver",
            Some(false),
            Some(("done", "high")),
            true,
        ),
        ("off", Some(true), Some(("done", "high")), true),
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

// --- Eligible approvers who can still vote (known-issue fix 2026-10-03) ---

const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

/// A request under its own team and policy. The policy names `approvers`
/// explicitly; the request lists them all as eligible (as routing would).
struct CapFixture {
    team_id: Uuid,
    feature_id: Uuid,
    policy_id: Uuid,
    request_id: Uuid,
    requester: Uuid,
    approvers: Vec<Uuid>,
}

async fn insert_user(pool: &PgPool, label: &str) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email)
         VALUES ($1, $2, 'x', 'Cap', $3, $4)",
    )
    .bind(user_id)
    .bind(format!("cap_{label}_{user_id}"))
    .bind(label)
    .bind(format!("cap_{label}_{user_id}@example.com"))
    .execute(pool)
    .await
    .unwrap();
    user_id
}

/// A member of `team_id` who holds the seeded "Approver" role.
async fn insert_team_approver(pool: &PgPool, team_id: Uuid) -> Uuid {
    let user_id = insert_user(pool, "approver").await;
    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(team_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(Uuid::parse_str(APPROVER_ROLE_ID).unwrap())
        .execute(pool)
        .await
        .unwrap();
    user_id
}

/// `change_type` is not `stage_change`, so approving executes nothing and the
/// test needs no pipeline stage.
async fn insert_capped_request(
    pool: &PgPool,
    mode: &str,
    policy_required: i32,
    approver_count: usize,
    required_override: Option<i32>,
) -> CapFixture {
    let team_id = insert_team(pool, Some(true)).await;
    let feature_id = feature::feature_repository(pool.clone())
        .create_feature(CreateFeature {
            team_id,
            key: format!("cap-feature-{}", Uuid::new_v4()),
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
    let mut approvers = Vec::new();
    for _ in 0..approver_count {
        approvers.push(insert_team_approver(pool, team_id).await);
    }
    let requester = insert_user(pool, "requester").await;

    let repository = approval::approval_repository(pool.clone());
    let policy = repository
        .create_policy(CreateApprovalPolicyInput {
            team_id,
            name: format!("cap-policy-{}", Uuid::new_v4()),
            description: None,
            applies_to: "all".to_string(),
            environment_ids: None,
            required_approvers: policy_required,
            approver_role_ids: vec![],
            approver_user_ids: approvers.clone(),
            allow_admin_override: false,
            fallback_to_roles: false,
            auto_approve_after_hours: None,
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
            change_type: "cap_test".to_string(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: requester,
            eligible_approver_ids: approvers.clone(),
            routing_reason: None,
            admin_override_enabled: false,
        })
        .await
        .expect("request creation should succeed");
    if let Some(required) = required_override {
        assert!(
            repository
                .set_required_approvers_override(request.id, required)
                .await
                .unwrap()
        );
    }

    CapFixture {
        team_id,
        feature_id,
        policy_id: policy.id,
        request_id: request.id,
        requester,
        approvers,
    }
}

async fn cleanup_cap(pool: &PgPool, fixture: &CapFixture) {
    let _ = sqlx::query("DELETE FROM activity_log WHERE metadata->>'approval_request_id' = $1")
        .bind(fixture.request_id.to_string())
        .execute(pool)
        .await;
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
    let mut users = fixture.approvers.clone();
    users.push(fixture.requester);
    let _ = sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(users)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(fixture.team_id)
        .execute(pool)
        .await;
}

fn logic_for(pool: &PgPool, use_pool: bool) -> Box<dyn approval_logic::ApprovalLogic> {
    let activity_log_repository =
        feature_toggle_backend::database::activity_log::activity_log_repository(pool.clone());
    let environment_logic = environment::environment_logic(
        feature_toggle_backend::database::environment::environment_repository(pool.clone()),
        activity_log_repository.clone_box(),
    );
    let (approval_events_tx, _) = tokio::sync::broadcast::channel::<ApprovalRequestEvent>(16);
    let (feature_updates_tx, _) = tokio::sync::broadcast::channel::<FeatureUpdate>(16);
    if use_pool {
        approval_logic::approval_logic_with_pool(
            pool.clone(),
            approval::approval_repository(pool.clone()),
            feature::feature_repository(pool.clone()).clone_box(),
            environment_logic,
            role::role_repository(pool.clone()),
            approval_events_tx,
            feature_updates_tx,
        )
    } else {
        approval_logic::approval_logic(
            approval::approval_repository(pool.clone()),
            feature::feature_repository(pool.clone()).clone_box(),
            environment_logic,
            role::role_repository(pool.clone()),
            approval_events_tx,
            feature_updates_tx,
        )
    }
}

async fn disable_user(pool: &PgPool, user_id: Uuid) {
    sqlx::query("UPDATE users SET enabled = FALSE WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn ready_ids(pool: &PgPool) -> Vec<Uuid> {
    approval::approval_repository(pool.clone())
        .list_capped_requests_ready_for_approval()
        .await
        .expect("ready query should succeed")
        .into_iter()
        .map(|request| request.id)
        .collect()
}

async fn status_of(pool: &PgPool, request_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM approval_requests WHERE id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Policy 1, override 2, eligible A and B. B loses the Approver role before
/// anyone votes: only A can still vote, so A's vote alone approves.
async fn a_removed_approver_lowers_the_requirement_for_the_next_vote(use_pool: bool) {
    let pool = init_pg_pool().await;
    let fixture = insert_capped_request(&pool, "require_extra_approver", 1, 2, Some(2)).await;
    let (a, b) = (fixture.approvers[0], fixture.approvers[1]);
    sqlx::query("DELETE FROM user_roles WHERE user_id = $1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();

    let remaining = approval::approval_repository(pool.clone())
        .count_remaining_eligible_approvers(vec![fixture.request_id])
        .await
        .unwrap();
    let voted = logic_for(&pool, use_pool)
        .approve_request(fixture.request_id, a, None)
        .await;
    let status = status_of(&pool, fixture.request_id).await;
    cleanup_cap(&pool, &fixture).await;

    assert_eq!(remaining.get(&fixture.request_id), Some(&1));
    let voted = voted.expect("vote should succeed");
    assert_eq!(voted.approved_count, 1);
    assert_eq!(status, "approved");
}

#[tokio::test]
async fn a_removed_approver_lowers_the_requirement_on_the_transaction_vote_path() {
    a_removed_approver_lowers_the_requirement_for_the_next_vote(true).await;
}

#[tokio::test]
async fn a_removed_approver_lowers_the_requirement_on_the_plain_vote_path() {
    a_removed_approver_lowers_the_requirement_for_the_next_vote(false).await;
}

/// Policy 1, override 2, eligible A and B. A approves (still pending: B can
/// vote). Then B is disabled: nobody is left to vote, so the reconciliation
/// approves the request and records why. A second run changes nothing.
#[tokio::test]
async fn reconciliation_approves_a_request_nobody_is_left_to_vote_on() {
    let pool = init_pg_pool().await;
    let fixture = insert_capped_request(&pool, "require_extra_approver", 1, 2, Some(2)).await;
    let (a, b) = (fixture.approvers[0], fixture.approvers[1]);
    let logic = logic_for(&pool, true);

    let first = logic
        .approve_request(fixture.request_id, a, None)
        .await
        .expect("vote should succeed");
    let pending_after_vote = first.status.as_str().to_string();
    let ready_before = ready_ids(&pool).await.contains(&fixture.request_id);

    disable_user(&pool, b).await;
    let ready_after = ready_ids(&pool).await.contains(&fixture.request_id);
    let request = approval::approval_repository(pool.clone())
        .get_request_by_id(fixture.request_id)
        .await
        .unwrap()
        .unwrap();
    let reconciled = logic.approve_capped_request(request.clone()).await;
    let again = logic.approve_capped_request(request).await;
    let status = status_of(&pool, fixture.request_id).await;
    let activity: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT metadata FROM activity_log
         WHERE activity_type = 'approval_requirement_reconciled'
           AND metadata->>'approval_request_id' = $1",
    )
    .bind(fixture.request_id.to_string())
    .fetch_optional(&pool)
    .await
    .unwrap();
    let still_ready = ready_ids(&pool).await.contains(&fixture.request_id);
    cleanup_cap(&pool, &fixture).await;

    assert_eq!(pending_after_vote, "pending", "B could still vote");
    assert!(!ready_before, "not ready while B can still vote");
    assert!(ready_after, "ready once nobody is left to vote");
    let reconciled = reconciled.expect("reconciliation should succeed");
    assert_eq!(
        reconciled.map(|request| request.status.as_str().to_string()),
        Some("approved".to_string())
    );
    assert!(again.expect("second run should succeed").is_none());
    assert_eq!(status, "approved");
    let activity = activity.expect("the reconciliation must record an activity entry");
    assert_eq!(activity["approved_count"], 1);
    assert_eq!(activity["required_approvers_override"], 2);
    assert_eq!(activity["required_approvals_effective"], 1);
    assert!(!still_ready);
}

/// Policy 2, override 3, eligible A, B, C. B and C are disabled, A approves:
/// one approval is below the policy's 2, so the request stays pending and the
/// reconciliation leaves it alone.
#[tokio::test]
async fn the_requirement_never_drops_below_the_policy() {
    let pool = init_pg_pool().await;
    let fixture = insert_capped_request(&pool, "require_extra_approver", 2, 3, Some(3)).await;
    disable_user(&pool, fixture.approvers[1]).await;
    disable_user(&pool, fixture.approvers[2]).await;
    let logic = logic_for(&pool, true);

    let voted = logic
        .approve_request(fixture.request_id, fixture.approvers[0], None)
        .await;
    let ready = ready_ids(&pool).await.contains(&fixture.request_id);
    let request = approval::approval_repository(pool.clone())
        .get_request_by_id(fixture.request_id)
        .await
        .unwrap()
        .unwrap();
    let reconciled = logic.approve_capped_request(request).await;
    let status = status_of(&pool, fixture.request_id).await;
    cleanup_cap(&pool, &fixture).await;

    assert_eq!(
        voted.expect("vote should succeed").status.as_str(),
        "pending"
    );
    assert!(!ready);
    assert!(reconciled.expect("reconciliation should succeed").is_none());
    assert_eq!(status, "pending");
}

/// Advisory (no override): an approver leaving changes nothing. The policy's
/// 2 approvals are still required and the reconciliation ignores the request.
#[tokio::test]
async fn without_an_override_the_requirement_is_unchanged() {
    let pool = init_pg_pool().await;
    let fixture = insert_capped_request(&pool, "advisory", 2, 3, None).await;
    disable_user(&pool, fixture.approvers[2]).await;
    let logic = logic_for(&pool, true);

    let first = logic
        .approve_request(fixture.request_id, fixture.approvers[0], None)
        .await
        .expect("vote should succeed");
    disable_user(&pool, fixture.approvers[1]).await;
    let ready = ready_ids(&pool).await.contains(&fixture.request_id);
    let status = status_of(&pool, fixture.request_id).await;
    cleanup_cap(&pool, &fixture).await;

    assert_eq!(first.status.as_str(), "pending");
    assert!(!ready);
    assert_eq!(status, "pending");
}

/// A closed request is never listed or changed, even when nobody is left.
#[tokio::test]
async fn reconciliation_never_touches_a_closed_request() {
    let pool = init_pg_pool().await;
    let fixture = insert_capped_request(&pool, "require_extra_approver", 1, 2, Some(2)).await;
    let logic = logic_for(&pool, true);
    logic
        .approve_request(fixture.request_id, fixture.approvers[0], None)
        .await
        .expect("vote should succeed");
    let cancelled = logic
        .cancel_request(fixture.request_id, fixture.requester)
        .await
        .expect("cancel should succeed");
    disable_user(&pool, fixture.approvers[1]).await;

    let ready = ready_ids(&pool).await.contains(&fixture.request_id);
    let reconciled = logic.approve_capped_request(cancelled).await;
    let status = status_of(&pool, fixture.request_id).await;
    cleanup_cap(&pool, &fixture).await;

    assert!(!ready);
    assert!(reconciled.expect("reconciliation should succeed").is_none());
    assert_eq!(status, "cancelled");
}
