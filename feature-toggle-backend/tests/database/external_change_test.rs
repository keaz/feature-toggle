//! JI-14: stage changes on behalf of Jira (`logic::external_change`), against
//! the seeded test DB. Each test builds its own team, environments and
//! features, and removes them at the end.

use std::collections::BTreeMap;

use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::approval::approval_repository;
use feature_toggle_backend::database::entity::{ApprovalStatus, FeatureType};
use feature_toggle_backend::database::feature::{
    CreateFeature, CreateFeatureStage, feature_repository,
};
use feature_toggle_backend::database::jira_integration::jira_integration_repository_tx;
use feature_toggle_backend::database::{init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::ActorContext;
use feature_toggle_backend::logic::approval::{ApprovalLogic, ApprovalRequestEvent};
use feature_toggle_backend::logic::external_change::{
    ExternalAction, ExternalActor, ExternalChangeContext, ExternalChangeLogic, ExternalOutcome,
    REFUSED_NOT_APPROVED, REFUSED_NOT_TRUSTED, external_change_logic,
};
use feature_toggle_backend::logic::jira_integration_tx::{
    JiraIntegrationInput, create_jira_integration_in_tx,
};
use feature_toggle_backend::logic::{approval, environment, feature};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Seeded users with the `Approver` role (`init.sql`).
const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SEED_APPROVER_ID: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

/// A team with a policy-gated environment (QA, one approver) and an
/// environment without a policy (Dev), a Jira integration whose shadow user
/// acts, and the logic stack wired as in `lib.rs::run`.
struct Fixture {
    pool: PgPool,
    team_id: Uuid,
    qa: Uuid,
    dev: Uuid,
    actor_user_id: Uuid,
    approval_logic: Box<dyn ApprovalLogic>,
    logic: Box<dyn ExternalChangeLogic>,
    updates_tx: broadcast::Sender<FeatureUpdate>,
}

impl Fixture {
    async fn new() -> Self {
        let pool = init_pg_pool().await;
        let team_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO teams (id, name, description) VALUES ($1, $2, 'external change test')",
        )
        .bind(team_id)
        .bind(format!("external-change-test-{team_id}"))
        .execute(&pool)
        .await
        .expect("insert team");
        let mut environments = Vec::new();
        for name in ["QA", "Dev"] {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO environments (id, name, active, team_id, environment_type) \
                 VALUES ($1, $2, TRUE, $3, 'Development')",
            )
            .bind(id)
            .bind(name)
            .bind(team_id)
            .execute(&pool)
            .await
            .expect("insert environment");
            environments.push(id);
        }
        let (qa, dev) = (environments[0], environments[1]);

        // Approvers must be team members to be eligible.
        for user in [SEED_ADMIN_ID, SEED_APPROVER_ID] {
            sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
                .bind(uuid(user))
                .bind(team_id)
                .execute(&pool)
                .await
                .expect("insert team member");
        }
        sqlx::query(
            "INSERT INTO approval_policies (team_id, name, applies_to, environment_ids, \
             required_approvers, approver_role_ids, enabled) \
             VALUES ($1, 'QA approvals', 'specific_environments', $2, 1, $3, TRUE)",
        )
        .bind(team_id)
        .bind(vec![qa])
        .bind(vec![uuid(APPROVER_ROLE_ID)])
        .execute(&pool)
        .await
        .expect("insert policy");

        let actor_user_id = {
            let repo = jira_integration_repository_tx(pool.clone());
            let activity = activity_log_repository(pool.clone());
            let mut tx = pool.begin().await.expect("begin");
            let created = create_jira_integration_in_tx(
                &mut tx,
                &repo,
                activity.as_ref(),
                team_id,
                JiraIntegrationInput {
                    name: "Jira".to_string(),
                    jira_base_url: None,
                    environment_field: "labels".to_string(),
                    environment_aliases: BTreeMap::new(),
                    jira_approved_environment_ids: vec![qa.to_string()],
                    feature_key_field: None,
                    enabled: true,
                },
                ActorContext::new(uuid(SEED_ADMIN_ID), "admin".to_string()),
            )
            .await
            .expect("create integration");
            tx.commit().await.expect("commit");
            created.integration.actor_user_id
        };

        let activity = activity_log_repository(pool.clone());
        let environment_logic = environment::environment_logic(
            feature_toggle_backend::database::environment::environment_repository(pool.clone()),
            activity.clone_box(),
        );
        let (approval_events_tx, _) = broadcast::channel::<ApprovalRequestEvent>(64);
        let (updates_tx, _) = broadcast::channel::<FeatureUpdate>(64);
        let approval_logic = approval::approval_logic_with_pool(
            pool.clone(),
            approval_repository(pool.clone()),
            feature_repository(pool.clone()),
            environment_logic.clone(),
            role::role_repository(pool.clone()),
            approval_events_tx,
            updates_tx.clone(),
        );
        let feature_logic = feature::feature_logic_with_approval(
            feature_repository(pool.clone()),
            environment_logic,
            activity.clone_box(),
            feature_toggle_backend::database::user::user_repository(pool.clone()),
            Some(approval_logic.clone()),
        );
        let logic = external_change_logic(
            pool.clone(),
            feature_logic,
            approval_logic.clone(),
            feature_repository(pool.clone()),
            activity,
            updates_tx.clone(),
        );

        Fixture {
            pool,
            team_id,
            qa,
            dev,
            actor_user_id,
            approval_logic,
            logic,
            updates_tx,
        }
    }

    /// A feature with one stage in `environment_id`, in `status`.
    async fn feature(&self, environment_id: Uuid, status: &str) -> Uuid {
        let repository = feature_repository(self.pool.clone());
        let stage_id = Uuid::new_v4();
        let feature_id = repository
            .create_feature(CreateFeature {
                team_id: self.team_id,
                key: format!("external-change-{}", Uuid::new_v4()),
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
                    environment_id,
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
        self.set_status(feature_id, status).await;
        feature_id
    }

    async fn set_status(&self, feature_id: Uuid, status: &str) {
        sqlx::query("UPDATE features_pipeline_stages SET status = $2 WHERE feature_id = $1")
            .bind(feature_id)
            .bind(status)
            .execute(&self.pool)
            .await
            .expect("set stage status");
    }

    async fn status(&self, feature_id: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM features_pipeline_stages WHERE feature_id = $1")
            .bind(feature_id)
            .fetch_one(&self.pool)
            .await
            .expect("stage status")
    }

    async fn request_count(&self, feature_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM approval_requests WHERE feature_id = $1")
            .bind(feature_id)
            .fetch_one(&self.pool)
            .await
            .expect("count requests")
    }

    fn context(&self, trusted: bool) -> ExternalChangeContext {
        ExternalChangeContext {
            actor_user_id: self.actor_user_id,
            external_ref: "PROJ-123".to_string(),
            external_status: "Ready for Release".to_string(),
            reason: "Jira status 'Ready for Release'".to_string(),
            external_actor: ExternalActor {
                system: "jira".to_string(),
                account_id: Some("5b10ac8d82e05b22cc7d4ef5".to_string()),
                display_name: Some("Jane Doe".to_string()),
            },
            trusted_approval: trusted,
        }
    }

    async fn apply(
        &self,
        feature_id: Uuid,
        environment_id: Uuid,
        action: ExternalAction,
        trusted: bool,
    ) -> ExternalOutcome {
        self.logic
            .apply_external_action(feature_id, environment_id, action, &self.context(trusted))
            .await
            .expect("apply external action")
    }

    async fn cleanup(self) {
        sqlx::query(
            "DELETE FROM approval_requests WHERE feature_id IN \
             (SELECT id FROM features WHERE team_id = $1)",
        )
        .bind(self.team_id)
        .execute(&self.pool)
        .await
        .expect("delete requests");
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
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(self.actor_user_id)
            .execute(&self.pool)
            .await
            .expect("delete shadow user");
    }
}

fn applied_request_id(outcome: &ExternalOutcome) -> Option<Uuid> {
    match outcome {
        ExternalOutcome::Applied {
            approval_request_id,
            ..
        } => *approval_request_id,
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test]
async fn request_creates_a_pending_request_with_the_issue_key_and_reason() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Request, false)
        .await;

    let request_id = applied_request_id(&outcome).expect("a pending request");
    assert!(matches!(
        &outcome,
        ExternalOutcome::Applied { from, to, .. }
            if from == "NOT_DEPLOYED" && to == "DEPLOYMENT_REQUESTED"
    ));
    let request = approval_repository(fx.pool.clone())
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(request.status, ApprovalStatus::Pending));
    assert_eq!(request.requested_by, fx.actor_user_id);
    assert_eq!(request.external_ref.as_deref(), Some("PROJ-123"));
    assert_eq!(
        request.request_reason.as_deref(),
        Some("Jira status 'Ready for Release'")
    );
    assert_eq!(request.approval_source, "fluxgate");

    // A second delivery changes nothing.
    let again = fx
        .apply(feature_id, fx.qa, ExternalAction::Request, false)
        .await;
    assert_eq!(
        again,
        ExternalOutcome::NoOp {
            status: "DEPLOYMENT_REQUESTED".to_string()
        }
    );
    assert_eq!(fx.request_count(feature_id).await, 1);
    fx.cleanup().await;
}

#[tokio::test]
async fn trusted_approve_under_a_policy_closes_the_request_as_jira() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Approve, true)
        .await;

    let request_id = applied_request_id(&outcome).expect("the approved request");
    assert!(matches!(
        &outcome,
        ExternalOutcome::Applied { from, to, .. }
            if from == "NOT_DEPLOYED" && to == "DEPLOYMENT_APPROVED"
    ));
    assert_eq!(fx.status(feature_id).await, "DEPLOYMENT_APPROVED");

    let request = approval_repository(fx.pool.clone())
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(request.status, ApprovalStatus::Approved));
    assert_eq!(request.approval_source, "jira");
    assert!(request.executed_at.is_some());
    assert_eq!(request.approved_count, 0, "no vote is recorded");
    let approver = request.external_approver.expect("external approver");
    assert_eq!(approver["system"], "jira");
    assert_eq!(approver["account_id"], "5b10ac8d82e05b22cc7d4ef5");
    assert_eq!(approver["display_name"], "Jane Doe");
    assert_eq!(approver["issue_key"], "PROJ-123");
    assert_eq!(approver["status"], "Ready for Release");

    let votes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM approval_votes WHERE request_id = $1")
            .bind(request_id)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(votes, 0);

    let metadata: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM activity_log WHERE activity_type = 'approval_request_approved_externally' \
         AND entity_id = $1",
    )
    .bind(feature_id.to_string())
    .fetch_one(&fx.pool)
    .await
    .expect("activity row");
    assert_eq!(metadata["approval_request_id"], request_id.to_string());
    assert_eq!(metadata["issue_key"], "PROJ-123");
    assert_eq!(metadata["external_status"], "Ready for Release");
    assert_eq!(metadata["external_actor"]["display_name"], "Jane Doe");
    // The policy uses the default AI risk mode (advisory), which never ran.
    assert_eq!(metadata["ai_risk_mode_skipped"], "advisory");

    fx.cleanup().await;
}

#[tokio::test]
async fn trusted_approve_of_a_pending_request_closes_that_request() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;
    let requested = fx
        .apply(feature_id, fx.qa, ExternalAction::Request, false)
        .await;
    let request_id = applied_request_id(&requested);

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Approve, true)
        .await;

    assert_eq!(applied_request_id(&outcome), request_id);
    assert_eq!(fx.status(feature_id).await, "DEPLOYMENT_APPROVED");
    assert_eq!(fx.request_count(feature_id).await, 1);
    fx.cleanup().await;
}

#[tokio::test]
async fn untrusted_approve_refuses_and_changes_nothing() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Approve, false)
        .await;

    assert_eq!(
        outcome,
        ExternalOutcome::Refused {
            reason: REFUSED_NOT_TRUSTED.to_string()
        }
    );
    assert_eq!(fx.status(feature_id).await, "NOT_DEPLOYED");
    assert_eq!(fx.request_count(feature_id).await, 0);
    fx.cleanup().await;
}

#[tokio::test]
async fn trusted_approve_without_a_policy_moves_the_stage_to_approved() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.dev, "NOT_DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.dev, ExternalAction::Approve, true)
        .await;

    assert_eq!(
        outcome,
        ExternalOutcome::Applied {
            from: "NOT_DEPLOYED".to_string(),
            to: "DEPLOYMENT_APPROVED".to_string(),
            approval_request_id: None,
        }
    );
    assert_eq!(fx.status(feature_id).await, "DEPLOYMENT_APPROVED");
    assert_eq!(fx.request_count(feature_id).await, 0);
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_log WHERE activity_type = 'approval_request_approved_externally' \
         AND entity_id = $1",
    )
    .bind(feature_id.to_string())
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(rows, 1);
    fx.cleanup().await;
}

#[tokio::test]
async fn deploy_before_approval_refuses() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "DEPLOYMENT_REQUESTED").await;

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Deploy, true)
        .await;

    assert_eq!(
        outcome,
        ExternalOutcome::Refused {
            reason: REFUSED_NOT_APPROVED.to_string()
        }
    );
    assert_eq!(fx.status(feature_id).await, "DEPLOYMENT_REQUESTED");
    fx.cleanup().await;
}

#[tokio::test]
async fn deploy_after_approval_deploys_and_broadcasts() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;
    fx.apply(feature_id, fx.qa, ExternalAction::Approve, true)
        .await;
    let mut updates = fx.updates_tx.subscribe();

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Deploy, false)
        .await;

    assert!(matches!(
        &outcome,
        ExternalOutcome::Applied { from, to, .. }
            if from == "DEPLOYMENT_APPROVED" && to == "DEPLOYED"
    ));
    assert_eq!(fx.status(feature_id).await, "DEPLOYED");
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM features_pipeline_stages WHERE feature_id = $1")
            .bind(feature_id)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert!(enabled);

    let mut broadcast = false;
    while let Ok(update) = updates.try_recv() {
        if update.feature.as_ref().map(|f| f.id.clone()) == Some(feature_id.to_string()) {
            broadcast = true;
        }
    }
    assert!(broadcast, "a FeatureUpdate is broadcast for the feature");

    let again = fx
        .apply(feature_id, fx.qa, ExternalAction::Deploy, false)
        .await;
    assert_eq!(
        again,
        ExternalOutcome::NoOp {
            status: "DEPLOYED".to_string()
        }
    );
    fx.cleanup().await;
}

#[tokio::test]
async fn trusted_rollback_from_deployed_ends_rollbacked() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Rollback, true)
        .await;

    let request_id = applied_request_id(&outcome).expect("the rollback request");
    assert!(matches!(
        &outcome,
        ExternalOutcome::Applied { from, to, .. } if from == "DEPLOYED" && to == "ROLLBACKED"
    ));
    assert_eq!(fx.status(feature_id).await, "ROLLBACKED");
    let request = approval_repository(fx.pool.clone())
        .get_request_by_id(request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.approval_source, "jira");
    fx.cleanup().await;
}

#[tokio::test]
async fn untrusted_rollback_leaves_a_pending_request() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Rollback, false)
        .await;

    assert!(applied_request_id(&outcome).is_some());
    assert_eq!(fx.status(feature_id).await, "ROLLBACK_REQUESTED");
    fx.cleanup().await;
}

#[tokio::test]
async fn a_freeze_window_refuses_without_override() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;
    sqlx::query(
        "INSERT INTO change_freeze_windows (team_id, name, environment_id, starts_at, ends_at) \
         VALUES ($1, 'Release freeze', $2, NOW() - INTERVAL '1 hour', NOW() + INTERVAL '1 hour')",
    )
    .bind(fx.team_id)
    .bind(fx.qa)
    .execute(&fx.pool)
    .await
    .expect("insert freeze window");

    let outcome = fx
        .apply(feature_id, fx.qa, ExternalAction::Approve, true)
        .await;

    assert_eq!(
        outcome,
        ExternalOutcome::Refused {
            reason: "freeze window Release freeze".to_string()
        }
    );
    assert_eq!(fx.status(feature_id).await, "NOT_DEPLOYED");
    assert_eq!(fx.request_count(feature_id).await, 0);
    let blocked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_log WHERE activity_type = 'freeze_blocked' AND entity_id = $1",
    )
    .bind(feature_id.to_string())
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(blocked, 1);
    fx.cleanup().await;
}

#[tokio::test]
async fn a_stage_missing_in_the_environment_refuses() {
    let fx = Fixture::new().await;
    let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;

    let outcome = fx
        .apply(feature_id, fx.dev, ExternalAction::Request, false)
        .await;

    assert!(matches!(outcome, ExternalOutcome::Refused { .. }));
    fx.cleanup().await;
}

#[tokio::test]
async fn a_human_vote_and_a_jira_approval_close_the_request_once() {
    for _ in 0..5 {
        let fx = Fixture::new().await;
        let feature_id = fx.feature(fx.qa, "NOT_DEPLOYED").await;
        let requested = fx
            .apply(feature_id, fx.qa, ExternalAction::Request, false)
            .await;
        let request_id = applied_request_id(&requested).unwrap();

        let context = fx.context(true);
        let (vote, jira) = tokio::join!(
            fx.approval_logic
                .approve_request(request_id, uuid(SEED_ADMIN_ID), None),
            fx.logic
                .apply_external_action(feature_id, fx.qa, ExternalAction::Approve, &context),
        );
        let jira = jira.expect("no infrastructure error");
        let jira_closed = matches!(jira, ExternalOutcome::Applied { .. });
        assert_ne!(
            vote.is_ok(),
            jira_closed,
            "exactly one closes it: vote {vote:?}, jira {jira:?}"
        );

        let request = approval_repository(fx.pool.clone())
            .get_request_by_id(request_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(request.status, ApprovalStatus::Approved));
        let expected_source = if jira_closed { "jira" } else { "fluxgate" };
        assert_eq!(request.approval_source, expected_source);
        assert_eq!(fx.status(feature_id).await, "DEPLOYMENT_APPROVED");
        fx.cleanup().await;
    }
}
