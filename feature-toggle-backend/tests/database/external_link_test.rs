use feature_toggle_backend::Error;
use feature_toggle_backend::database::activity_log::activity_log_repository;
use feature_toggle_backend::database::external_link::{
    CreateExternalLink, ExternalLinkRepository, ExternalLinkRepositoryTx, external_link_repository,
    external_link_repository_tx,
};
use feature_toggle_backend::database::feature::feature_repository;
use feature_toggle_backend::database::init_pg_pool;
use feature_toggle_backend::logic::ActorContext;
use feature_toggle_backend::logic::external_link::NewExternalLink;
use feature_toggle_backend::logic::external_link_tx::{
    create_external_link_in_tx, delete_external_link_in_tx,
};
use feature_toggle_backend::utils::activity_logger::activity_types::{
    EXTERNAL_LINK_ADDED, EXTERNAL_LINK_REMOVED,
};
use sqlx::PgPool;
use uuid::Uuid;

/// Seeded admin user from `init.sql`.
const SEED_ADMIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

async fn insert_team(pool: &PgPool) -> Uuid {
    let team_id = Uuid::new_v4();
    sqlx::query("INSERT INTO teams (id, name, description) VALUES ($1, $2, 'external link test')")
        .bind(team_id)
        .bind(format!("external-link-test-{team_id}"))
        .execute(pool)
        .await
        .expect("insert team");
    team_id
}

async fn insert_feature(pool: &PgPool, team_id: Uuid, key: &str) -> Uuid {
    let feature_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO features (id, key, feature_type, team_id) VALUES ($1, $2, 'Simple', $3)",
    )
    .bind(feature_id)
    .bind(key)
    .bind(team_id)
    .execute(pool)
    .await
    .expect("insert feature");
    feature_id
}

async fn delete_team(pool: &PgPool, team_id: Uuid) {
    sqlx::query("DELETE FROM features WHERE team_id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("delete features");
    sqlx::query("DELETE FROM teams WHERE id = $1")
        .bind(team_id)
        .execute(pool)
        .await
        .expect("delete team");
}

fn jira_link(feature_id: Uuid, key: &str) -> CreateExternalLink {
    CreateExternalLink {
        feature_id,
        system: "jira".to_string(),
        external_key: key.to_string(),
        url: Some(format!("https://acme.atlassian.net/browse/{key}")),
        created_by: Some(Uuid::parse_str(SEED_ADMIN_ID).unwrap()),
    }
}

/// A key no other test run uses, so parallel and repeated runs never collide.
fn unique_key() -> String {
    let project = Uuid::new_v4().simple().to_string()[..8].to_uppercase();
    let number = Uuid::new_v4().as_u128() % 1_000_000_000 + 1;
    format!("EXT{project}-{number}")
}

#[tokio::test]
async fn create_list_and_delete_external_link() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_id = insert_feature(&pool, team_id, "ext-link-crud").await;
    let key = unique_key();

    let created = repository
        .create(jira_link(feature_id, &key))
        .await
        .expect("create link");
    assert_eq!(created.feature_id, feature_id);
    assert_eq!(created.system, "jira");
    assert_eq!(created.external_key, key);
    assert_eq!(
        created.url.as_deref(),
        Some(format!("https://acme.atlassian.net/browse/{key}").as_str())
    );
    assert_eq!(
        created.created_by,
        Some(Uuid::parse_str(SEED_ADMIN_ID).unwrap())
    );

    let listed = repository
        .list_for_feature(feature_id)
        .await
        .expect("list links");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.id);

    assert!(
        repository
            .delete(feature_id, created.id)
            .await
            .expect("delete link")
    );
    assert!(
        !repository
            .delete(feature_id, created.id)
            .await
            .expect("second delete")
    );
    assert!(
        repository
            .list_for_feature(feature_id)
            .await
            .expect("list after delete")
            .is_empty()
    );

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn delete_only_removes_a_link_of_the_given_feature() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_a = insert_feature(&pool, team_id, "ext-link-a").await;
    let feature_b = insert_feature(&pool, team_id, "ext-link-b").await;

    let link = repository
        .create(jira_link(feature_a, &unique_key()))
        .await
        .expect("create link");

    assert!(
        !repository
            .delete(feature_b, link.id)
            .await
            .expect("delete with other feature")
    );
    assert_eq!(
        repository
            .list_for_feature(feature_a)
            .await
            .expect("list")
            .len(),
        1
    );

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn duplicate_link_is_a_conflict() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_id = insert_feature(&pool, team_id, "ext-link-dup").await;
    let key = unique_key();

    repository
        .create(jira_link(feature_id, &key))
        .await
        .expect("first create");
    let duplicate = repository.create(jira_link(feature_id, &key)).await;
    assert!(
        matches!(duplicate, Err(Error::RecordAlreadyExists(_))),
        "expected RecordAlreadyExists, got {duplicate:?}"
    );

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn links_are_deleted_with_their_feature() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_id = insert_feature(&pool, team_id, "ext-link-cascade").await;
    let link = repository
        .create(jira_link(feature_id, &unique_key()))
        .await
        .expect("create link");

    sqlx::query("DELETE FROM features WHERE id = $1")
        .bind(feature_id)
        .execute(&pool)
        .await
        .expect("delete feature");

    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM feature_external_links WHERE id = $1")
            .bind(link.id)
            .fetch_one(&pool)
            .await
            .expect("count links");
    assert_eq!(remaining, 0);

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn feature_ids_for_key_is_team_scoped() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_a = insert_team(&pool).await;
    let team_b = insert_team(&pool).await;
    let feature_a1 = insert_feature(&pool, team_a, "ext-link-a1").await;
    let feature_a2 = insert_feature(&pool, team_a, "ext-link-a2").await;
    let feature_b = insert_feature(&pool, team_b, "ext-link-b1").await;
    let key = unique_key();

    for feature_id in [feature_a1, feature_a2, feature_b] {
        repository
            .create(jira_link(feature_id, &key))
            .await
            .expect("create link");
    }

    let mut team_a_ids = repository
        .feature_ids_for_key(team_a, "jira", &key)
        .await
        .expect("team a ids");
    team_a_ids.sort();
    let mut expected = vec![feature_a1, feature_a2];
    expected.sort();
    assert_eq!(team_a_ids, expected);

    let team_b_ids = repository
        .feature_ids_for_key(team_b, "jira", &key)
        .await
        .expect("team b ids");
    assert_eq!(team_b_ids, vec![feature_b]);

    assert!(
        repository
            .feature_ids_for_key(team_a, "jira", &unique_key())
            .await
            .expect("unknown key")
            .is_empty()
    );

    delete_team(&pool, team_a).await;
    delete_team(&pool, team_b).await;
}

#[tokio::test]
async fn feature_scope_returns_team_and_key() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_id = insert_feature(&pool, team_id, "ext-link-scope").await;

    let scope = repository
        .feature_scope(feature_id)
        .await
        .expect("scope")
        .expect("feature exists");
    assert_eq!(scope.team_id, team_id);
    assert_eq!(scope.key, "ext-link-scope");
    assert!(
        repository
            .feature_scope(Uuid::new_v4())
            .await
            .expect("missing scope")
            .is_none()
    );

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn tx_writes_roll_back() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository_tx(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_id = insert_feature(&pool, team_id, "ext-link-tx").await;
    let committed = repository
        .create(jira_link(feature_id, &unique_key()))
        .await
        .expect("committed link");

    let mut tx = pool.begin().await.expect("begin");
    let created = repository
        .create_tx(&mut tx, jira_link(feature_id, &unique_key()))
        .await
        .expect("create in tx");
    let deleted = repository
        .delete_tx(&mut tx, feature_id, committed.id)
        .await
        .expect("delete in tx")
        .expect("deleted row");
    assert_eq!(deleted.id, committed.id);
    tx.rollback().await.expect("rollback");

    let ids: Vec<Uuid> = repository
        .list_for_feature(feature_id)
        .await
        .expect("list")
        .into_iter()
        .map(|link| link.id)
        .collect();
    assert_eq!(ids, vec![committed.id]);
    assert!(!ids.contains(&created.id));

    delete_team(&pool, team_id).await;
}

async fn activity_rows(
    pool: &PgPool,
    activity_type: &str,
    link_id: Uuid,
) -> Vec<serde_json::Value> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT jsonb_build_object( \
             'entity_type', entity_type, 'entity_id', entity_id, \
             'actor_id', actor_id, 'actor_name', actor_name, 'metadata', metadata) \
         FROM activity_log WHERE activity_type = $1 AND metadata->>'link_id' = $2",
    )
    .bind(activity_type)
    .bind(link_id.to_string())
    .fetch_all(pool)
    .await
    .expect("activity rows")
}

fn admin_actor() -> ActorContext {
    ActorContext::new(Uuid::parse_str(SEED_ADMIN_ID).unwrap(), "admin".to_string())
}

fn new_link(key: &str) -> NewExternalLink {
    NewExternalLink {
        system: "jira".to_string(),
        external_key: key.to_string(),
        url: Some(format!("https://acme.atlassian.net/browse/{key}")),
    }
}

#[tokio::test]
async fn create_and_delete_in_tx_write_activity_rows() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository_tx(pool.clone());
    let activity_repo = activity_log_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_id = insert_feature(&pool, team_id, "ext-link-activity").await;
    let key = unique_key();

    let mut tx = pool.begin().await.expect("begin");
    let link = create_external_link_in_tx(
        &mut tx,
        &repository,
        activity_repo.as_ref(),
        feature_id,
        new_link(&key),
        Some(admin_actor()),
    )
    .await
    .expect("create link");
    tx.commit().await.expect("commit");
    assert_eq!(link.created_by, Some(admin_actor().id));

    let expected_metadata = serde_json::json!({
        "feature_id": feature_id.to_string(),
        "feature_key": "ext-link-activity",
        "team_id": team_id.to_string(),
        "link_id": link.id.to_string(),
        "system": "jira",
        "external_key": key,
        "url": format!("https://acme.atlassian.net/browse/{key}"),
    });
    let added = activity_rows(&pool, EXTERNAL_LINK_ADDED, link.id).await;
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["entity_type"], "feature");
    assert_eq!(added[0]["entity_id"], feature_id.to_string());
    assert_eq!(added[0]["actor_id"], SEED_ADMIN_ID);
    assert_eq!(added[0]["actor_name"], "admin");
    assert_eq!(added[0]["metadata"], expected_metadata);

    let mut tx = pool.begin().await.expect("begin");
    delete_external_link_in_tx(
        &mut tx,
        &repository,
        activity_repo.as_ref(),
        feature_id,
        link.id,
        Some(admin_actor()),
    )
    .await
    .expect("delete link");
    tx.commit().await.expect("commit");

    let removed = activity_rows(&pool, EXTERNAL_LINK_REMOVED, link.id).await;
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0]["entity_id"], feature_id.to_string());
    assert_eq!(removed[0]["metadata"], expected_metadata);
    assert!(
        repository
            .list_for_feature(feature_id)
            .await
            .expect("list")
            .is_empty()
    );

    delete_team(&pool, team_id).await;
}

#[tokio::test]
async fn create_in_tx_for_missing_feature_is_not_found() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository_tx(pool.clone());
    let activity_repo = activity_log_repository(pool.clone());
    let missing = Uuid::new_v4();

    let mut tx = pool.begin().await.expect("begin");
    let result = create_external_link_in_tx(
        &mut tx,
        &repository,
        activity_repo.as_ref(),
        missing,
        new_link(&unique_key()),
        Some(admin_actor()),
    )
    .await;
    assert!(
        matches!(result, Err(Error::NotFound(id)) if id == missing),
        "got {result:?}"
    );
}

#[tokio::test]
async fn delete_in_tx_of_a_link_on_another_feature_is_not_found() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository_tx(pool.clone());
    let activity_repo = activity_log_repository(pool.clone());
    let team_id = insert_team(&pool).await;
    let feature_a = insert_feature(&pool, team_id, "ext-link-del-a").await;
    let feature_b = insert_feature(&pool, team_id, "ext-link-del-b").await;
    let link = repository
        .create(jira_link(feature_a, &unique_key()))
        .await
        .expect("create link");

    let mut tx = pool.begin().await.expect("begin");
    let result = delete_external_link_in_tx(
        &mut tx,
        &repository,
        activity_repo.as_ref(),
        feature_b,
        link.id,
        Some(admin_actor()),
    )
    .await;
    assert!(
        matches!(result, Err(Error::NotFound(id)) if id == link.id),
        "got {result:?}"
    );
    tx.rollback().await.expect("rollback");
    assert!(
        activity_rows(&pool, EXTERNAL_LINK_REMOVED, link.id)
            .await
            .is_empty()
    );

    delete_team(&pool, team_id).await;
}

async fn listed_ids_for_key(pool: &PgPool, team_id: Uuid, key: &str) -> Vec<Uuid> {
    let (features, total) = feature_repository(pool.clone())
        .get_features_with_offset_filtered(
            team_id,
            None,
            None,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(key.to_string()),
            0,
            100,
        )
        .await
        .expect("filtered list");
    assert_eq!(total as usize, features.len());
    let mut ids: Vec<Uuid> = features.into_iter().map(|feature| feature.id).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn feature_list_filters_by_external_key_within_the_team() {
    let pool = init_pg_pool().await;
    let repository = external_link_repository(pool.clone());
    let team_a = insert_team(&pool).await;
    let team_b = insert_team(&pool).await;
    let linked_1 = insert_feature(&pool, team_a, "ext-filter-1").await;
    let linked_2 = insert_feature(&pool, team_a, "ext-filter-2").await;
    let _unlinked = insert_feature(&pool, team_a, "ext-filter-3").await;
    let other_link = insert_feature(&pool, team_a, "ext-filter-4").await;
    let other_team = insert_feature(&pool, team_b, "ext-filter-b").await;
    let key = unique_key();

    for feature_id in [linked_1, linked_2, other_team] {
        repository
            .create(jira_link(feature_id, &key))
            .await
            .expect("create link");
    }
    repository
        .create(jira_link(other_link, &unique_key()))
        .await
        .expect("create other link");

    let mut expected = vec![linked_1, linked_2];
    expected.sort();
    assert_eq!(listed_ids_for_key(&pool, team_a, &key).await, expected);
    assert_eq!(
        listed_ids_for_key(&pool, team_b, &key).await,
        vec![other_team]
    );
    assert!(
        listed_ids_for_key(&pool, team_a, &unique_key())
            .await
            .is_empty()
    );

    delete_team(&pool, team_a).await;
    delete_team(&pool, team_b).await;
}
