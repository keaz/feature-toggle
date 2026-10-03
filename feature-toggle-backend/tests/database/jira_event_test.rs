//! JI-15: the Jira event log (`jira_integration_events`).

use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jira_event::{NewJiraEvent, jira_event_repository};
use feature_toggle_backend::database::jira_integration::jira_integration_repository_tx;
use feature_toggle_backend::database::jira_outbound_job::{
    NewOutboundJob, OutboundKind, jira_outbound_job_repository,
};
use feature_toggle_backend::logic::ActorContext;
use feature_toggle_backend::logic::jira_integration_tx::{
    JiraIntegrationInput, create_jira_integration_in_tx,
};
use sqlx::PgPool;
use uuid::Uuid;

const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

/// A team with one integration; returns (team id, integration id, shadow user).
async fn integration(pool: &PgPool, name: &str) -> (Uuid, Uuid, Uuid) {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira events')")
        .bind(team_id)
        .bind(format!("jira-event-test-{team_id}"))
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
            name: name.to_string(),
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
    let integration = created.integration;
    (team_id, integration.id, integration.actor_user_id)
}

async fn cleanup(pool: &PgPool, team_id: Uuid, shadow_user: Uuid) {
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("delete team");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(shadow_user)
        .execute(pool)
        .await
        .expect("delete shadow user");
}

fn event(integration_id: Uuid, hash: &str) -> NewJiraEvent {
    NewJiraEvent {
        id: Uuid::new_v4(),
        integration_id,
        issue_key: Some("PROJ-1".to_string()),
        jira_status: Some("Done".to_string()),
        jira_actor: Some(serde_json::json!({"displayName": "Jane Doe"})),
        delivery_hash: Some(hash.to_string()),
        results: serde_json::json!([{"featureKey": "checkout", "outcome": "applied"}]),
        unknown_environments: vec!["Lab".to_string()],
        unknown_features: vec![],
        ignored: None,
        error: None,
    }
}

#[tokio::test]
async fn insert_and_find_a_recent_delivery() {
    let pool = init_pg_pool().await;
    let (team, id, user) = integration(&pool, "events-a").await;
    let (other_team, other, other_user) = integration(&pool, "events-b").await;
    let repo = jira_event_repository(pool.clone());

    let stored = repo.insert(event(id, "h1")).await.expect("insert");
    assert_eq!(stored.integration_id, id);
    assert_eq!(stored.results[0]["featureKey"], "checkout");
    assert_eq!(stored.unknown_environments, vec!["Lab".to_string()]);

    let since = Utc::now() - Duration::minutes(10);
    let found = repo.find_recent_delivery(id, "h1", since).await.unwrap();
    assert_eq!(found.map(|row| row.id), Some(stored.id));
    // Another hash, another integration, or an older window: no match.
    assert!(
        repo.find_recent_delivery(id, "h2", since)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.find_recent_delivery(other, "h1", since)
            .await
            .unwrap()
            .is_none()
    );
    let later = Utc::now() + Duration::minutes(1);
    assert!(
        repo.find_recent_delivery(id, "h1", later)
            .await
            .unwrap()
            .is_none()
    );

    // A delivery that failed is processed again, not replayed.
    let mut failed = event(id, "h3");
    failed.error = Some("database error".to_string());
    repo.insert(failed).await.unwrap();
    assert!(
        repo.find_recent_delivery(id, "h3", since)
            .await
            .unwrap()
            .is_none()
    );

    cleanup(&pool, team, user).await;
    cleanup(&pool, other_team, other_user).await;
}

#[tokio::test]
async fn list_is_newest_first_with_a_total() {
    let pool = init_pg_pool().await;
    let (team, id, user) = integration(&pool, "events-list").await;
    let repo = jira_event_repository(pool.clone());
    let mut ids = Vec::new();
    for hash in ["a", "b", "c"] {
        ids.push(repo.insert(event(id, hash)).await.unwrap().id);
    }

    let (page, total) = repo.list(id, 0, 2).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(
        page.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1]]
    );
    let (rest, _) = repo.list(id, 2, 2).await.unwrap();
    assert_eq!(
        rest.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![ids[0]]
    );

    cleanup(&pool, team, user).await;
}

#[tokio::test]
async fn delete_older_than_removes_only_old_events() {
    let pool = init_pg_pool().await;
    let (team, id, user) = integration(&pool, "events-cleanup").await;
    let repo = jira_event_repository(pool.clone());
    let old = repo.insert(event(id, "old")).await.unwrap();
    let new = repo.insert(event(id, "new")).await.unwrap();
    sqlx::query(
        "UPDATE jira_integration_events SET received_at = NOW() - INTERVAL '31 days' WHERE id = $1",
    )
    .bind(old.id)
    .execute(&pool)
    .await
    .unwrap();

    let deleted = repo
        .delete_older_than(Utc::now() - Duration::days(30))
        .await
        .unwrap();

    assert!(deleted >= 1);
    let (left, total) = repo.list(id, 0, 10).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(left[0].id, new.id);
    cleanup(&pool, team, user).await;
}

fn comment_job(integration_id: Uuid, dedupe_key: &str) -> NewOutboundJob {
    NewOutboundJob {
        integration_id,
        issue_key: "PROJ-1".to_string(),
        feature_id: None,
        kind: OutboundKind::Comment,
        payload: serde_json::json!({"lines": ["FluxGate: test"]}),
        dedupe_key: dedupe_key.to_string(),
    }
}

async fn job_count(pool: &PgPool, dedupe_key: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM jira_outbound_jobs WHERE dedupe_key = $1")
        .bind(dedupe_key)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn insert_with_jobs_is_atomic() {
    let pool = init_pg_pool().await;
    let (team, id, user) = integration(&pool, "events-atomic").await;
    let repo = jira_event_repository(pool.clone());
    let jobs = jira_outbound_job_repository(pool.clone());

    // Event and job are stored together.
    let first = event(id, "atomic-1");
    let key = format!("event:{}", first.id);
    let stored = repo
        .insert_with_jobs(first.clone(), vec![comment_job(id, &key)])
        .await
        .expect("insert with jobs");
    assert_eq!(stored.id, first.id, "the caller's id is used");
    assert_eq!(job_count(&pool, &key).await, 1);

    // A dedupe key that is already taken does not fail the call; the event is stored.
    let taken = format!("taken-{}", Uuid::new_v4());
    assert!(jobs.enqueue(comment_job(id, &taken)).await.unwrap());
    let second = event(id, "atomic-2");
    let stored = repo
        .insert_with_jobs(second.clone(), vec![comment_job(id, &taken)])
        .await
        .expect("a taken dedupe key is not an error");
    assert_eq!(stored.id, second.id);
    assert_eq!(job_count(&pool, &taken).await, 1);

    // A job that fails (unknown integration: foreign key) rolls the event back too.
    let third = event(id, "atomic-3");
    let ok_key = format!("rolled-back-{}", Uuid::new_v4());
    let result = repo
        .insert_with_jobs(
            third.clone(),
            vec![
                comment_job(id, &ok_key),
                comment_job(Uuid::new_v4(), &format!("bad-{}", Uuid::new_v4())),
            ],
        )
        .await;
    assert!(result.is_err());
    let events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jira_integration_events WHERE id = $1")
            .bind(third.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 0, "event rolled back");
    assert_eq!(job_count(&pool, &ok_key).await, 0, "first job rolled back");

    cleanup(&pool, team, user).await;
}
