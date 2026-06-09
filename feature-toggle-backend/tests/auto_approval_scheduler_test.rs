use chrono::Utc;
use feature_toggle_backend::database::approval::MockApprovalRepository;
use feature_toggle_backend::database::entity::{ApprovalRequest, ApprovalStatus};
use feature_toggle_backend::logic::approval::MockApprovalLogic;
use feature_toggle_backend::scheduler::auto_approval::AutoApprovalScheduler;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn auto_approval_scheduler_processes_pending_requests() {
    let mut mock_repo = MockApprovalRepository::new();
    let mut mock_logic = MockApprovalLogic::new();

    let pending_request = ApprovalRequest {
        id: Uuid::new_v4(),
        policy_id: Uuid::new_v4(),
        feature_id: Uuid::new_v4(),
        environment_id: Some(Uuid::new_v4()),
        change_type: "stage_change".into(),
        change_payload: json!({
            "stage_id": "stage-1",
            "next_status": "DEPLOYED",
        }),
        change_description: Some("Auto approve test".into()),
        requested_by: Uuid::new_v4(),
        eligible_approver_ids: Vec::new(),
        routing_reason: None,
        admin_override_enabled: false,
        status: ApprovalStatus::Pending,
        approved_count: 0,
        rejected_count: 0,
        executed_at: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let pending_clone = pending_request.clone();
    mock_repo
        .expect_list_requests_due_for_auto_approval()
        .returning(move || Ok(vec![pending_clone.clone()]));

    mock_logic
        .expect_auto_approve_request()
        .times(1)
        .returning(Ok);

    let scheduler = AutoApprovalScheduler::new(
        Box::new(mock_repo),
        Box::new(mock_logic),
        Duration::from_secs(0),
    );

    scheduler.run_pending().await.unwrap();
}
