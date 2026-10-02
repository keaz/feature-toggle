use feature_toggle_backend::Error;
use feature_toggle_backend::database::ai::{
    AiFeature, JudgmentResult, NewJudgment, TeamAiSettings, ai_judgment_repository,
    team_ai_settings_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::judgment::{JudgmentKind, SubjectType};
use serde_json::json;
use sqlx::PgPool;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use uuid::Uuid;

/// `claim_retryable` claims rows across all teams, so tests that create or claim
/// judgments run one at a time.
fn ai_judgment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("AI test team {team_id}"))
        .bind("ai repository test")
        .execute(pool)
        .await
        .expect("failed to insert team");
    team_id
}

async fn delete_team(pool: &PgPool, team_id: Uuid) {
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("failed to delete team");
}

fn new_judgment(team_id: Uuid, subject_id: Uuid, hash: &str) -> NewJudgment {
    NewJudgment {
        team_id,
        kind: JudgmentKind::FlagKind,
        subject_type: SubjectType::Feature,
        subject_id,
        input: json!({ "feature": { "key": "checkout-v2" } }),
        input_hash: hash.to_string(),
    }
}

fn result() -> JudgmentResult {
    JudgmentResult {
        model: "jev-1.13.0".to_string(),
        raw_answers: json!({ "kind": { "type": "noul", "noul": 0.9 } }),
        derived: json!({ "level": "low" }),
        input_tokens: Some(120),
    }
}

#[tokio::test]
async fn settings_default_to_all_off_and_upsert_persists() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = team_ai_settings_repository(pool.clone());

    let empty = repo.get(team_id).await.unwrap();
    assert_eq!(empty.settings, TeamAiSettings::default());
    assert!(empty.updated_at.is_none());

    let actor = Uuid::new_v4();
    let wanted = TeamAiSettings {
        approval_risk: true,
        justification_check: false,
        flag_kind: true,
        nl_search: false,
    };
    let saved = repo
        .upsert(team_id, wanted.clone(), Some(actor))
        .await
        .unwrap();
    assert_eq!(saved.settings, wanted);
    assert_eq!(saved.updated_by, Some(actor));
    assert!(saved.updated_at.is_some());

    let loaded = repo.get(team_id).await.unwrap();
    assert_eq!(loaded.settings, wanted);
    assert!(loaded.settings.is_enabled(AiFeature::FlagKind));
    assert!(!loaded.settings.is_enabled(AiFeature::NlSearch));

    let off = repo
        .upsert(team_id, TeamAiSettings::default(), None)
        .await
        .unwrap();
    assert_eq!(off.settings, TeamAiSettings::default());

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn upsert_unknown_team_is_not_found() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let repo = team_ai_settings_repository(pool);
    let missing = Uuid::new_v4();
    let error = repo
        .upsert(missing, TeamAiSettings::default(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::NotFound(id) if id == missing),
        "{error:?}"
    );
}

#[tokio::test]
async fn upsert_pending_resets_an_existing_row() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());
    let subject_id = Uuid::new_v4();

    let first = repo
        .upsert_pending(new_judgment(team_id, subject_id, "h1"))
        .await
        .unwrap();
    assert_eq!(first.status, "pending");
    assert!(
        repo.mark_failed(first.id, "h1".into(), "boom".into())
            .await
            .unwrap()
    );

    let second = repo
        .upsert_pending(new_judgment(team_id, subject_id, "h2"))
        .await
        .unwrap();
    assert_eq!(
        second.id, first.id,
        "the unique key keeps one row per subject and kind"
    );
    assert_eq!(second.status, "pending");
    assert_eq!(
        second.attempts, 1,
        "the initial run counts as the first attempt"
    );
    assert_eq!(second.input_hash, "h2");
    assert!(second.error.is_none());
    assert!(second.completed_at.is_none());

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn mark_done_with_stale_hash_changes_nothing() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());
    let subject_id = Uuid::new_v4();
    let row = repo
        .upsert_pending(new_judgment(team_id, subject_id, "new"))
        .await
        .unwrap();

    assert!(
        !repo
            .mark_done(row.id, "old".into(), result())
            .await
            .unwrap()
    );
    let unchanged = repo
        .get_for_subject(SubjectType::Feature, subject_id, JudgmentKind::FlagKind)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.status, "pending");
    assert!(unchanged.derived.is_none());

    assert!(
        repo.mark_done(row.id, "new".into(), result())
            .await
            .unwrap()
    );
    let done = repo
        .get_for_subject(SubjectType::Feature, subject_id, JudgmentKind::FlagKind)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "done");
    assert_eq!(done.model.as_deref(), Some("jev-1.13.0"));
    assert_eq!(done.derived, Some(json!({ "level": "low" })));
    assert_eq!(done.input_tokens, Some(120));
    assert!(done.completed_at.is_some());

    // A late failure never overwrites a finished judgment.
    assert!(
        !repo
            .mark_failed(row.id, "new".into(), "late".into())
            .await
            .unwrap()
    );

    let many = repo
        .get_for_subjects(
            SubjectType::Feature,
            vec![subject_id, Uuid::new_v4()],
            JudgmentKind::FlagKind,
        )
        .await
        .unwrap();
    assert_eq!(many.len(), 1);

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn mark_done_is_applied_only_once() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());
    let row = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "h"))
        .await
        .unwrap();

    assert!(repo.mark_done(row.id, "h".into(), result()).await.unwrap());
    assert!(
        !repo.mark_done(row.id, "h".into(), result()).await.unwrap(),
        "a second run of the same input must not be stored or applied again"
    );

    delete_team(&pool, team_id).await;
}

async fn set_attempts(pool: &PgPool, id: Uuid, status: &str, attempts: i32) {
    sqlx::query("UPDATE ai_judgments SET status = $2, attempts = $3 WHERE id = $1")
        .bind(id)
        .bind(status)
        .bind(attempts)
        .execute(pool)
        .await
        .unwrap();
}

async fn age(pool: &PgPool, id: Uuid) {
    sqlx::query("UPDATE ai_judgments SET created_at = NOW() - INTERVAL '5 minutes' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn claim_retryable_picks_old_pending_and_failed_under_three_attempts() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());

    let fresh_pending = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "a"))
        .await
        .unwrap();
    let old_pending = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "b"))
        .await
        .unwrap();
    let failed_once = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "c"))
        .await
        .unwrap();
    let exhausted = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "d"))
        .await
        .unwrap();
    let done = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "e"))
        .await
        .unwrap();

    age(&pool, old_pending.id).await;
    assert!(
        repo.mark_failed(failed_once.id, "c".into(), "x".into())
            .await
            .unwrap()
    );
    set_attempts(&pool, exhausted.id, "failed", 3).await;
    repo.mark_done(done.id, "e".into(), result()).await.unwrap();

    let claimed = repo.claim_retryable(1000).await.unwrap();
    let ids: Vec<Uuid> = claimed.iter().map(|row| row.id).collect();
    assert!(ids.contains(&old_pending.id));
    assert!(ids.contains(&failed_once.id));
    assert!(!ids.contains(&fresh_pending.id));
    assert!(!ids.contains(&exhausted.id));
    assert!(!ids.contains(&done.id));
    for row in claimed.iter().filter(|row| row.team_id == team_id) {
        assert_eq!(row.attempts, 2, "claiming a row counts the retry attempt");
    }

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn stuck_pending_row_stops_after_three_attempts() {
    let _guard = ai_judgment_lock().lock().await;
    let pool = init_pg_pool().await;
    let team_id = insert_team(&pool).await;
    let repo = ai_judgment_repository(pool.clone());
    let stuck = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "s"))
        .await
        .unwrap();
    age(&pool, stuck.id).await;

    let claimed_ids = |rows: Vec<feature_toggle_backend::database::ai::AiJudgment>| -> Vec<Uuid> {
        rows.into_iter().map(|row| row.id).collect()
    };
    assert!(claimed_ids(repo.claim_retryable(1000).await.unwrap()).contains(&stuck.id));
    assert!(claimed_ids(repo.claim_retryable(1000).await.unwrap()).contains(&stuck.id));
    assert!(
        !claimed_ids(repo.claim_retryable(1000).await.unwrap()).contains(&stuck.id),
        "a row whose result can never be stored must stop after 3 attempts"
    );

    delete_team(&pool, team_id).await;
}
