use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{CreateFeature, CreateFeatureStage};
use feature_toggle_backend::database::{approval, feature, init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::approval::ApprovalRequestEvent;
use feature_toggle_backend::logic::feature::StageChangeRequestType;
use feature_toggle_backend::logic::{
    approval as approval_logic, environment, feature as feature_logic,
};
use feature_toggle_backend::model::ID;
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
    let requester = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
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
}
