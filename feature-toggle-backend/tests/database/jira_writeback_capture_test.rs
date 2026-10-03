//! JI-43: Source 2 of the Jira write-back outbox (`JiraWritebackCapture`) and
//! `ActivityLogRepository::list_window`, against the seeded test DB.
//!
//! The capture cursor is one global row, so these tests run serially.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{CreateFeature, feature_repository};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::database::jira_integration::jira_integration_repository_tx;
use feature_toggle_backend::logic::ActorContext;
use feature_toggle_backend::logic::jira_capture::CAPTURE_TYPES;
use feature_toggle_backend::logic::jira_integration_tx::{
    JiraIntegrationInput, create_jira_integration_in_tx,
};
use feature_toggle_backend::scheduler::JiraWritebackCapture;
use serial_test::serial;
use sqlx::PgPool;
use uuid::Uuid;

const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Team {
    team_id: Uuid,
    integration_id: Uuid,
    shadow_user: Uuid,
}

async fn team_with_integration(pool: &PgPool, writeback: bool) -> Team {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'jira capture')")
        .bind(team_id)
        .bind(format!("jira-capture-{team_id}"))
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
            name: "capture".to_string(),
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
    sqlx::query("UPDATE jira_integrations SET writeback_enabled = $2 WHERE id = $1")
        .bind(created.integration.id)
        .bind(writeback)
        .execute(pool)
        .await
        .expect("enable write-back");
    Team {
        team_id,
        integration_id: created.integration.id,
        shadow_user: created.integration.actor_user_id,
    }
}

async fn feature(pool: &PgPool, team_id: Uuid, link: Option<&str>) -> Uuid {
    let feature_id = feature_repository(pool.clone())
        .create_feature(CreateFeature {
            team_id,
            key: format!("capture-{}", Uuid::new_v4()),
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
        .expect("create feature");
    if let Some(key) = link {
        sqlx::query(
            "INSERT INTO feature_external_links (feature_id, system, external_key) \
             VALUES ($1, 'jira', $2)",
        )
        .bind(feature_id)
        .bind(key)
        .execute(pool)
        .await
        .expect("link feature");
    }
    feature_id
}

async fn set_cursor(pool: &PgPool, at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO jira_writeback_cursor (id, last_created_at) VALUES (TRUE, $1) \
         ON CONFLICT (id) DO UPDATE SET last_created_at = EXCLUDED.last_created_at",
    )
    .bind(at)
    .execute(pool)
    .await
    .expect("set cursor");
}

async fn cursor(pool: &PgPool) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT last_created_at FROM jira_writeback_cursor")
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn activity(
    pool: &PgPool,
    kind: &str,
    feature_id: Uuid,
    actor_id: Option<Uuid>,
    created_at: DateTime<Utc>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO activity_log (activity_type, entity_type, entity_id, actor_id, actor_name, \
         description, metadata, created_at) \
         VALUES ($1, 'stage', $2, $3, 'Jane Doe', 'test', $4, $5) RETURNING id",
    )
    .bind(kind)
    .bind(Uuid::new_v4().to_string())
    .bind(actor_id)
    .bind(serde_json::json!({
        "feature_id": feature_id.to_string(),
        "feature_key": "capture-key",
        "environment_name": "prod",
    }))
    .bind(created_at)
    .fetch_one(pool)
    .await
    .expect("insert activity")
}

async fn jobs(pool: &PgPool, integration_id: Uuid) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT kind, issue_key, dedupe_key FROM jira_outbound_jobs WHERE integration_id = $1 \
         ORDER BY created_at, id",
    )
    .bind(integration_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn capture(pool: &PgPool) -> JiraWritebackCapture {
    JiraWritebackCapture::new(pool.clone(), std::time::Duration::from_secs(5))
        .with_lag(Duration::zero())
}

async fn cleanup(pool: &PgPool, teams: &[&Team], features: &[Uuid]) {
    for feature_id in features {
        sqlx::query("DELETE FROM activity_log WHERE metadata->>'feature_id' = $1")
            .bind(feature_id.to_string())
            .execute(pool)
            .await
            .unwrap();
    }
    for team in teams {
        sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(team.team_id)
            .execute(pool)
            .await
            .expect("delete team");
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(team.shadow_user)
            .execute(pool)
            .await
            .expect("delete shadow user");
    }
}

#[tokio::test]
#[serial(jira_capture)]
async fn list_window_is_ascending_and_bounded() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, false).await;
    let feature_id = feature(&pool, team.team_id, None).await;
    // Activity types only this run uses: rows other tests left in the shared DB
    // cannot fall into the window.
    // `list_window` takes `&'static str`; leaking four short strings in a test is fine.
    let run = Uuid::new_v4().simple().to_string();
    let unique =
        |kind: &str| -> &'static str { Box::leak(format!("ct_{kind}_{run}").into_boxed_str()) };
    let (approved, rejected, deployed, other) = (
        unique("approved"),
        unique("rejected"),
        unique("deployed"),
        unique("other"),
    );
    let types = [approved, rejected, deployed];
    let base = Utc::now() - Duration::hours(3);
    // Inserted out of order; one of another type; one outside the window.
    let c = activity(
        &pool,
        deployed,
        feature_id,
        None,
        base + Duration::seconds(30),
    )
    .await;
    let a = activity(
        &pool,
        approved,
        feature_id,
        None,
        base + Duration::seconds(10),
    )
    .await;
    let b = activity(
        &pool,
        rejected,
        feature_id,
        None,
        base + Duration::seconds(10),
    )
    .await;
    activity(&pool, other, feature_id, None, base + Duration::seconds(20)).await;
    activity(
        &pool,
        deployed,
        feature_id,
        None,
        base + Duration::seconds(90),
    )
    .await;
    activity(
        &pool,
        deployed,
        feature_id,
        None,
        base - Duration::seconds(5),
    )
    .await;

    let repo = activity_log_repository(pool.clone());
    let from = base;
    let to = base + Duration::seconds(30);
    let rows = repo.list_window(&types, from, to, 10).await;
    let limited = repo.list_window(&types, from, to, 2).await;
    // Bounds are inclusive.
    let edge = repo.list_window(&[deployed], to, to, 10).await;
    // Clean up before asserting, so a failure leaves nothing behind.
    cleanup(&pool, &[&team], &[feature_id]).await;

    let rows = rows.unwrap();
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let (first, second) = if a < b { (a, b) } else { (b, a) };
    assert_eq!(
        ids,
        vec![first, second, c],
        "ascending by created_at, then id"
    );
    assert!(rows.iter().all(|row| row.activity_type != other));
    assert_eq!(limited.unwrap().len(), 2);
    assert_eq!(edge.unwrap().len(), 1);
}

#[tokio::test]
#[serial(jira_capture)]
async fn first_run_sets_the_cursor_and_enqueues_nothing() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    // Older activity is not backfilled.
    activity(
        &pool,
        "stage_deployed",
        feature_id,
        None,
        Utc::now() - Duration::minutes(5),
    )
    .await;
    sqlx::query("DELETE FROM jira_writeback_cursor")
        .execute(&pool)
        .await
        .unwrap();

    let before = Utc::now();
    assert_eq!(capture(&pool).run_once().await.unwrap(), 0);

    assert!(cursor(&pool).await.unwrap() >= before - Duration::seconds(1));
    assert!(jobs(&pool, team.integration_id).await.is_empty());
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn human_deploy_enqueues_comment_and_remote_link() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(1)).await;
    let row = activity(
        &pool,
        "stage_deployed",
        feature_id,
        Some(Uuid::parse_str(SEED_ADMIN_ID).unwrap()),
        Utc::now(),
    )
    .await;

    // The run reads the whole shared DB, so its total is not asserted: only this
    // integration's jobs are.
    capture(&pool).run_once().await.unwrap();

    let stored = jobs(&pool, team.integration_id).await;
    let kinds: Vec<&str> = stored.iter().map(|job| job.0.as_str()).collect();
    assert_eq!(kinds, vec!["comment", "remote_link"], "comment first");
    assert!(stored.iter().all(|job| job.1 == "PROJ-1"));
    assert_eq!(
        stored[0].2,
        format!("activity:{row}:{}:PROJ-1", team.integration_id)
    );
    let lines: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM jira_outbound_jobs WHERE integration_id = $1 AND kind = 'comment'",
    )
    .bind(team.integration_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(lines["lines"][1], "Deployed to prod by Jane Doe");
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn rereading_the_overlap_window_does_not_duplicate_jobs() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    let start = Utc::now() - Duration::minutes(1);
    set_cursor(&pool, start).await;
    activity(&pool, "stage_approved", feature_id, None, Utc::now()).await;

    capture(&pool).run_once().await.unwrap();
    assert_eq!(jobs(&pool, team.integration_id).await.len(), 2);
    // The cursor moved to the row; the next run re-reads it through the overlap.
    assert!(cursor(&pool).await.unwrap() > start);
    capture(&pool).run_once().await.unwrap();
    // Even with the cursor put back, the dedupe keys hold.
    set_cursor(&pool, start).await;
    capture(&pool).run_once().await.unwrap();
    assert_eq!(jobs(&pool, team.integration_id).await.len(), 2);
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn late_committed_row_inside_the_overlap_is_captured() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    let at = Utc::now() - Duration::seconds(5);
    set_cursor(&pool, at).await;
    // A transaction that started before the cursor and committed after it.
    activity(
        &pool,
        "stage_rollbacked",
        feature_id,
        None,
        at - Duration::seconds(30),
    )
    .await;

    capture(&pool).run_once().await.unwrap();

    assert_eq!(jobs(&pool, team.integration_id).await.len(), 2);
    assert!(
        cursor(&pool).await.unwrap() >= at,
        "the cursor never moves back"
    );
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn rows_of_unlinked_features_are_skipped() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, None).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(1)).await;
    activity(&pool, "stage_deployed", feature_id, None, Utc::now()).await;

    capture(&pool).run_once().await.unwrap();

    assert!(jobs(&pool, team.integration_id).await.is_empty());
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn other_teams_integrations_get_nothing() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let other = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(1)).await;
    activity(&pool, "stage_deployed", feature_id, None, Utc::now()).await;

    capture(&pool).run_once().await.unwrap();

    assert_eq!(jobs(&pool, team.integration_id).await.len(), 2);
    assert!(jobs(&pool, other.integration_id).await.is_empty());
    cleanup(&pool, &[&team, &other], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn jira_made_rows_and_write_back_off_teams_give_no_comment() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let off = team_with_integration(&pool, false).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    let off_feature = feature(&pool, off.team_id, Some("PROJ-2")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(1)).await;
    // The shadow user of an integration of ANOTHER team marks the row as Jira-made.
    activity(
        &pool,
        "stage_deployed",
        feature_id,
        Some(off.shadow_user),
        Utc::now(),
    )
    .await;
    activity(&pool, "stage_deployed", off_feature, None, Utc::now()).await;

    capture(&pool).run_once().await.unwrap();

    let stored = jobs(&pool, team.integration_id).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, "remote_link");
    assert!(jobs(&pool, off.integration_id).await.is_empty());
    cleanup(&pool, &[&team, &off], &[feature_id, off_feature]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn link_rows_target_their_issue_and_removal_deletes() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(1)).await;
    for (kind, key) in [
        ("external_link_added", "PROJ-9"),
        ("external_link_removed", "PROJ-8"),
    ] {
        sqlx::query(
            "INSERT INTO activity_log (activity_type, entity_type, entity_id, description, \
             metadata, created_at) VALUES ($1, 'feature', $2, 'link', $3, now())",
        )
        .bind(kind)
        .bind(feature_id.to_string())
        .bind(serde_json::json!({
            "feature_id": feature_id.to_string(),
            "feature_key": "k",
            "team_id": team.team_id.to_string(),
            "system": "jira",
            "external_key": key,
        }))
        .execute(&pool)
        .await
        .unwrap();
    }

    capture(&pool).run_once().await.unwrap();

    let mut stored: Vec<(String, String)> = jobs(&pool, team.integration_id)
        .await
        .into_iter()
        .map(|job| (job.0, job.1))
        .collect();
    stored.sort();
    assert_eq!(
        stored,
        vec![
            ("remote_link".to_string(), "PROJ-9".to_string()),
            ("remote_link_delete".to_string(), "PROJ-8".to_string()),
        ]
    );
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn more_than_500_rows_are_drained_in_one_run() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(30)).await;
    sqlx::query(
        "INSERT INTO activity_log (activity_type, entity_type, entity_id, actor_name, \
         description, metadata, created_at) \
         SELECT 'stage_deployed', 'stage', gen_random_uuid()::text, 'Jane Doe', 'bulk', \
                jsonb_build_object('feature_id', $1::text, 'environment_name', 'prod'), \
                now() - interval '20 minutes' + (g * interval '1 millisecond') \
         FROM generate_series(1, 1200) g",
    )
    .bind(feature_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    capture(&pool).run_once().await.unwrap();

    let comments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jira_outbound_jobs WHERE integration_id = $1 AND kind = 'comment'",
    )
    .bind(team.integration_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(comments, 1200);
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn link_removed_after_feature_deleted_enqueues_delete_without_feature_id() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(1)).await;
    sqlx::query(
        "INSERT INTO activity_log (activity_type, entity_type, entity_id, description, \
         metadata, created_at) VALUES ('external_link_removed', 'feature', $1, 'link', $2, now())",
    )
    .bind(feature_id.to_string())
    .bind(serde_json::json!({
        "feature_id": feature_id.to_string(),
        "feature_key": "gone",
        "team_id": team.team_id.to_string(),
        "system": "jira",
        "external_key": "PROJ-1",
    }))
    .execute(&pool)
    .await
    .unwrap();
    // The feature is deleted before the next capture tick.
    sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await
        .unwrap();

    let before = cursor(&pool).await.unwrap();
    capture(&pool)
        .run_once()
        .await
        .expect("capture does not fail");

    let rows: Vec<(String, Option<Uuid>, serde_json::Value)> = sqlx::query_as(
        "SELECT kind, feature_id, payload FROM jira_outbound_jobs WHERE integration_id = $1",
    )
    .bind(team.integration_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "remote_link_delete");
    assert_eq!(rows[0].1, None);
    assert_eq!(rows[0].2["featureId"], feature_id.to_string());
    assert!(cursor(&pool).await.unwrap() > before);
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn failing_row_does_not_block_the_cursor() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let bad = feature(&pool, team.team_id, Some("BAD-1")).await;
    let good = feature(&pool, team.team_id, Some("PROJ-1")).await;
    // Force the outbox insert of the bad issue to fail.
    sqlx::query("DROP TRIGGER IF EXISTS capture_test_fail ON jira_outbound_jobs")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE OR REPLACE FUNCTION capture_test_fail() RETURNS trigger AS $$ BEGIN \
         IF NEW.issue_key = 'BAD-1' THEN RAISE EXCEPTION 'forced failure'; END IF; \
         RETURN NEW; END $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER capture_test_fail BEFORE INSERT ON jira_outbound_jobs \
         FOR EACH ROW EXECUTE FUNCTION capture_test_fail()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let start = Utc::now() - Duration::minutes(1);
    set_cursor(&pool, start).await;
    let now = Utc::now();
    activity(
        &pool,
        "stage_deployed",
        bad,
        None,
        now - Duration::seconds(2),
    )
    .await;
    let last = now - Duration::seconds(1);
    activity(&pool, "stage_deployed", good, None, last).await;

    let result = capture(&pool).run_once().await;

    sqlx::query("DROP TRIGGER capture_test_fail ON jira_outbound_jobs")
        .execute(&pool)
        .await
        .unwrap();
    result.expect("a failing row does not fail the run");
    assert_eq!(
        jobs(&pool, team.integration_id).await.len(),
        2,
        "only the good row"
    );
    assert!(
        jobs(&pool, team.integration_id)
            .await
            .iter()
            .all(|job| job.1 == "PROJ-1")
    );
    assert!(
        cursor(&pool).await.unwrap() >= last,
        "the cursor advanced past both rows"
    );
    cleanup(&pool, &[&team], &[bad, good]).await;
}

#[tokio::test]
#[serial(jira_capture)]
async fn more_than_500_rows_with_the_same_created_at_are_all_captured() {
    let pool = init_pg_pool().await;
    let team = team_with_integration(&pool, true).await;
    let feature_id = feature(&pool, team.team_id, Some("PROJ-1")).await;
    set_cursor(&pool, Utc::now() - Duration::minutes(30)).await;
    // One transaction: every row gets the same created_at.
    sqlx::query(
        "INSERT INTO activity_log (activity_type, entity_type, entity_id, actor_name, \
         description, metadata, created_at) \
         SELECT 'stage_deployed', 'stage', gen_random_uuid()::text, 'Jane Doe', 'bulk', \
                jsonb_build_object('feature_id', $1::text, 'environment_name', 'prod'), \
                now() - interval '10 minutes' \
         FROM generate_series(1, 1300) g",
    )
    .bind(feature_id.to_string())
    .execute(&pool)
    .await
    .unwrap();

    capture(&pool).run_once().await.unwrap();

    let comments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jira_outbound_jobs WHERE integration_id = $1 AND kind = 'comment'",
    )
    .bind(team.integration_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(comments, 1300);
    cleanup(&pool, &[&team], &[feature_id]).await;
}

#[test]
fn capture_types_are_the_twelve_of_the_design() {
    assert_eq!(CAPTURE_TYPES.len(), 12);
}
