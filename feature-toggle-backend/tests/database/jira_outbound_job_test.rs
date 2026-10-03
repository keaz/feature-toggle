//! JI-42: the Jira write-back outbox (`jira_outbound_jobs`).

use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jira_integration::jira_integration_repository_tx;
use feature_toggle_backend::database::jira_outbound_job::{
    JiraOutboundJobRepositoryTx, NewOutboundJob, OutboundKind, jira_outbound_job_repository,
    jira_outbound_job_repository_tx,
};
use feature_toggle_backend::logic::ActorContext;
use feature_toggle_backend::logic::jira_integration_tx::{
    JiraIntegrationInput, create_jira_integration_in_tx,
};
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Fixture {
    team_id: Uuid,
    integration_id: Uuid,
    shadow_user: Uuid,
}

/// A team with one enabled integration whose write-back is on and not paused.
async fn fixture(pool: &PgPool) -> Fixture {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira jobs')")
        .bind(team_id)
        .bind(format!("jira-job-test-{team_id}"))
        .execute(pool)
        .await
        .expect("insert team");
    let repo = jira_integration_repository_tx(pool.clone());
    let activity = activity_log_repository(pool.clone());
    let mut tx = pool.begin().await.expect("begin");
    let created = create_jira_integration_in_tx(
        &mut tx,
        &repo,
        activity.as_ref(),
        team_id,
        JiraIntegrationInput {
            name: "jobs".to_string(),
            jira_base_url: None,
            environment_field: "labels".to_string(),
            environment_aliases: BTreeMap::new(),
            jira_approved_environment_ids: vec![],
            feature_key_field: None,
            enabled: true,
        },
        ActorContext::new(Uuid::parse_str(SEED_ADMIN_ID).unwrap(), "admin".to_string()),
    )
    .await
    .expect("create integration");
    tx.commit().await.expect("commit");
    sqlx::query("UPDATE jira_integrations SET writeback_enabled = TRUE WHERE id = $1")
        .bind(created.integration.id)
        .execute(pool)
        .await
        .expect("enable write-back");
    Fixture {
        team_id,
        integration_id: created.integration.id,
        shadow_user: created.integration.actor_user_id,
    }
}

impl Fixture {
    async fn cleanup(self, pool: &PgPool) {
        sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(self.team_id)
            .execute(pool)
            .await
            .expect("delete team");
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(self.shadow_user)
            .execute(pool)
            .await
            .expect("delete shadow user");
    }

    async fn feature(&self, pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, $2, 'Simple', $3)",
        )
        .bind(id)
        .bind(format!("job-flag-{id}"))
        .bind(self.team_id)
        .execute(pool)
        .await
        .expect("insert feature");
        id
    }

    fn job(&self, kind: OutboundKind, dedupe_key: &str) -> NewOutboundJob {
        NewOutboundJob {
            integration_id: self.integration_id,
            issue_key: "PROJ-1".to_string(),
            feature_id: None,
            kind,
            payload: serde_json::json!({"lines": ["hello"]}),
            dedupe_key: dedupe_key.to_string(),
        }
    }
}

#[tokio::test]
async fn enqueue_is_idempotent_on_dedupe_key() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());

    assert!(
        repo.enqueue(fx.job(OutboundKind::Comment, "k1"))
            .await
            .unwrap()
    );
    assert!(
        !repo
            .enqueue(fx.job(OutboundKind::Comment, "k1"))
            .await
            .unwrap()
    );
    assert!(
        repo.enqueue(fx.job(OutboundKind::Comment, "k2"))
            .await
            .unwrap()
    );
    let (rows, total) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    assert_eq!(total, 2);
    assert!(
        rows.iter()
            .all(|row| row.status == "pending" && row.attempts == 0)
    );
    assert_eq!(rows[0].kind, "comment");

    fx.cleanup(&pool).await;
}

#[tokio::test]
async fn only_one_pending_remote_link_per_issue_and_feature() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    let feature = fx.feature(&pool).await;
    let link = |dedupe: &str| NewOutboundJob {
        feature_id: Some(feature),
        ..fx.job(OutboundKind::RemoteLink, dedupe)
    };

    assert!(repo.enqueue(link("l1")).await.unwrap());
    // Different dedupe key, same pending remote link: a no-op.
    assert!(!repo.enqueue(link("l2")).await.unwrap());
    // Another issue is a different link.
    let other_issue = NewOutboundJob {
        issue_key: "PROJ-2".to_string(),
        ..link("l3")
    };
    assert!(repo.enqueue(other_issue).await.unwrap());
    // Once the first one is sent, a new refresh can be queued.
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    let first = rows.iter().find(|row| row.dedupe_key == "l1").unwrap();
    repo.mark_sent(first.id, None).await.unwrap();
    assert!(repo.enqueue(link("l4")).await.unwrap());

    fx.cleanup(&pool).await;
}

#[tokio::test]
#[serial(jira_jobs)]
async fn claim_due_skips_paused_and_disabled_integrations() {
    let pool = init_pg_pool().await;
    let active = fixture(&pool).await;
    let paused = fixture(&pool).await;
    let disabled = fixture(&pool).await;
    let writeback_off = fixture(&pool).await;
    sqlx::query(
        "UPDATE jira_integrations SET writeback_paused_reason = 'Jira returned 401' WHERE id = $1",
    )
    .bind(paused.integration_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE jira_integrations SET enabled = FALSE WHERE id = $1")
        .bind(disabled.integration_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jira_integrations SET writeback_enabled = FALSE WHERE id = $1")
        .bind(writeback_off.integration_id)
        .execute(&pool)
        .await
        .unwrap();
    let repo = jira_outbound_job_repository(pool.clone());
    let owned = |fx: &Fixture| fx.job(OutboundKind::Comment, &format!("c-{}", fx.integration_id));
    for fx in [&active, &paused, &disabled, &writeback_off] {
        assert!(repo.enqueue(owned(fx)).await.unwrap());
    }

    let claimed = repo.claim_due(500, Duration::minutes(5)).await.unwrap();
    let ours: Vec<_> = claimed
        .iter()
        .filter(|row| {
            [&active, &paused, &disabled, &writeback_off]
                .iter()
                .any(|fx| fx.integration_id == row.integration_id)
        })
        .collect();
    assert!(!ours.is_empty());
    assert!(
        ours.iter()
            .all(|row| row.integration_id == active.integration_id)
    );

    for fx in [active, paused, disabled, writeback_off] {
        fx.cleanup(&pool).await;
    }
}

#[tokio::test]
#[serial(jira_jobs)]
async fn claim_due_leases_jobs_so_a_second_claim_gets_none() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    repo.enqueue(fx.job(
        OutboundKind::Comment,
        &format!("lease-{}", fx.integration_id),
    ))
    .await
    .unwrap();

    let mine = |rows: Vec<feature_toggle_backend::database::entity::OutboundJobRow>| {
        rows.into_iter()
            .filter(|row| row.integration_id == fx.integration_id)
            .collect::<Vec<_>>()
    };
    let first = mine(repo.claim_due(500, Duration::minutes(5)).await.unwrap());
    assert_eq!(first.len(), 1);
    assert!(first[0].next_attempt_at > Utc::now() + Duration::minutes(4));
    assert_eq!(first[0].status, "pending");
    let second = mine(repo.claim_due(500, Duration::minutes(5)).await.unwrap());
    assert!(second.is_empty());

    fx.cleanup(&pool).await;
}

#[tokio::test]
#[serial(jira_jobs)]
async fn a_waiting_earlier_job_holds_back_later_jobs_of_the_same_issue() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    repo.enqueue(fx.job(OutboundKind::Comment, &format!("o1-{}", fx.integration_id)))
        .await
        .unwrap();
    repo.enqueue(fx.job(OutboundKind::Comment, &format!("o2-{}", fx.integration_id)))
        .await
        .unwrap();
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    let first = rows
        .iter()
        .find(|row| row.dedupe_key.starts_with("o1-"))
        .unwrap();
    // The first job waits for a retry; the second one is due but must not overtake it.
    repo.mark_retry(
        first.id,
        1,
        Utc::now() + Duration::minutes(10),
        "500: x".to_string(),
    )
    .await
    .unwrap();
    let claimed = repo.claim_due(500, Duration::minutes(5)).await.unwrap();
    assert!(
        claimed
            .iter()
            .all(|row| row.integration_id != fx.integration_id),
        "a later job overtook the waiting one"
    );

    fx.cleanup(&pool).await;
}

#[tokio::test]
async fn retry_dead_resets_attempts_and_rejects_non_dead() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let other = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    repo.enqueue(fx.job(OutboundKind::Comment, &format!("rd-{}", fx.integration_id)))
        .await
        .unwrap();
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    let id = rows[0].id;

    // Pending: not dead.
    assert!(
        repo.retry_dead(fx.integration_id, id)
            .await
            .unwrap()
            .is_none()
    );
    repo.mark_dead(id, 6, "500: boom".to_string())
        .await
        .unwrap();
    // Another integration's id does not match.
    assert!(
        repo.retry_dead(other.integration_id, id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.retry_dead(fx.integration_id, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );

    let retried = repo
        .retry_dead(fx.integration_id, id)
        .await
        .unwrap()
        .expect("dead job is retried");
    assert_eq!(retried.status, "pending");
    assert_eq!(retried.attempts, 0);
    assert!(retried.next_attempt_at <= Utc::now() + Duration::seconds(1));
    // Second call: no longer dead.
    assert!(
        repo.retry_dead(fx.integration_id, id)
            .await
            .unwrap()
            .is_none()
    );

    fx.cleanup(&pool).await;
    other.cleanup(&pool).await;
}

#[tokio::test]
async fn cancel_pending_marks_jobs_dead() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let other = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    let tx_repo = jira_outbound_job_repository_tx(pool.clone());
    for (name, f) in [("a", &fx), ("b", &fx), ("c", &other)] {
        repo.enqueue(f.job(
            OutboundKind::Comment,
            &format!("{name}-{}", f.integration_id),
        ))
        .await
        .unwrap();
    }
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    repo.mark_sent(rows[0].id, None).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let cancelled = tx_repo
        .cancel_pending_tx(&mut tx, fx.integration_id, "write-back disabled")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(cancelled, 1, "the sent job is untouched");

    let (dead, _) = repo
        .list(fx.integration_id, Some("dead".to_string()), 0, 10)
        .await
        .unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].last_error.as_deref(), Some("write-back disabled"));
    let (still_pending, _) = repo
        .list(other.integration_id, Some("pending".to_string()), 0, 10)
        .await
        .unwrap();
    assert_eq!(still_pending.len(), 1);

    fx.cleanup(&pool).await;
    other.cleanup(&pool).await;
}

#[tokio::test]
async fn delete_finished_before_keeps_pending() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    for name in ["sent", "dead", "pending"] {
        repo.enqueue(fx.job(
            OutboundKind::Comment,
            &format!("{name}-{}", fx.integration_id),
        ))
        .await
        .unwrap();
    }
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    let id_of = |name: &str| {
        rows.iter()
            .find(|row| row.dedupe_key.starts_with(name))
            .unwrap()
            .id
    };
    repo.mark_sent(id_of("sent"), None).await.unwrap();
    repo.mark_dead(id_of("dead"), 6, "x".to_string())
        .await
        .unwrap();
    // Age everything by 100 days.
    sqlx::query(
        "UPDATE jira_outbound_jobs SET created_at = created_at - interval '100 days', \
         sent_at = sent_at - interval '100 days' WHERE integration_id = $1",
    )
    .bind(fx.integration_id)
    .execute(&pool)
    .await
    .unwrap();

    let deleted = repo
        .delete_finished_before(
            Utc::now() - Duration::days(30),
            Utc::now() - Duration::days(90),
        )
        .await
        .unwrap();
    assert!(deleted >= 2);
    let (left, total) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(left[0].status, "pending");

    fx.cleanup(&pool).await;
}

#[tokio::test]
async fn cancelled_job_is_not_revived_by_mark_retry() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let repo = jira_outbound_job_repository(pool.clone());
    let tx_repo = jira_outbound_job_repository_tx(pool.clone());
    repo.enqueue(fx.job(OutboundKind::Comment, &format!("rv-{}", fx.integration_id)))
        .await
        .unwrap();
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    let id = rows[0].id;
    assert!(repo.is_pending(id).await.unwrap());

    let mut conn = pool.acquire().await.unwrap();
    tx_repo
        .cancel_pending_tx(&mut conn, fx.integration_id, "write-back disabled")
        .await
        .unwrap();
    drop(conn);
    assert!(!repo.is_pending(id).await.unwrap());

    repo.mark_retry(id, 1, Utc::now(), "500: x".to_string())
        .await
        .unwrap();
    repo.mark_sent(id, None).await.unwrap();
    repo.mark_dead(id, 2, "other".to_string()).await.unwrap();
    let (rows, _) = repo.list(fx.integration_id, None, 0, 10).await.unwrap();
    assert_eq!(rows[0].status, "dead");
    assert_eq!(rows[0].attempts, 0);
    assert_eq!(rows[0].last_error.as_deref(), Some("write-back disabled"));
    assert!(rows[0].sent_at.is_none());

    fx.cleanup(&pool).await;
}

#[tokio::test]
#[serial(jira_jobs)]
async fn jobs_enqueued_in_one_tx_keep_insert_order() {
    let pool = init_pg_pool().await;
    let fx = fixture(&pool).await;
    let tx_repo = jira_outbound_job_repository_tx(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    for name in ["a", "b", "c"] {
        let job = fx.job(
            OutboundKind::Comment,
            &format!("{name}-{}", fx.integration_id),
        );
        assert!(tx_repo.enqueue_tx(&mut tx, job).await.unwrap());
    }
    tx.commit().await.unwrap();

    let repo = jira_outbound_job_repository(pool.clone());
    let claimed: Vec<_> = repo
        .claim_due(500, Duration::minutes(5))
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.integration_id == fx.integration_id)
        .collect();
    let mut by_created = claimed.clone();
    by_created.sort_by_key(|row| (row.created_at, row.id));
    let order: Vec<_> = by_created
        .iter()
        .map(|row| row.dedupe_key.split('-').next().unwrap().to_string())
        .collect();
    assert_eq!(order, vec!["a", "b", "c"]);
    assert!(by_created[0].created_at < by_created[1].created_at);
    assert!(by_created[1].created_at < by_created[2].created_at);

    fx.cleanup(&pool).await;
}
