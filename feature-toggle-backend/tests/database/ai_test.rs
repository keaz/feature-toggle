use feature_toggle_backend::Error;
use feature_toggle_backend::database::ai::{
    AiFeature, JudgmentResult, NewJudgment, TeamAiSettings, ai_judgment_repository,
    team_ai_settings_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::judgment::{JudgmentKind, SubjectType};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

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
    assert_eq!(second.attempts, 0);
    assert_eq!(second.input_hash, "h2");
    assert!(second.error.is_none());
    assert!(second.completed_at.is_none());

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn mark_done_with_stale_hash_changes_nothing() {
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
async fn list_retryable_picks_old_pending_and_failed_under_three_attempts() {
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
    let failed_twice = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "c"))
        .await
        .unwrap();
    let failed_thrice = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "d"))
        .await
        .unwrap();
    let done = repo
        .upsert_pending(new_judgment(team_id, Uuid::new_v4(), "e"))
        .await
        .unwrap();

    sqlx::query("UPDATE ai_judgments SET created_at = NOW() - INTERVAL '5 minutes' WHERE id = $1")
        .bind(old_pending.id)
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..2 {
        repo.mark_failed(failed_twice.id, "c".into(), "x".into())
            .await
            .unwrap();
    }
    for _ in 0..3 {
        repo.mark_failed(failed_thrice.id, "d".into(), "x".into())
            .await
            .unwrap();
    }
    repo.mark_done(done.id, "e".into(), result()).await.unwrap();

    let ids: Vec<Uuid> = repo
        .list_retryable(1000)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert!(ids.contains(&old_pending.id));
    assert!(ids.contains(&failed_twice.id));
    assert!(!ids.contains(&fresh_pending.id));
    assert!(!ids.contains(&failed_thrice.id));
    assert!(!ids.contains(&done.id));

    delete_team(&pool, team_id).await;
}
