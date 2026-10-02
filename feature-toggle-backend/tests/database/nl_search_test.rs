use feature_toggle_backend::database::entity::FeatureType;
use feature_toggle_backend::database::feature::{CreateFeature, feature_repository};
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::model::FlagKindFilter;
use sqlx::PgPool;
use uuid::Uuid;

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, $3)")
        .bind(team_id)
        .bind(format!("NL search test team {team_id}"))
        .bind("nl search repository test")
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
    owner: Option<&str>,
    tags: &[&str],
    lifecycle_stage: &str,
) -> CreateFeature {
    CreateFeature {
        team_id,
        key: key.to_string(),
        description: Some("NL search test feature".to_string()),
        feature_type: FeatureType::Simple,
        lifecycle_stage: lifecycle_stage.to_string(),
        owner: owner.map(str::to_string),
        purpose: None,
        reference_url: None,
        expires_at: None,
        cleanup_reason: None,
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        flag_kind: None,
        stages: vec![],
        dependencies: vec![],
        variants: None,
    }
}

async fn set_usage(pool: &PgPool, id: Uuid, count_30d: i64) {
    sqlx::query("UPDATE features SET evaluation_count_30d = $2 WHERE id = $1")
        .bind(id)
        .bind(count_30d)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn top_owners_are_ranked_by_use_and_skip_archived_and_blank_owners() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    for (key, owner) in [
        ("a", Some("payments")),
        ("b", Some("payments")),
        ("c", Some("search")),
        ("d", Some("   ")),
        ("e", None),
    ] {
        repo.create_feature(new_feature(team_id, key, owner, &[], "active"))
            .await
            .unwrap();
    }
    repo.create_feature(new_feature(team_id, "old", Some("legacy"), &[], "archived"))
        .await
        .unwrap();
    sqlx::query("UPDATE features SET archived_at = NOW() WHERE team_id = $1 AND lifecycle_stage = 'archived'")
        .bind(team_id)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        repo.get_team_owner_candidates(team_id, 254).await.unwrap(),
        ["payments", "search"]
    );
    assert_eq!(
        repo.get_team_owner_candidates(team_id, 1).await.unwrap(),
        ["payments"]
    );

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn usage_search_orders_by_thirty_day_evaluations_and_applies_filters() {
    let pool = init_pg_pool().await;
    let repo = feature_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let quiet = repo
        .create_feature(new_feature(
            team_id,
            "quiet",
            Some("payments"),
            &["payments"],
            "active",
        ))
        .await
        .unwrap();
    let busy = repo
        .create_feature(new_feature(
            team_id,
            "busy",
            Some("payments"),
            &["payments"],
            "active",
        ))
        .await
        .unwrap();
    let middle = repo
        .create_feature(new_feature(
            team_id,
            "middle",
            Some("search"),
            &["search"],
            "active",
        ))
        .await
        .unwrap();
    let archived = repo
        .create_feature(new_feature(
            team_id,
            "gone",
            Some("payments"),
            &["payments"],
            "archived",
        ))
        .await
        .unwrap();
    set_usage(&pool, quiet, 1).await;
    set_usage(&pool, busy, 900).await;
    set_usage(&pool, middle, 50).await;
    set_usage(&pool, archived, 5000).await;

    let ids = |features: Vec<feature_toggle_backend::database::entity::Feature>| {
        features.into_iter().map(|f| f.id).collect::<Vec<_>>()
    };
    let search = |tag: Option<&str>, owner: Option<&str>, stage: Option<&str>, limit: i64| {
        let repo = &repo;
        let tag = tag.map(str::to_string);
        let owner = owner.map(str::to_string);
        let stage = stage.map(str::to_string);
        async move {
            ids(repo
                .get_features_by_usage_filtered(
                    team_id, None, stage, None, owner, None, tag, None, None, None, limit,
                )
                .await
                .unwrap())
        }
    };

    // Archived features stay hidden unless the stage asks for them.
    assert_eq!(search(None, None, None, 50).await, [busy, middle, quiet]);
    assert_eq!(search(None, None, None, 2).await, [busy, middle]);
    assert_eq!(
        search(Some("payments"), None, None, 50).await,
        [busy, quiet]
    );
    assert_eq!(search(None, Some("search"), None, 50).await, [middle]);
    assert_eq!(search(None, None, Some("archived"), 50).await, [archived]);

    // The flag kind filter reaches the same query.
    let unclassified = repo
        .get_features_by_usage_filtered(
            team_id,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(FlagKindFilter::Unclassified),
            50,
        )
        .await
        .unwrap();
    assert_eq!(unclassified.len(), 3);

    delete_team(&pool, team_id).await;
}
