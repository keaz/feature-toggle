//! JI-40: activity rows for approval decisions, cancel and gated requests,
//! against the seeded test DB. Each test builds its own team, environments,
//! users and feature, and removes them at the end.

use feature_toggle_backend::database::activity_log::{ActivityLogRow, activity_log_repository};
use feature_toggle_backend::database::approval::approval_repository;
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{
    CreateFeature, CreateFeatureStage, feature_repository,
};
use feature_toggle_backend::database::{init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::approval::{
    ApprovalLogic, ApprovalRequestEvent, ExternalApproval, ExternalApprovalResult,
    approval_logic_with_pool,
};
use feature_toggle_backend::logic::environment;
use feature_toggle_backend::logic::feature::{
    FeatureLogic, StageChangeRequestType, feature_logic_with_approval,
};
use feature_toggle_backend::model::{ID, StageChangeMeta};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

struct Fixture {
    pool: PgPool,
    team_id: Uuid,
    prod_name: String,
    feature_id: Uuid,
    feature_key: String,
    stage_id: Uuid,
    requester_id: Uuid,
    requester_name: String,
    approver_id: Uuid,
    approver_name: String,
    approval_logic: Box<dyn ApprovalLogic>,
    feature_logic: Box<dyn FeatureLogic>,
}

async fn insert_user(pool: &PgPool, team_id: Uuid, role: bool) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let username = format!("approval_activity_{id}");
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin) \
         VALUES ($1, $2, 'x', 'Approval', 'Activity', $3, false)",
    )
    .bind(id)
    .bind(&username)
    .bind(format!("{username}@example.com"))
    .execute(pool)
    .await
    .expect("insert user");
    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(id)
        .bind(team_id)
        .execute(pool)
        .await
        .expect("insert membership");
    if role {
        sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
            .bind(id)
            .bind(Uuid::parse_str(APPROVER_ROLE_ID).unwrap())
            .execute(pool)
            .await
            .expect("insert role");
    }
    (id, username)
}

impl Fixture {
    async fn new() -> Self {
        let pool = init_pg_pool().await;
        let team_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO teams (id, name, description) VALUES ($1, $2, 'approval activity test')",
        )
        .bind(team_id)
        .bind(format!("approval-activity-{team_id}"))
        .execute(&pool)
        .await
        .expect("insert team");
        let mut env_ids = Vec::new();
        let mut prod_name = String::new();
        for (name, kind) in [("Dev", "Development"), ("Prod", "Production")] {
            let id = Uuid::new_v4();
            let name = format!("{name}-{id}");
            sqlx::query(
                "INSERT INTO environments (id, name, active, team_id, environment_type) \
                 VALUES ($1, $2, TRUE, $3, $4)",
            )
            .bind(id)
            .bind(&name)
            .bind(team_id)
            .bind(kind)
            .execute(&pool)
            .await
            .expect("insert environment");
            prod_name = name;
            env_ids.push(id);
        }
        let prod = env_ids[1];
        sqlx::query(
            "INSERT INTO approval_policies (team_id, name, applies_to, environment_ids, \
             required_approvers, approver_role_ids, enabled) \
             VALUES ($1, 'Prod approvals', 'specific_environments', $2, 1, $3, TRUE)",
        )
        .bind(team_id)
        .bind(vec![prod])
        .bind(vec![Uuid::parse_str(APPROVER_ROLE_ID).unwrap()])
        .execute(&pool)
        .await
        .expect("insert policy");

        let (requester_id, requester_name) = insert_user(&pool, team_id, false).await;
        let (approver_id, approver_name) = insert_user(&pool, team_id, true).await;

        let repository = feature_repository(pool.clone());
        let stage_id = Uuid::new_v4();
        let feature_key = format!("approval-activity-{}", Uuid::new_v4());
        let feature_id = repository
            .create_feature(CreateFeature {
                team_id,
                key: feature_key.clone(),
                description: None,
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
                    environment_id: prod,
                    order_index: 0,
                    parent_stage: None,
                    position: "{ \"x\": 0, \"y\": 0 }".to_string(),
                    enabled: false,
                }],
                dependencies: vec![],
                variants: None,
                flag_kind: None,
            })
            .await
            .expect("create feature");
        sqlx::query("UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1")
            .bind(stage_id)
            .execute(&pool)
            .await
            .expect("reset stage");

        let activity = activity_log_repository(pool.clone());
        let environment_logic = environment::environment_logic(
            feature_toggle_backend::database::environment::environment_repository(pool.clone()),
            activity.clone_box(),
        );
        let (approval_events_tx, _) = broadcast::channel::<ApprovalRequestEvent>(64);
        let (updates_tx, _) = broadcast::channel::<FeatureUpdate>(64);
        let approval_logic = approval_logic_with_pool(
            pool.clone(),
            approval_repository(pool.clone()),
            feature_repository(pool.clone()),
            environment_logic.clone(),
            role::role_repository(pool.clone()),
            approval_events_tx,
            updates_tx,
        );
        let feature_logic = feature_logic_with_approval(
            feature_repository(pool.clone()),
            environment_logic,
            activity,
            feature_toggle_backend::database::user::user_repository(pool.clone()),
            Some(approval_logic.clone()),
        );

        Fixture {
            pool,
            team_id,
            prod_name,
            feature_id,
            feature_key,
            stage_id,
            requester_id,
            requester_name,
            approver_id,
            approver_name,
            approval_logic,
            feature_logic,
        }
    }

    /// Requests a deployment of the prod stage; returns the pending request id.
    async fn request_deployment(&self) -> Uuid {
        let feature = self
            .feature_logic
            .request_stage_change(
                ID::from(self.stage_id),
                StageChangeRequestType::DeploymentRequested,
                self.requester_id,
                StageChangeMeta {
                    external_ref: Some("PROJ-9".to_string()),
                    reason: Some("ship it".to_string()),
                    check_reason: false,
                },
            )
            .await
            .expect("request deployment");
        feature
            .pending_approval_request_id
            .and_then(|id| Uuid::try_from(id).ok())
            .expect("pending approval id")
    }

    async fn activity(&self, activity_type: &str) -> Vec<ActivityLogRow> {
        sqlx::query_as::<_, ActivityLogRow>(
            "SELECT * FROM activity_log WHERE entity_id = $1 AND activity_type = $2 \
             ORDER BY created_at",
        )
        .bind(self.stage_id.to_string())
        .bind(activity_type)
        .fetch_all(&self.pool)
        .await
        .expect("read activity")
    }

    async fn stage_status(&self) -> String {
        sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE id = $1")
            .bind(self.stage_id)
            .fetch_one(&self.pool)
            .await
            .expect("stage status")
    }

    fn assert_row(&self, row: &ActivityLogRow, request_id: Uuid, status: &str) {
        assert_eq!(row.entity_type, "stage");
        assert_eq!(row.entity_id, self.stage_id.to_string());
        let metadata = row.metadata.as_ref().expect("metadata");
        assert_eq!(metadata["status"], status);
        assert_eq!(metadata["feature_id"], self.feature_id.to_string());
        assert_eq!(metadata["feature_key"], self.feature_key);
        assert_eq!(metadata["team_id"], self.team_id.to_string());
        assert_eq!(metadata["stage_id"], self.stage_id.to_string());
        assert_eq!(metadata["environment_name"], self.prod_name);
        assert!(metadata["environment_id"].is_string());
        assert_eq!(metadata["approval_request_id"], request_id.to_string());
        assert_eq!(metadata["external_ref"], "PROJ-9");
        assert_eq!(metadata["reason"], "ship it");
    }

    async fn cleanup(self) {
        sqlx::query("DELETE FROM activity_log WHERE entity_id = $1")
            .bind(self.stage_id.to_string())
            .execute(&self.pool)
            .await
            .expect("delete activity");
        sqlx::query("DELETE FROM approval_requests WHERE feature_id = $1")
            .bind(self.feature_id)
            .execute(&self.pool)
            .await
            .expect("delete requests");
        sqlx::query("DELETE FROM features WHERE id = $1")
            .bind(self.feature_id)
            .execute(&self.pool)
            .await
            .expect("delete feature");
        sqlx::query("DELETE FROM approval_policies WHERE team_id = $1")
            .bind(self.team_id)
            .execute(&self.pool)
            .await
            .expect("delete policies");
        sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(self.team_id)
            .execute(&self.pool)
            .await
            .expect("delete team");
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![self.requester_id, self.approver_id])
            .execute(&self.pool)
            .await
            .expect("delete users");
    }
}

#[tokio::test]
async fn final_approval_vote_writes_stage_approved_in_the_same_transaction() {
    let fx = Fixture::new().await;
    let request_id = fx.request_deployment().await;

    fx.approval_logic
        .approve_request(request_id, fx.approver_id, None)
        .await
        .expect("approve");

    assert_eq!(fx.stage_status().await, "DEPLOYMENT_APPROVED");
    let rows = fx.activity("stage_approved").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(fx.approver_id));
    assert_eq!(
        rows[0].actor_name.as_deref(),
        Some(fx.approver_name.as_str())
    );
    fx.assert_row(&rows[0], request_id, "DEPLOYMENT_APPROVED");
    fx.cleanup().await;
}

#[tokio::test]
async fn rejection_vote_writes_stage_rejected() {
    let fx = Fixture::new().await;
    let request_id = fx.request_deployment().await;

    fx.approval_logic
        .reject_request(request_id, fx.approver_id, None)
        .await
        .expect("reject");

    assert_eq!(fx.stage_status().await, "DEPLOYMENT_REJECTED");
    let rows = fx.activity("stage_rejected").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(fx.approver_id));
    assert_eq!(
        rows[0].actor_name.as_deref(),
        Some(fx.approver_name.as_str())
    );
    fx.assert_row(&rows[0], request_id, "DEPLOYMENT_REJECTED");
    assert!(fx.activity("stage_approved").await.is_empty());
    fx.cleanup().await;
}

#[tokio::test]
async fn auto_approval_writes_stage_approved_without_actor_id() {
    let fx = Fixture::new().await;
    let request_id = fx.request_deployment().await;
    let request = approval_repository(fx.pool.clone())
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .unwrap();

    fx.approval_logic
        .auto_approve_request(request)
        .await
        .expect("auto approve");

    let rows = fx.activity("stage_approved").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, None);
    assert_eq!(rows[0].actor_name.as_deref(), Some("Auto-approval"));
    fx.assert_row(&rows[0], request_id, "DEPLOYMENT_APPROVED");
    fx.cleanup().await;
}

#[tokio::test]
async fn cancel_writes_approval_request_cancelled_with_previous_status() {
    let fx = Fixture::new().await;
    let request_id = fx.request_deployment().await;

    fx.approval_logic
        .cancel_request(request_id, fx.requester_id)
        .await
        .expect("cancel");

    assert_eq!(fx.stage_status().await, "NOT_DEPLOYED");
    let rows = fx.activity("approval_request_cancelled").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(fx.requester_id));
    assert_eq!(
        rows[0].actor_name.as_deref(),
        Some(fx.requester_name.as_str())
    );
    fx.assert_row(&rows[0], request_id, "NOT_DEPLOYED");
    fx.cleanup().await;
}

#[tokio::test]
async fn gated_request_writes_stage_change_requested_with_approval_request_id() {
    let fx = Fixture::new().await;
    let request_id = fx.request_deployment().await;

    assert_eq!(fx.stage_status().await, "DEPLOYMENT_REQUESTED");
    let rows = fx.activity("stage_change_requested").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(fx.requester_id));
    assert_eq!(
        rows[0].actor_name.as_deref(),
        Some(fx.requester_name.as_str())
    );
    fx.assert_row(&rows[0], request_id, "DEPLOYMENT_REQUESTED");
    fx.cleanup().await;
}

#[tokio::test]
async fn external_approval_still_writes_only_its_own_row() {
    let fx = Fixture::new().await;
    fx.request_deployment().await;

    let result = fx
        .approval_logic
        .approve_stage_change_externally(
            fx.stage_id,
            ExternalApproval {
                actor_user_id: fx.approver_id,
                actor_name: "Jira (Jane Doe)".to_string(),
                source: "jira".to_string(),
                approver: serde_json::json!({ "system": "jira" }),
                metadata: serde_json::json!({}),
            },
        )
        .await
        .expect("external approval");

    assert!(matches!(result, ExternalApprovalResult::Approved { .. }));
    assert!(fx.activity("stage_approved").await.is_empty());
    let external: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_log WHERE activity_type = \
         'approval_request_approved_externally' AND metadata->>'stage_id' = $1",
    )
    .bind(fx.stage_id.to_string())
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(external, 1);
    sqlx::query("DELETE FROM activity_log WHERE metadata->>'stage_id' = $1")
        .bind(fx.stage_id.to_string())
        .execute(&fx.pool)
        .await
        .unwrap();
    fx.cleanup().await;
}

#[tokio::test]
async fn failed_execution_rolls_back_the_activity_row() {
    let fx = Fixture::new().await;
    let request_id = fx.request_deployment().await;
    // The stage is gone when the vote lands: the change cannot be applied.
    sqlx::query("DELETE FROM features_pipeline_stages WHERE id = $1")
        .bind(fx.stage_id)
        .execute(&fx.pool)
        .await
        .expect("delete stage");

    let result = fx
        .approval_logic
        .approve_request(request_id, fx.approver_id, None)
        .await;

    assert!(result.is_err());
    assert!(fx.activity("stage_approved").await.is_empty());
    let request = approval_repository(fx.pool.clone())
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.status.as_str(), "pending");
    fx.cleanup().await;
}

/// Approval logic with a judgment service whose client must never be called,
/// plus the repository to read the stored judgments.
fn approval_logic_with_judgments(
    pool: &PgPool,
) -> (
    Box<dyn ApprovalLogic>,
    Box<dyn feature_toggle_backend::database::ai::AiJudgmentRepository>,
) {
    use feature_toggle_backend::database::ai::{
        ai_judgment_repository, team_ai_settings_repository,
    };
    use feature_toggle_backend::judgment::client::MockJudgmentClient;
    use feature_toggle_backend::judgment::service::JudgmentService;
    use feature_toggle_backend::logic::approval::approval_logic_with_pool_and_notifications;

    let mut client = MockJudgmentClient::new();
    client.expect_evaluate().times(0);
    client.expect_model().returning(|| "jev-test".to_string());
    let service = std::sync::Arc::new(JudgmentService::new(
        std::sync::Arc::new(client),
        ai_judgment_repository(pool.clone()),
        team_ai_settings_repository(pool.clone()),
    ));
    let activity = activity_log_repository(pool.clone());
    let environment_logic = environment::environment_logic(
        feature_toggle_backend::database::environment::environment_repository(pool.clone()),
        activity,
    );
    let (approval_events_tx, _) = broadcast::channel::<ApprovalRequestEvent>(64);
    let (updates_tx, _) = broadcast::channel::<FeatureUpdate>(64);
    let logic = approval_logic_with_pool_and_notifications(
        pool.clone(),
        approval_repository(pool.clone()),
        feature_repository(pool.clone()),
        environment_logic,
        role::role_repository(pool.clone()),
        approval_events_tx,
        updates_tx,
        None,
        Some(service),
    );
    (logic, ai_judgment_repository(pool.clone()))
}

/// The approval-risk row queued when the request was created.
fn risk_judgment(
    team_id: Uuid,
    request_id: Uuid,
) -> feature_toggle_backend::database::ai::NewJudgment {
    feature_toggle_backend::database::ai::NewJudgment {
        team_id,
        kind: feature_toggle_backend::judgment::JudgmentKind::ApprovalRisk,
        subject_type: feature_toggle_backend::judgment::SubjectType::ApprovalRequest,
        subject_id: request_id,
        input: serde_json::json!({ "change": { "type": "stage_change" } }),
        input_hash: "ji51".to_string(),
    }
}

fn jira_approval(actor_user_id: Uuid) -> ExternalApproval {
    ExternalApproval {
        actor_user_id,
        actor_name: "Jira (Jane Doe)".to_string(),
        source: "jira".to_string(),
        approver: serde_json::json!({ "system": "jira" }),
        metadata: serde_json::json!({}),
    }
}

async fn external_approval_row(fx: &Fixture) -> ActivityLogRow {
    sqlx::query_as::<_, ActivityLogRow>(
        "SELECT * FROM activity_log WHERE activity_type = \
         'approval_request_approved_externally' AND metadata->>'stage_id' = $1",
    )
    .bind(fx.stage_id.to_string())
    .fetch_one(&fx.pool)
    .await
    .expect("external approval row")
}

async fn delete_external_rows(fx: &Fixture) {
    sqlx::query("DELETE FROM activity_log WHERE metadata->>'stage_id' = $1")
        .bind(fx.stage_id.to_string())
        .execute(&fx.pool)
        .await
        .unwrap();
}

/// JI-51: Jira approved the request before its risk assessment ran, so the
/// assessment is skipped: no Jev call and no late assessment row.
#[tokio::test]
async fn external_approval_skips_the_unfinished_risk_assessment() {
    let fx = Fixture::new().await;
    sqlx::query("UPDATE approval_policies SET ai_risk_mode = 'advisory' WHERE team_id = $1")
        .bind(fx.team_id)
        .execute(&fx.pool)
        .await
        .unwrap();
    let request_id = fx.request_deployment().await;
    let (logic, judgments) = approval_logic_with_judgments(&fx.pool);
    judgments
        .upsert_pending(risk_judgment(fx.team_id, request_id))
        .await
        .unwrap();

    let result = logic
        .approve_stage_change_externally(fx.stage_id, jira_approval(fx.approver_id))
        .await
        .expect("external approval");

    assert_eq!(
        result,
        ExternalApprovalResult::Approved {
            approval_request_id: Some(request_id)
        }
    );
    let stored = judgments
        .get_for_subject(
            feature_toggle_backend::judgment::SubjectType::ApprovalRequest,
            request_id,
            feature_toggle_backend::judgment::JudgmentKind::ApprovalRisk,
        )
        .await
        .unwrap()
        .expect("judgment row");
    assert_eq!(stored.status, "skipped");
    assert_eq!(
        stored.error.as_deref(),
        Some("skipped: approval request approved by jira before the assessment ran")
    );
    let metadata = external_approval_row(&fx).await.metadata.expect("metadata");
    assert_eq!(metadata["ai_risk_mode_skipped"], "advisory");
    assert_eq!(metadata["ai_risk_assessment"], "skipped");

    delete_external_rows(&fx).await;
    fx.cleanup().await;
}

/// JI-51: an assessment that finished before Jira approved is history; it
/// stays done.
#[tokio::test]
async fn external_approval_keeps_a_finished_risk_assessment() {
    let fx = Fixture::new().await;
    sqlx::query("UPDATE approval_policies SET ai_risk_mode = 'advisory' WHERE team_id = $1")
        .bind(fx.team_id)
        .execute(&fx.pool)
        .await
        .unwrap();
    let request_id = fx.request_deployment().await;
    let (logic, judgments) = approval_logic_with_judgments(&fx.pool);
    let row = judgments
        .upsert_pending(risk_judgment(fx.team_id, request_id))
        .await
        .unwrap();
    judgments
        .mark_done(
            row.id,
            "ji51".to_string(),
            feature_toggle_backend::database::ai::JudgmentResult {
                model: "jev-test".to_string(),
                raw_answers: serde_json::json!({}),
                derived: serde_json::json!({ "level": "low" }),
                input_tokens: None,
            },
        )
        .await
        .unwrap();

    logic
        .approve_stage_change_externally(fx.stage_id, jira_approval(fx.approver_id))
        .await
        .expect("external approval");

    let stored = judgments
        .get_for_subject(
            feature_toggle_backend::judgment::SubjectType::ApprovalRequest,
            request_id,
            feature_toggle_backend::judgment::JudgmentKind::ApprovalRisk,
        )
        .await
        .unwrap()
        .expect("judgment row");
    assert_eq!(stored.status, "done");

    delete_external_rows(&fx).await;
    fx.cleanup().await;
}
