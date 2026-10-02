use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{
    CreateFeature, FeatureRepository, UpdateFeature, feature_repository,
};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::model::{FlagKind, FlagKindFilter, FlagKindSource};
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("Flag kind test team {team_id}"))
        .bind("flag kind repository test")
        .execute(pool)
        .await
        .expect("failed to insert team");
    team_id
}

async fn delete_team(pool: &PgPool, team_id: Uuid) {
    sqlx::query("DELETE FROM features WHERE team_id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("failed to delete features");
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("failed to delete team");
}

fn new_feature(
    team_id: Uuid,
    key: &str,
    tags: &[&str],
    flag_kind: Option<FlagKind>,
    lifecycle_stage: &str,
) -> CreateFeature {
    CreateFeature {
        team_id,
        key: format!("{key}-{}", Uuid::new_v4()),
        description: Some("Flag kind test feature".to_string()),
        feature_type: FeatureType::Simple,
        lifecycle_stage: lifecycle_stage.to_string(),
        owner: None,
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        flag_kind,
        stages: vec![],
        dependencies: vec![],
        variants: None,
    }
}

fn update_with(id: Uuid, key: String, flag_kind: Option<Option<FlagKind>>) -> UpdateFeature {
    UpdateFeature {
        id,
        key: Some(key),
        description: None,
        feature_type: None,
        lifecycle_stage: None,
        owner: None,
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: None,
        flag_kind,
        archive_confirmation: false,
        stages: vec![],
        dependencies: vec![],
        variants: None,
    }
}

#[tokio::test]
async fn create_stores_a_user_chosen_kind_and_leaves_the_rest_unclassified() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;

    let chosen = repo
        .create_feature(new_feature(
            team_id,
            "chosen",
            &[],
            Some(FlagKind::Ops),
            "active",
        ))
        .await
        .unwrap();
    let plain = repo
        .create_feature(new_feature(team_id, "plain", &[], None, "active"))
        .await
        .unwrap();

    let chosen = repo.get_feature_by_id(chosen).await.unwrap();
    assert_eq!(chosen.flag_kind, Some(FlagKind::Ops));
    assert_eq!(chosen.flag_kind_source, Some(FlagKindSource::User));
    assert_eq!(chosen.flag_kind_confidence, None);
    let plain = repo.get_feature_by_id(plain).await.unwrap();
    assert_eq!(plain.flag_kind, None);
    assert_eq!(plain.flag_kind_source, None);

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn update_follows_the_user_choice_rules() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let input = new_feature(team_id, "update", &[], None, "active");
    let key = input.key.clone();
    let id = repo.create_feature(input).await.unwrap();
    assert!(
        repo.set_ai_flag_kind(id, FlagKind::Release, 0.8)
            .await
            .unwrap()
    );

    // Absent: unchanged.
    repo.update_feature(update_with(id, key.clone(), None))
        .await
        .unwrap();
    let feature = repo.get_feature_by_id(id).await.unwrap();
    assert_eq!(feature.flag_kind, Some(FlagKind::Release));
    assert_eq!(feature.flag_kind_source, Some(FlagKindSource::Ai));
    assert!(feature.flag_kind_confidence.is_some());

    // Same value: the source stays `ai`.
    repo.update_feature(update_with(id, key.clone(), Some(Some(FlagKind::Release))))
        .await
        .unwrap();
    let feature = repo.get_feature_by_id(id).await.unwrap();
    assert_eq!(feature.flag_kind_source, Some(FlagKindSource::Ai));
    assert!(feature.flag_kind_confidence.is_some());

    // A different value: the user's choice, with no confidence.
    repo.update_feature(update_with(id, key.clone(), Some(Some(FlagKind::Config))))
        .await
        .unwrap();
    let feature = repo.get_feature_by_id(id).await.unwrap();
    assert_eq!(feature.flag_kind, Some(FlagKind::Config));
    assert_eq!(feature.flag_kind_source, Some(FlagKindSource::User));
    assert_eq!(feature.flag_kind_confidence, None);

    // AI cannot overwrite it now.
    assert!(
        !repo
            .set_ai_flag_kind(id, FlagKind::Ops, 0.99)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.get_feature_by_id(id).await.unwrap().flag_kind,
        Some(FlagKind::Config)
    );

    // `null` clears it and still counts as the user's choice.
    repo.update_feature(update_with(id, key, Some(None)))
        .await
        .unwrap();
    let feature = repo.get_feature_by_id(id).await.unwrap();
    assert_eq!(feature.flag_kind, None);
    assert_eq!(feature.flag_kind_source, Some(FlagKindSource::User));
    assert!(
        !repo
            .set_ai_flag_kind(id, FlagKind::Ops, 0.99)
            .await
            .unwrap()
    );

    delete_team(&pool, team_id).await;
}

async fn list_with(
    repo: &dyn FeatureRepository,
    team_id: Uuid,
    filter: Option<FlagKindFilter>,
) -> Vec<Uuid> {
    let (features, total) = repo
        .get_features_with_offset_filtered(
            team_id, None, None, None, None, false, None, None, None, None, None, filter, 0, 100,
        )
        .await
        .unwrap();
    assert_eq!(total as usize, features.len());
    let mut ids: Vec<Uuid> = features.iter().map(|feature| feature.id).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn list_filter_matches_each_kind_and_unclassified() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;

    let mut by_kind = Vec::new();
    for kind in FlagKind::ALL {
        let id = repo
            .create_feature(new_feature(
                team_id,
                kind.as_str(),
                &[],
                Some(kind),
                "active",
            ))
            .await
            .unwrap();
        by_kind.push((kind, id));
    }
    let unclassified = repo
        .create_feature(new_feature(team_id, "none", &[], None, "active"))
        .await
        .unwrap();

    for (kind, id) in &by_kind {
        assert_eq!(
            list_with(repo.as_ref(), team_id, Some(FlagKindFilter::Kind(*kind))).await,
            vec![*id],
            "{kind:?}"
        );
    }
    assert_eq!(
        list_with(repo.as_ref(), team_id, Some(FlagKindFilter::Unclassified)).await,
        vec![unclassified]
    );
    assert_eq!(list_with(repo.as_ref(), team_id, None).await.len(), 6);

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn tag_candidates_are_ranked_by_use_and_skip_excluded_tags() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    for tags in [
        &["payments", "billing"][..],
        &["payments", "ops"][..],
        &["payments", "billing", "beta"][..],
    ] {
        repo.create_feature(new_feature(team_id, "tagged", tags, None, "active"))
            .await
            .unwrap();
    }
    // An archived feature's tags do not count.
    repo.create_feature(new_feature(team_id, "old", &["legacy"], None, "archived"))
        .await
        .unwrap();
    sqlx::query("UPDATE features SET archived_at = NOW() WHERE team_id = $1 AND lifecycle_stage = 'archived'")
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();

    let all = repo
        .get_team_tag_candidates(team_id, vec![], 50)
        .await
        .unwrap();
    assert_eq!(all, ["payments", "billing", "beta", "ops"]);

    let without_existing = repo
        .get_team_tag_candidates(team_id, vec!["payments".to_string()], 50)
        .await
        .unwrap();
    assert_eq!(without_existing, ["billing", "beta", "ops"]);

    // The limit applies after the exclusion.
    let limited = repo
        .get_team_tag_candidates(team_id, vec!["payments".to_string()], 2)
        .await
        .unwrap();
    assert_eq!(limited, ["billing", "beta"]);

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn backfill_selection_skips_classified_archived_and_user_decided_features() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;

    let wanted = repo
        .create_feature(new_feature(team_id, "wanted", &[], None, "active"))
        .await
        .unwrap();
    let ai_classified = repo
        .create_feature(new_feature(team_id, "ai", &[], None, "active"))
        .await
        .unwrap();
    repo.set_ai_flag_kind(ai_classified, FlagKind::Ops, 0.9)
        .await
        .unwrap();
    repo.create_feature(new_feature(
        team_id,
        "user",
        &[],
        Some(FlagKind::Config),
        "active",
    ))
    .await
    .unwrap();
    let cleared = repo
        .create_feature(new_feature(team_id, "cleared", &[], None, "active"))
        .await
        .unwrap();
    sqlx::query("UPDATE features SET flag_kind_source = 'user' WHERE id = $1")
        .bind(cleared)
        .execute(&pool)
        .await
        .unwrap();
    let archived = repo
        .create_feature(new_feature(team_id, "archived", &[], None, "archived"))
        .await
        .unwrap();
    assert!(
        repo.get_feature_by_id(archived)
            .await
            .unwrap()
            .archived_at
            .is_some()
    );

    let needing = repo
        .get_features_needing_flag_kind(team_id, 500)
        .await
        .unwrap();
    assert_eq!(needing.iter().map(|f| f.id).collect::<Vec<_>>(), [wanted]);

    let limited = repo
        .get_features_needing_flag_kind(team_id, 0)
        .await
        .unwrap();
    assert!(limited.is_empty());

    delete_team(&pool, team_id).await;
}
