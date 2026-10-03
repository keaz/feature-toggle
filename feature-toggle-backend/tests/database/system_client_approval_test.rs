use chrono::{Duration, Utc};
use feature_toggle_backend::database::system_client::{
    CreateSystemClient, system_client_repository,
};
use feature_toggle_backend::database::{approval, feature, init_pg_pool, role};
use feature_toggle_backend::grpc::pb::FeatureUpdate;
use feature_toggle_backend::logic::approval::{ApprovalRequestEvent, approval_logic_with_pool};
use feature_toggle_backend::logic::environment;
use uuid::Uuid;

const APPROVER_ROLE_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Approvals need a human (Jira decision J2). A system client's shadow user holds
/// the Approver role, but it never counts as an eligible approver, in its own
/// team or any other. A human approver of the team still counts.
#[tokio::test]
async fn test_system_client_is_never_an_eligible_approver() {
    let pool = init_pg_pool().await;
    let team_id = Uuid::new_v4();
    let other_team_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let other_env_id = Uuid::new_v4();
    let approver_role = Uuid::parse_str(APPROVER_ROLE_ID).unwrap();

    for (team, env) in [(team_id, env_id), (other_team_id, other_env_id)] {
        sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'system client approval test')")
            .bind(team)
            .bind(format!("sc-approval-{team}"))
            .execute(&pool)
            .await
            .expect("insert team");
        sqlx::query(
            "INSERT INTO environments (id, name, active, team_id, environment_type) VALUES ($1, $2, true, $3, 'Production')",
        )
        .bind(env)
        .bind(format!("sc-approval-prod-{env}"))
        .bind(team)
        .execute(&pool)
        .await
        .expect("insert environment");
        sqlx::query(
            r#"INSERT INTO approval_policies
                   (team_id, name, applies_to, required_approvers, approver_role_ids, enabled)
               VALUES ($1, $2, 'production_only', 1, $3, true)"#,
        )
        .bind(team)
        .bind(format!("sc-approval-policy-{team}"))
        .bind(vec![approver_role])
        .execute(&pool)
        .await
        .expect("insert policy");
    }

    let system_client = system_client_repository(pool.clone())
        .create_system_client(
            team_id,
            CreateSystemClient {
                name: format!("sc-approval-{}", Uuid::new_v4().simple()),
                description: None,
                enabled: true,
                expires_at: Utc::now() + Duration::days(1),
            },
        )
        .await
        .expect("create system client");

    // A human approver in the own team only.
    let human_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO users (id, username, password_hash, first_name, last_name, email, is_admin, enabled)
           VALUES ($1, $2, 'x', 'Human', 'Approver', $3, false, true)"#,
    )
    .bind(human_id)
    .bind(format!("sc-approval-human-{human_id}"))
    .bind(format!("sc-approval-human-{human_id}@example.com"))
    .execute(&pool)
    .await
    .expect("insert human approver");
    sqlx::query("INSERT INTO user_teams (user_id, team_id) VALUES ($1, $2)")
        .bind(human_id)
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("add human approver to team");
    sqlx::query("INSERT INTO user_roles (user_id, role_id) VALUES ($1, $2)")
        .bind(human_id)
        .bind(approver_role)
        .execute(&pool)
        .await
        .expect("give human the Approver role");

    let activity_log_repository =
        feature_toggle_backend::database::activity_log::activity_log_repository(pool.clone());
    let environment_logic = environment::environment_logic(
        feature_toggle_backend::database::environment::environment_repository(pool.clone()),
        activity_log_repository.clone_box(),
    );
    let (approval_events_tx, _approval_events_rx) =
        tokio::sync::broadcast::channel::<ApprovalRequestEvent>(16);
    let (feature_updates_tx, _feature_updates_rx) =
        tokio::sync::broadcast::channel::<FeatureUpdate>(16);
    let logic = approval_logic_with_pool(
        pool.clone(),
        approval::approval_repository(pool.clone()),
        feature::feature_repository(pool.clone()),
        environment_logic,
        role::role_repository(pool.clone()),
        approval_events_tx,
        feature_updates_tx,
    );

    let own = logic
        .preview_stage_change_policy(team_id, env_id, "DEPLOYMENT_REQUESTED")
        .await;
    let other = logic
        .preview_stage_change_policy(other_team_id, other_env_id, "DEPLOYMENT_REQUESTED")
        .await;

    // Clean up before asserting. The shadow user is not cascaded by the team.
    sqlx::query("DELETE FROM teams WHERE id = ANY($1)")
        .bind(vec![team_id, other_team_id])
        .execute(&pool)
        .await
        .expect("delete teams");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![system_client.id, human_id])
        .execute(&pool)
        .await
        .expect("delete shadow user and human approver");

    // Only the human: the system client of this team is not counted.
    assert_eq!(
        own.expect("preview own team").eligible_approvers_count,
        Some(1)
    );
    assert_eq!(
        other.expect("preview other team").eligible_approvers_count,
        Some(0)
    );
}
