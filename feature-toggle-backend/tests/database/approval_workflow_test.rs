use feature_toggle_backend::database::approval::CreateApprovalRequestInput;
use feature_toggle_backend::database::entity::{ApprovalStatus, FeatureType};
use feature_toggle_backend::database::feature::{CreateFeature, CreateFeatureStage};
use feature_toggle_backend::database::{approval, feature, init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::approval::ApprovalRequestEvent;
use feature_toggle_backend::logic::feature::StageChangeRequestType;
use feature_toggle_backend::logic::{
    approval as approval_logic, environment, feature as feature_logic,
};
use feature_toggle_backend::model::{ID, StageChangeMeta};
use uuid::Uuid;

const TEAM_ID: &str = "51ecc366-f1cd-4d3d-ab73-fa60bad98f27";
const POLICY_ENVIRONMENT_ID: &str = "9f9f9f9f-aaaa-4aaa-aaaa-aaaaaaaaaaaa";

async fn create_isolated_feature_stage(
    repository: &dyn feature::FeatureRepository,
) -> (Uuid, Uuid) {
    let team_id = Uuid::parse_str(TEAM_ID).unwrap();
    let environment_id = Uuid::parse_str(POLICY_ENVIRONMENT_ID).unwrap();
    let stage_id = Uuid::new_v4();

    let feature_id = repository
        .create_feature(CreateFeature {
            team_id,
            key: format!("approval-workflow-feature-{}", Uuid::new_v4()),
            description: Some("Feature for isolated approval workflow tests".to_string()),
            feature_type: FeatureType::Simple,
            lifecycle_stage: "active".to_string(),
            owner: None,
            purpose: None,
            reference_url: None,
            expires_at: None,
            cleanup_reason: None,
            tags: vec![],
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
            flag_kind: None,
        })
        .await
        .expect("feature setup should succeed");

    (feature_id, stage_id)
}

#[tokio::test]
async fn test_stage_change_creates_approval_request_when_policy_exists() {
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
    let approval_logic = approval_logic::approval_logic(
        approval_repository.clone(),
        feature_repository.clone_box(),
        environment_logic.clone(),
        role_repository.clone(),
        approval_events_tx.clone(),
        feature_updates_tx.clone(),
    );
    let feature_logic = feature_logic::feature_logic_with_approval(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        feature_toggle_backend::database::user::user_repository(pool.clone()),
        Some(approval_logic.clone()),
    );

    let (feature_id, stage_id) = create_isolated_feature_stage(feature_repository.as_ref()).await;
    let requester = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();

    // Reset stage status to a pending state for deterministic transition
    sqlx::query!(
        "UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1",
        stage_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let feature = feature_logic
        .request_stage_change(
            ID::from(stage_id),
            StageChangeRequestType::DeploymentRequested,
            requester,
            StageChangeMeta::default(),
        )
        .await
        .expect("stage change should be intercepted by approval policy");

    let request_id = feature
        .pending_approval_request_id
        .clone()
        .and_then(|id| Uuid::try_from(id).ok())
        .expect("pending approval id should be set");

    // Stage should remain unchanged until approvals are collected
    let status: String =
        sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE id = $1")
            .bind(stage_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "DEPLOYMENT_REQUESTED");

    // The approval request should be persisted
    let stored = approval_repository
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .expect("request should exist");
    assert_eq!(stored.approved_count, 0);
    assert_eq!(stored.rejected_count, 0);
    assert_eq!(stored.status.as_str(), "pending");

    // Cleanup test feature/stage rows.
    let _ = sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_stage_change_without_approval_logic_transitions_directly() {
    let pool = init_pg_pool().await;
    let feature_repository = feature::feature_repository(pool.clone());
    let activity_log_repository =
        feature_toggle_backend::database::activity_log::activity_log_repository(pool.clone());
    let environment_logic = environment::environment_logic(
        feature_toggle_backend::database::environment::environment_repository(pool.clone()),
        activity_log_repository.clone_box(),
    );
    let feature_logic = feature_logic::feature_logic(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        feature_toggle_backend::database::user::user_repository(pool.clone()),
    );

    let (feature_id, stage_id) = create_isolated_feature_stage(feature_repository.as_ref()).await;
    let requester = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();

    sqlx::query!(
        "UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1",
        stage_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let feature = feature_logic
        .request_stage_change(
            ID::from(stage_id),
            StageChangeRequestType::DeploymentRequested,
            requester,
            StageChangeMeta::default(),
        )
        .await
        .expect("stage change should transition directly without approval logic");

    assert!(
        feature.pending_approval_request_id.is_none(),
        "direct path should not create an approval request"
    );

    let status: String =
        sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE id = $1")
            .bind(stage_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "DEPLOYMENT_REQUESTED");

    let pending_requests: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM approval_requests WHERE feature_id = $1 AND status = 'pending'",
    )
    .bind(feature_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending_requests, 0);

    let _ = sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_quorum_approvals_execute_stage_change() {
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
    let approval_logic = approval_logic::approval_logic(
        approval_repository.clone(),
        feature_repository.clone_box(),
        environment_logic.clone(),
        role_repository.clone(),
        approval_events_tx.clone(),
        feature_updates_tx.clone(),
    );
    let feature_logic = feature_logic::feature_logic_with_approval(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        feature_toggle_backend::database::user::user_repository(pool.clone()),
        Some(approval_logic.clone()),
    );

    let (feature_id, stage_id) = create_isolated_feature_stage(feature_repository.as_ref()).await;
    // Requesters never count as approvers, so the requester must be someone other
    // than the two seeded approvers.
    let requester = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email)
         VALUES ($1, $2, 'x', 'Quorum', 'Requester', $3)",
    )
    .bind(requester)
    .bind(format!("quorum_requester_{requester}"))
    .bind(format!("quorum_requester_{requester}@example.com"))
    .execute(&pool)
    .await
    .unwrap();
    let approver_one = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let approver_two = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();

    sqlx::query!(
        "UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1",
        stage_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let feature = feature_logic
        .request_stage_change(
            ID::from(stage_id),
            StageChangeRequestType::DeploymentRequested,
            requester,
            StageChangeMeta::default(),
        )
        .await
        .expect("stage change should be intercepted by approval policy");

    let request_id = feature
        .pending_approval_request_id
        .clone()
        .and_then(|id| Uuid::try_from(id).ok())
        .expect("pending approval id should be set");

    let first_vote = approval_logic
        .approve_request(request_id, approver_one, Some("First sign-off".into()))
        .await
        .unwrap();
    assert_eq!(first_vote.approved_count, 1);
    assert_eq!(first_vote.status.as_str(), "pending");

    let second_vote = approval_logic
        .approve_request(request_id, approver_two, Some("Second sign-off".into()))
        .await
        .unwrap();
    assert_eq!(second_vote.approved_count, 2);
    assert_eq!(second_vote.status.as_str(), "approved");

    let status: String =
        sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE id = $1")
            .bind(stage_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "DEPLOYMENT_APPROVED");

    // Cleanup test feature/stage rows.
    let _ = sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM approval_requests WHERE requested_by = $1")
        .bind(requester)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(requester)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_policy_ai_risk_mode_round_trips_and_is_kept_when_absent_on_update() {
    use feature_toggle_backend::database::approval::{
        CreateApprovalPolicyInput, UpdateApprovalPolicyInput,
    };

    let pool = init_pg_pool().await;
    let repository = approval::approval_repository(pool.clone());
    let team_id = Uuid::parse_str(TEAM_ID).unwrap();

    let created = repository
        .create_policy(CreateApprovalPolicyInput {
            team_id,
            name: format!("ai-risk-mode-{}", Uuid::new_v4()),
            description: None,
            applies_to: "all".to_string(),
            environment_ids: None,
            required_approvers: 1,
            approver_role_ids: vec![],
            approver_user_ids: vec![Uuid::new_v4()],
            allow_admin_override: false,
            fallback_to_roles: true,
            auto_approve_after_hours: None,
            enabled: false,
            ai_risk_mode: "advisory".to_string(),
        })
        .await
        .expect("policy creation should succeed");
    assert_eq!(created.ai_risk_mode, "advisory");

    let renamed = repository
        .update_policy(
            created.id,
            UpdateApprovalPolicyInput {
                name: Some("renamed".to_string()),
                description: None,
                applies_to: None,
                environment_ids: None,
                required_approvers: None,
                approver_role_ids: None,
                approver_user_ids: None,
                allow_admin_override: None,
                fallback_to_roles: None,
                auto_approve_after_hours: None,
                enabled: None,
                ai_risk_mode: None,
            },
        )
        .await
        .expect("update without a mode should succeed");
    assert_eq!(renamed.ai_risk_mode, "advisory");

    let changed = repository
        .update_policy(
            created.id,
            UpdateApprovalPolicyInput {
                name: None,
                description: None,
                applies_to: None,
                environment_ids: None,
                required_approvers: None,
                approver_role_ids: None,
                approver_user_ids: None,
                allow_admin_override: None,
                fallback_to_roles: None,
                auto_approve_after_hours: None,
                enabled: None,
                ai_risk_mode: Some("off".to_string()),
            },
        )
        .await
        .expect("update with a mode should succeed");
    assert_eq!(changed.ai_risk_mode, "off");
    assert!(
        repository
            .get_policy_by_id(created.id)
            .await
            .unwrap()
            .is_some_and(|policy| policy.ai_risk_mode == "off")
    );

    let invalid = repository
        .update_policy(
            created.id,
            UpdateApprovalPolicyInput {
                name: None,
                description: None,
                applies_to: None,
                environment_ids: None,
                required_approvers: None,
                approver_role_ids: None,
                approver_user_ids: None,
                allow_admin_override: None,
                fallback_to_roles: None,
                auto_approve_after_hours: None,
                enabled: None,
                ai_risk_mode: Some("bogus".to_string()),
            },
        )
        .await;
    assert!(
        invalid.is_err(),
        "the CHECK constraint rejects unknown modes"
    );

    repository.delete_policy(created.id).await.unwrap();
}

#[tokio::test]
async fn test_stage_change_stores_external_ref_and_reason_on_the_approval_request() {
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
    let approval_logic = approval_logic::approval_logic(
        approval_repository.clone(),
        feature_repository.clone_box(),
        environment_logic.clone(),
        role_repository.clone(),
        approval_events_tx.clone(),
        feature_updates_tx.clone(),
    );
    let feature_logic = feature_logic::feature_logic_with_approval(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        feature_toggle_backend::database::user::user_repository(pool.clone()),
        Some(approval_logic.clone()),
    );

    let (feature_id, stage_id) = create_isolated_feature_stage(feature_repository.as_ref()).await;
    let requester = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();

    sqlx::query!(
        "UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1",
        stage_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let feature = feature_logic
        .request_stage_change(
            ID::from(stage_id),
            StageChangeRequestType::DeploymentRequested,
            requester,
            StageChangeMeta {
                external_ref: Some("PROJ-123".to_string()),
                reason: Some("Ready for QA".to_string()),
                check_reason: false,
            },
        )
        .await
        .expect("stage change should be intercepted by approval policy");
    let request_id = feature
        .pending_approval_request_id
        .clone()
        .and_then(|id| Uuid::try_from(id).ok())
        .expect("pending approval id should be set");

    let stored = approval_repository
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .expect("request should exist");
    assert_eq!(stored.external_ref.as_deref(), Some("PROJ-123"));
    assert_eq!(stored.request_reason.as_deref(), Some("Ready for QA"));

    let (items, _) = approval_repository
        .list_requests_for_team_with_offset(
            Some(Uuid::parse_str(TEAM_ID).unwrap()),
            Some(vec![ApprovalStatus::Pending]),
            0,
            100,
        )
        .await
        .unwrap();
    let listed = items
        .iter()
        .find(|item| item.id == request_id)
        .expect("request should be listed");
    assert_eq!(listed.external_ref.as_deref(), Some("PROJ-123"));
    assert_eq!(listed.request_reason.as_deref(), Some("Ready for QA"));

    let _ = sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_approval_request_without_external_ref_reads_back_none() {
    let pool = init_pg_pool().await;
    let feature_repository = feature::feature_repository(pool.clone());
    let approval_repository = approval::approval_repository(pool.clone());
    let (feature_id, _stage_id) = create_isolated_feature_stage(feature_repository.as_ref()).await;
    let policy_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM approval_policies WHERE team_id = $1 AND enabled LIMIT 1",
    )
    .bind(Uuid::parse_str(TEAM_ID).unwrap())
    .fetch_one(&pool)
    .await
    .expect("seeded policy");

    let created = approval_repository
        .create_request(CreateApprovalRequestInput {
            policy_id,
            feature_id,
            environment_id: None,
            change_type: "stage_change".to_string(),
            change_payload: serde_json::json!({}),
            change_description: None,
            requested_by: Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap(),
            eligible_approver_ids: vec![],
            routing_reason: None,
            admin_override_enabled: false,
            external_ref: None,
            request_reason: None,
        })
        .await
        .expect("request creation should succeed");
    assert_eq!(created.external_ref, None);
    assert_eq!(created.request_reason, None);

    let stored = approval_repository
        .get_request_by_id(created.id)
        .await
        .unwrap()
        .expect("request should exist");
    assert_eq!(stored.external_ref, None);
    assert_eq!(stored.request_reason, None);

    let _ = sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn test_direct_stage_change_records_external_ref_and_reason_in_activity() {
    let pool = init_pg_pool().await;
    let feature_repository = feature::feature_repository(pool.clone());
    let activity_log_repository =
        feature_toggle_backend::database::activity_log::activity_log_repository(pool.clone());
    let environment_logic = environment::environment_logic(
        feature_toggle_backend::database::environment::environment_repository(pool.clone()),
        activity_log_repository.clone_box(),
    );
    let feature_logic = feature_logic::feature_logic(
        feature_repository.clone_box(),
        environment_logic.clone(),
        activity_log_repository.clone_box(),
        feature_toggle_backend::database::user::user_repository(pool.clone()),
    );

    let (feature_id, stage_id) = create_isolated_feature_stage(feature_repository.as_ref()).await;
    let requester = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();

    sqlx::query!(
        "UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1",
        stage_id
    )
    .execute(&pool)
    .await
    .unwrap();

    feature_logic
        .request_stage_change(
            ID::from(stage_id),
            StageChangeRequestType::DeploymentRequested,
            requester,
            StageChangeMeta {
                external_ref: Some("PROJ-456".to_string()),
                reason: Some("Ticket moved to Ready".to_string()),
                check_reason: false,
            },
        )
        .await
        .expect("stage change should apply directly");

    let metadata: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM activity_log WHERE entity_id = $1 AND activity_type = 'stage_change_requested'",
    )
    .bind(stage_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("activity row");
    assert_eq!(metadata["external_ref"], "PROJ-456");
    assert_eq!(metadata["reason"], "Ticket moved to Ready");

    let _ = sqlx::query!("DELETE FROM features WHERE id = $1", feature_id)
        .execute(&pool)
        .await;
}
