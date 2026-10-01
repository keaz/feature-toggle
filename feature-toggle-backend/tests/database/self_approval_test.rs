//! The requester of an approval request must never be able to approve it, and
//! must not count towards the approvers a request needs at creation time.

use feature_toggle_backend::Error;
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{CreateFeature, CreateFeatureStage};
use feature_toggle_backend::database::{approval, feature, init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::approval::{ApprovalLogic, ApprovalRequestEvent};
use feature_toggle_backend::logic::feature::StageChangeRequestType;
use feature_toggle_backend::logic::{
    approval as approval_logic, environment, feature as feature_logic,
};
use feature_toggle_backend::model::ID;
use sqlx::PgPool;
use uuid::Uuid;

const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

struct Fixture {
    pool: PgPool,
    team_id: Uuid,
    feature_id: Uuid,
    stage_id: Uuid,
    requester_id: Uuid,
    approval_logic: Box<dyn ApprovalLogic>,
    feature_logic: Box<dyn feature_logic::FeatureLogic>,
}

impl Fixture {
    async fn new(requester_is_admin: bool) -> Self {
        let pool = init_pg_pool().await;
        let team_id = Uuid::new_v4();
        let env_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO teams (id, name, description) VALUES ($1, $2, 'self approval test')",
        )
        .bind(team_id)
        .bind(format!("self-approval-{team_id}"))
        .execute(&pool)
        .await
        .expect("insert team");
        sqlx::query(
            "INSERT INTO environments (id, name, active, team_id, environment_type) VALUES ($1, $2, true, $3, 'Production')",
        )
        .bind(env_id)
        .bind(format!("self-approval-prod-{env_id}"))
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("insert environment");
        sqlx::query(
            r#"INSERT INTO approval_policies
                   (team_id, name, applies_to, required_approvers, approver_role_ids,
                    allow_admin_override, enabled)
               VALUES ($1, $2, 'production_only', 1, $3, true, true)"#,
        )
        .bind(team_id)
        .bind(format!("self-approval-policy-{team_id}"))
        .bind(vec![Uuid::parse_str(APPROVER_ROLE_ID).unwrap()])
        .execute(&pool)
        .await
        .expect("insert policy");

        let requester_id = insert_team_approver(&pool, team_id, requester_is_admin).await;

        let feature_repository = feature::feature_repository(pool.clone());
        let stage_id = Uuid::new_v4();
        let feature_id = feature_repository
            .create_feature(CreateFeature {
                team_id,
                key: format!("self-approval-{}", Uuid::new_v4()),
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
                    environment_id: env_id,
                    order_index: 0,
                    parent_stage: None,
                    position: "{ \"x\": 0, \"y\": 0 }".to_string(),
                    enabled: true,
                }],
                dependencies: vec![],
                variants: None,
            })
            .await
            .expect("create feature");

        let activity_log_repository =
            feature_toggle_backend::database::activity_log::activity_log_repository(pool.clone());
        let environment_logic = environment::environment_logic(
            feature_toggle_backend::database::environment::environment_repository(pool.clone()),
            activity_log_repository.clone_box(),
        );
        let (approval_events_tx, _rx1) =
            tokio::sync::broadcast::channel::<ApprovalRequestEvent>(16);
        let (feature_updates_tx, _rx2) = tokio::sync::broadcast::channel::<FeatureUpdate>(16);
        let approval_logic = approval_logic::approval_logic_with_pool(
            pool.clone(),
            approval::approval_repository(pool.clone()),
            feature_repository.clone_box(),
            environment_logic.clone(),
            role::role_repository(pool.clone()),
            approval_events_tx,
            feature_updates_tx,
        );
        let feature_logic = feature_logic::feature_logic_with_approval(
            feature_repository.clone_box(),
            environment_logic,
            activity_log_repository.clone_box(),
            feature_toggle_backend::database::user::user_repository(pool.clone()),
            Some(approval_logic.clone()),
        );

        Self {
            pool,
            team_id,
            feature_id,
            stage_id,
            requester_id,
            approval_logic,
            feature_logic,
        }
    }

    async fn request_deployment(&self) -> Result<Uuid, Error> {
        sqlx::query("UPDATE features_pipeline_stages SET status = 'NOT_DEPLOYED' WHERE id = $1")
            .bind(self.stage_id)
            .execute(&self.pool)
            .await
            .expect("reset stage");
        let feature = self
            .feature_logic
            .request_stage_change(
                ID::from(self.stage_id),
                StageChangeRequestType::DeploymentRequested,
                self.requester_id,
            )
            .await?;
        Ok(feature
            .pending_approval_request_id
            .and_then(|id| Uuid::try_from(id).ok())
            .expect("pending approval id"))
    }

    async fn cleanup(self, users: &[Uuid]) {
        let _ = sqlx::query("DELETE FROM approval_requests WHERE feature_id = $1")
            .bind(self.feature_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM features WHERE id = $1")
            .bind(self.feature_id)
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(self.team_id)
            .execute(&self.pool)
            .await;
        let mut all = users.to_vec();
        all.push(self.requester_id);
        let _ = sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(all)
            .execute(&self.pool)
            .await;
    }
}

async fn insert_team_approver(pool: &PgPool, team_id: Uuid, is_admin: bool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin)
         VALUES ($1, $2, 'x', 'Self', 'Approval', $3, $4)",
    )
    .bind(id)
    .bind(format!("self_approval_{id}"))
    .bind(format!("self_approval_{id}@example.com"))
    .bind(is_admin)
    .execute(pool)
    .await
    .expect("insert user");
    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(id)
        .bind(team_id)
        .execute(pool)
        .await
        .expect("insert membership");
    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
        .bind(id)
        .bind(Uuid::parse_str(APPROVER_ROLE_ID).unwrap())
        .execute(pool)
        .await
        .expect("assign approver role");
    id
}

/// A team whose only approver is the requester cannot create an approval request:
/// nobody else could ever approve it.
#[tokio::test]
async fn test_request_creation_fails_when_requester_is_only_approver() {
    let fixture = Fixture::new(false).await;

    let result = fixture.request_deployment().await;

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM approval_requests WHERE feature_id = $1")
            .bind(fixture.feature_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    fixture.cleanup(&[]).await;

    match result {
        Err(Error::InvalidInput(message)) => {
            assert!(message.contains("eligible approver"), "got: {message}")
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
    assert_eq!(pending, 0, "no un-approvable request may be left behind");
}

/// With a second approver the request is created, the requester is not in the frozen
/// eligible set, and neither a plain nor an admin-override approval by the requester works.
/// Rejecting their own request stays allowed.
async fn requester_cannot_approve_own_request(requester_is_admin: bool) {
    let fixture = Fixture::new(requester_is_admin).await;
    let other_approver = insert_team_approver(&fixture.pool, fixture.team_id, false).await;

    let request_id = fixture
        .request_deployment()
        .await
        .expect("request creation with a second approver");
    let stored = fixture
        .approval_logic
        .get_request(request_id)
        .await
        .expect("stored request");

    let approve = fixture
        .approval_logic
        .approve_request(request_id, fixture.requester_id, None)
        .await;
    let still_pending = fixture
        .approval_logic
        .get_request(request_id)
        .await
        .expect("request after approve attempt");
    let reject = fixture
        .approval_logic
        .reject_request(request_id, fixture.requester_id, Some("withdrawn".into()))
        .await;
    fixture.cleanup(&[other_approver]).await;

    assert_eq!(stored.eligible_approver_ids, vec![other_approver]);
    assert!(
        matches!(approve, Err(Error::SelfApprovalNotAllowed)),
        "got {approve:?}"
    );
    assert_eq!(still_pending.approved_count, 0);
    assert_eq!(still_pending.status.as_str(), "pending");
    assert!(reject.is_ok(), "own rejection stays allowed: {reject:?}");
}

#[tokio::test]
async fn test_requester_cannot_approve_own_request() {
    requester_cannot_approve_own_request(false).await;
}

#[tokio::test]
async fn test_admin_requester_cannot_self_approve_via_override() {
    requester_cannot_approve_own_request(true).await;
}

/// Someone other than the requester can still approve it.
#[tokio::test]
async fn test_other_approver_can_still_approve() {
    let fixture = Fixture::new(false).await;
    let other_approver = insert_team_approver(&fixture.pool, fixture.team_id, false).await;
    let request_id = fixture.request_deployment().await.expect("create request");

    let approve = fixture
        .approval_logic
        .approve_request(request_id, other_approver, None)
        .await;
    fixture.cleanup(&[other_approver]).await;

    let approved = approve.expect("approval by a different approver");
    assert_eq!(approved.status.as_str(), "approved");
}
